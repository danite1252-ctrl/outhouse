//! Email: confirmation links, password resets and security notices, sent
//! through any SMTP provider (Brevo, Mailgun, Amazon SES, Gmail with an app
//! password, a local relay...). See deploy/README.md "Email".
//!
//! Settings come from the environment:
//!   SMTP_HOST, SMTP_PORT (587), SMTP_USERNAME, SMTP_PASSWORD,
//!   SMTP_TLS = starttls (default; 465 means tls) | tls | none,
//!   MAIL_FROM = "Outhouse <no-reply@example.com>".
//! Links in emails always use PUBLIC_URL (never the request's Host header),
//! so email counts as set up only when PUBLIC_URL is set too.
//!
//! Sending never holds up a request: mail goes on an in-memory queue and a
//! background task delivers it, retrying a few times. Nothing about a mail
//! (and so no link in one) is ever written to disk or the database.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::state::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// Plain connection upgraded with STARTTLS (port 587); required.
    StartTls,
    /// TLS from the start (port 465).
    Tls,
    /// No encryption: only for a relay on the same machine or tests.
    None,
}

impl TlsMode {
    fn parse(value: Option<&str>, port: u16) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("tls" | "ssl" | "implicit" | "wrapper" | "smtps") => TlsMode::Tls,
            Some("none" | "off" | "plain" | "no" | "false") => TlsMode::None,
            Some("starttls" | "start-tls" | "on" | "yes" | "true") => TlsMode::StartTls,
            _ if port == 465 => TlsMode::Tls,
            _ => TlsMode::StartTls,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            TlsMode::StartTls => "starttls",
            TlsMode::Tls => "tls",
            TlsMode::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MailSettings {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: Mailbox,
    /// PUBLIC_URL, without a trailing slash.
    pub public_url: String,
}

/// One email, ready to go.
#[derive(Debug, Clone)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

#[derive(Debug, Default)]
struct Stats {
    sent: u64,
    failed: u64,
    last_ok_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    last_error_at: Option<DateTime<Utc>>,
}

static SETTINGS: OnceLock<Result<MailSettings, String>> = OnceLock::new();
static QUEUE: OnceLock<mpsc::Sender<Email>> = OnceLock::new();
static STATS: Mutex<Stats> = Mutex::new(Stats {
    sent: 0,
    failed: 0,
    last_ok_at: None,
    last_error: None,
    last_error_at: None,
});

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Reads the settings (once, at start-up). Ok when mail can be sent;
/// otherwise what's missing, for Server status.
pub fn settings_from_env(config: &Config) -> Result<MailSettings, String> {
    let host = env("SMTP_HOST").ok_or("SMTP_HOST isn't set")?;
    let port = match env("SMTP_PORT") {
        Some(p) => p
            .parse::<u16>()
            .map_err(|_| format!("SMTP_PORT ({p}) isn't a port number"))?,
        None => 587,
    };
    let tls = TlsMode::parse(env("SMTP_TLS").as_deref(), port);
    let from_text = env("MAIL_FROM").ok_or("MAIL_FROM isn't set")?;
    let from: Mailbox = from_text.parse().map_err(|_| {
        format!("MAIL_FROM ({from_text}) isn't an address like \"Outhouse <no-reply@example.com>\"")
    })?;
    let public_url = config
        .public_url
        .clone()
        .ok_or("PUBLIC_URL isn't set (links in emails need the site's address)")?;
    Ok(MailSettings {
        host,
        port,
        tls,
        username: env("SMTP_USERNAME"),
        password: env("SMTP_PASSWORD"),
        from,
        public_url,
    })
}

/// Reads the settings and starts the sender. Call once at start-up.
pub fn init(config: &Config) {
    let settings = SETTINGS.get_or_init(|| settings_from_env(config));
    match settings {
        Ok(s) => {
            tracing::info!(host = %s.host, port = s.port, tls = s.tls.as_str(), "email is set up");
            let (tx, rx) = mpsc::channel::<Email>(2000);
            if QUEUE.set(tx).is_ok() {
                tokio::spawn(worker(rx));
            }
        }
        Err(why) => {
            tracing::info!(reason = %why, "email isn't set up: password resets go through the site owner")
        }
    }
}

fn settings() -> Option<&'static MailSettings> {
    SETTINGS.get().and_then(|s| s.as_ref().ok())
}

/// Can mail be sent?
pub fn configured() -> bool {
    settings().is_some() && QUEUE.get().is_some()
}

/// A link to a page on this site, for an email (PUBLIC_URL + path).
pub fn link(path: &str) -> Option<String> {
    settings().map(|s| format!("{}{path}", s.public_url))
}

/// For Server status (site owner only).
pub fn status_json() -> Value {
    let stats = STATS.lock().unwrap_or_else(|e| e.into_inner());
    let (configured, problem, host, from, tls) = match SETTINGS.get() {
        Some(Ok(s)) => (
            true,
            None,
            Some(format!("{}:{}", s.host, s.port)),
            Some(s.from.to_string()),
            Some(s.tls.as_str()),
        ),
        Some(Err(why)) => (false, Some(why.clone()), None, None, None),
        None => (false, Some("not started".to_string()), None, None, None),
    };
    json!({
        "configured": configured,
        "problem": problem,
        "server": host,
        "from": from,
        "tls": tls,
        "sent": stats.sent,
        "failed": stats.failed,
        "last_ok_at": stats.last_ok_at,
        "last_error": stats.last_error,
        "last_error_at": stats.last_error_at,
        "queued": QUEUE.get().map(|q| q.max_capacity() - q.capacity()).unwrap_or(0),
    })
}

/// Puts a mail on the queue. False when email isn't set up or the queue is
/// full (it's never worth failing a request over).
pub fn queue(email: Email) -> bool {
    let Some(q) = QUEUE.get() else {
        return false;
    };
    match q.try_send(email) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "email queue full; a mail was dropped");
            false
        }
    }
}

/// Sends one mail now and says what happened (the site owner's "Send a
/// test email" button).
pub async fn send_now(email: Email) -> Result<(), String> {
    let s = settings().ok_or("Email isn't set up.")?;
    let result = deliver(s, &email).await;
    note(&result);
    result
}

fn note(result: &Result<(), String>) {
    let mut stats = STATS.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(()) => {
            stats.sent += 1;
            stats.last_ok_at = Some(Utc::now());
        }
        Err(e) => {
            stats.failed += 1;
            stats.last_error = Some(e.clone());
            stats.last_error_at = Some(Utc::now());
        }
    }
}

/// Waits between tries: 15 s, 1 min, 5 min, 15 min, then gives up.
const RETRY_DELAYS: [u64; 4] = [15, 60, 300, 900];

async fn worker(mut rx: mpsc::Receiver<Email>) {
    // A few at a time, so one slow provider doesn't hold up the rest.
    let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
    while let Some(email) = rx.recv().await {
        let Some(s) = settings() else { continue };
        let Ok(permit) = limit.clone().acquire_owned().await else {
            break;
        };
        tokio::spawn(async move {
            let _permit = permit;
            let mut attempt = 0;
            loop {
                let result = deliver(s, &email).await;
                note(&result);
                match result {
                    Ok(()) => {
                        tracing::info!(subject = %email.subject, "email sent");
                        return;
                    }
                    Err(e) if attempt < RETRY_DELAYS.len() => {
                        tracing::warn!(error = %e, attempt, "email didn't send; trying again later");
                        tokio::time::sleep(Duration::from_secs(RETRY_DELAYS[attempt])).await;
                        attempt += 1;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, subject = %email.subject, "email gave up after retries");
                        return;
                    }
                }
            }
        });
    }
}

fn transport(s: &MailSettings) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&s.host).port(s.port);
    match s.tls {
        TlsMode::None => {}
        TlsMode::StartTls | TlsMode::Tls => {
            let params = TlsParameters::new(s.host.clone()).map_err(|e| e.to_string())?;
            b = b.tls(if s.tls == TlsMode::Tls {
                Tls::Wrapper(params)
            } else {
                Tls::Required(params)
            });
        }
    }
    if let (Some(u), Some(p)) = (&s.username, &s.password) {
        b = b.credentials(Credentials::new(u.clone(), p.clone()));
    }
    Ok(b.timeout(Some(Duration::from_secs(20))).build())
}

async fn deliver(s: &MailSettings, email: &Email) -> Result<(), String> {
    let to: Mailbox = email
        .to
        .parse()
        .map_err(|_| "That isn't an email address.".to_string())?;
    let message = Message::builder()
        .from(s.from.clone())
        .to(to)
        .subject(email.subject.clone())
        .multipart(MultiPart::alternative_plain_html(
            email.text.clone(),
            email.html.clone(),
        ))
        .map_err(|e| e.to_string())?;
    let mailer = transport(s)?;
    mailer
        .send(message)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ---------- what the emails say ----------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A short, friendly mail: a greeting, some paragraphs, maybe one button,
/// and a quiet footer. Plain text and simple HTML; no pictures, no tracking.
pub fn compose(
    to: &str,
    subject: &str,
    paragraphs: &[String],
    button: Option<(&str, &str)>,
    footer: &str,
) -> Email {
    let mut text = String::new();
    for p in paragraphs {
        text.push_str(p);
        text.push_str("\n\n");
    }
    if let Some((label, url)) = button {
        text.push_str(&format!("{label}:\n{url}\n\n"));
    }
    text.push_str("-- \n");
    text.push_str(footer);
    text.push('\n');

    let mut html = String::from(
        "<!doctype html><html><body style=\"margin:0;padding:24px;background:#121214;font-family:Inter,Segoe UI,Arial,sans-serif;color:#ededed\">\
         <div style=\"max-width:480px;margin:0 auto;background:#161618;border:1px solid #2a2a2e;border-radius:14px;padding:28px\">\
         <div style=\"font:800 22px/1 Montserrat,Arial Black,sans-serif;letter-spacing:.04em;color:#ff6b35;margin-bottom:18px\">OUTHOUSE</div>",
    );
    for p in paragraphs {
        html.push_str(&format!(
            "<p style=\"font-size:15px;line-height:1.5;margin:0 0 14px\">{}</p>",
            esc(p)
        ));
    }
    if let Some((label, url)) = button {
        html.push_str(&format!(
            "<p style=\"margin:20px 0\"><a href=\"{u}\" style=\"display:inline-block;background:#ff6b35;color:#121214;font-weight:700;text-decoration:none;padding:12px 20px;border-radius:10px\">{l}</a></p>\
             <p style=\"font-size:12px;color:#a1a1aa;word-break:break-all;margin:0 0 14px\">Or copy this link: {u}</p>",
            u = esc(url),
            l = esc(label)
        ));
    }
    html.push_str(&format!(
        "<p style=\"font-size:12px;color:#a1a1aa;margin:18px 0 0;border-top:1px solid #2a2a2e;padding-top:12px\">{}</p></div></body></html>",
        esc(footer)
    ));
    Email {
        to: to.to_string(),
        subject: subject.to_string(),
        text,
        html,
    }
}

pub const FOOTER: &str =
    "You're getting this because of your Outhouse account. Outhouse never asks for your password by email.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_modes() {
        assert_eq!(TlsMode::parse(None, 587), TlsMode::StartTls);
        assert_eq!(TlsMode::parse(None, 465), TlsMode::Tls);
        assert_eq!(TlsMode::parse(Some("TLS"), 587), TlsMode::Tls);
        assert_eq!(TlsMode::parse(Some("none"), 25), TlsMode::None);
        assert_eq!(TlsMode::parse(Some("starttls"), 465), TlsMode::StartTls);
    }

    #[test]
    fn mails_escape_and_carry_the_link() {
        let m = compose(
            "a@example.com",
            "Hi",
            &["Hello <b>you</b> & co".to_string()],
            Some(("Open", "https://oh.example/reset?token=abc&x=\"1\"")),
            FOOTER,
        );
        assert!(m
            .text
            .contains("https://oh.example/reset?token=abc&x=\"1\""));
        assert!(m.html.contains("Hello &lt;b&gt;you&lt;/b&gt; &amp; co"));
        assert!(m
            .html
            .contains("href=\"https://oh.example/reset?token=abc&amp;x=&quot;1&quot;\""));
        assert!(!m.html.contains("<img"));
    }
}
