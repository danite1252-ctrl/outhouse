//! Web Push (RFC 8030 / 8291 / 8292): notifications on phones and desktops
//! while Outhouse isn't open, for DMs, mentions, replies and raid notices.
//!
//! The VAPID key pair is made on first start and kept in the database
//! (`push_vapid`), so it survives restarts and comes back with backups.
//! Payloads are encrypted with aes128gcm for the browser's own key; the push
//! service (Google, Mozilla, Microsoft, Apple) only passes them along.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Context};
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use outhouse_common::models::{Message, DM_GUILD};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use ring::{aead, agreement};
use serde_json::json;
use sha2::Sha256;
use sqlx::PgPool;

use crate::perms;
use crate::state::{AppState, Config};

/// Record size advertised in the aes128gcm header.
const RECORD_SIZE: u32 = 4096;
/// Payloads are kept well under the 4 KB push services accept.
const MAX_BODY_CHARS: usize = 180;
/// A subscription that fails this many times in a row is dropped.
const MAX_FAILURES: i32 = 10;
/// Push services keep undelivered notifications this long (seconds).
const TTL_SECONDS: u32 = 4 * 3600;

/// Push services browsers use. Endpoints must be HTTPS on one of these (or a
/// subdomain), so a subscription can't make the server call anything else.
const KNOWN_PUSH_HOSTS: &[&str] = &[
    "fcm.googleapis.com",
    "android.googleapis.com",
    "push.services.mozilla.com",
    "notify.windows.com",
    "push.apple.com",
];

/// The server's VAPID signing key.
pub struct Vapid {
    key: EcdsaKeyPair,
    /// Uncompressed P-256 public key, base64url (what browsers subscribe with).
    pub public_key: String,
}

impl Vapid {
    fn from_pkcs8(pkcs8: &[u8]) -> anyhow::Result<Self> {
        let key = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|e| anyhow!("bad VAPID key: {e}"))?;
        let public_key = URL_SAFE_NO_PAD.encode(key.public_key().as_ref());
        Ok(Self { key, public_key })
    }

    /// `Authorization: vapid t=<JWT>, k=<public key>` for one endpoint.
    fn authorization(&self, endpoint: &str, subject: &str) -> anyhow::Result<String> {
        let url = reqwest::Url::parse(endpoint)?;
        let audience = url.origin().ascii_serialization();
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({
            "aud": audience,
            "exp": Utc::now().timestamp() + 12 * 3600,
            "sub": subject,
        }))?);
        let signing_input = format!("{header}.{claims}");
        let sig = self
            .key
            .sign(&SystemRandom::new(), signing_input.as_bytes())
            .map_err(|_| anyhow!("signing the VAPID token"))?;
        Ok(format!(
            "vapid t={signing_input}.{}, k={}",
            URL_SAFE_NO_PAD.encode(sig.as_ref()),
            self.public_key
        ))
    }
}

/// Loads the VAPID key, making and saving one on first start.
pub async fn load_or_create(db: &PgPool) -> anyhow::Result<Vapid> {
    let existing: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT private_pkcs8 FROM push_vapid WHERE id = 1")
            .fetch_optional(db)
            .await?;
    let pkcs8 = match existing {
        Some(k) => k,
        None => {
            let doc = EcdsaKeyPair::generate_pkcs8(
                &ECDSA_P256_SHA256_FIXED_SIGNING,
                &SystemRandom::new(),
            )
            .map_err(|_| anyhow!("generating a VAPID key"))?;
            sqlx::query(
                "INSERT INTO push_vapid (id, private_pkcs8) VALUES (1, $1)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(doc.as_ref())
            .execute(db)
            .await?;
            tracing::info!("made a new Web Push (VAPID) key");
            // Another server process may have won the race: use whatever's saved.
            sqlx::query_scalar("SELECT private_pkcs8 FROM push_vapid WHERE id = 1")
                .fetch_one(db)
                .await?
        }
    };
    Vapid::from_pkcs8(&pkcs8)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

/// Accepts base64url with or without padding (and plain base64 from older
/// browsers).
pub fn decode_b64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim().trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(s)
        .ok()
        .or_else(|| {
            URL_SAFE
                .decode(format!("{s}{}", "=".repeat((4 - s.len() % 4) % 4)))
                .ok()
        })
        .or_else(|| {
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(s)
                .ok()
        })
}

/// Encrypts a payload for one browser (RFC 8291, "aes128gcm" encoding).
/// `ua_public` is the subscription's p256dh key (65 bytes), `auth` its
/// 16-byte auth secret.
pub fn encrypt(ua_public: &[u8], auth: &[u8], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    if ua_public.len() != 65 || auth.len() < 16 {
        return Err(anyhow!("bad subscription keys"));
    }
    let rng = SystemRandom::new();
    let ephemeral = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| anyhow!("making an ephemeral key"))?;
    let as_public = ephemeral
        .compute_public_key()
        .map_err(|_| anyhow!("ephemeral public key"))?
        .as_ref()
        .to_vec();
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public);
    let ecdh_secret = agreement::agree_ephemeral(ephemeral, &peer, |s| s.to_vec())
        .map_err(|_| anyhow!("key agreement failed (bad p256dh key?)"))?;

    // IKM = HKDF(auth, ecdh_secret, "WebPush: info" || 0 || ua_public || as_public, 32)
    let prk_key = hmac_sha256(auth, &ecdh_secret);
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(&as_public);
    key_info.push(1);
    let ikm = hmac_sha256(&prk_key, &key_info);

    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|_| anyhow!("random salt"))?;
    let prk = hmac_sha256(&salt, &ikm);
    let cek = hmac_sha256(&prk, b"Content-Encoding: aes128gcm\0\x01");
    let nonce = hmac_sha256(&prk, b"Content-Encoding: nonce\0\x01");

    // One record: the payload, then the 0x02 "last record" delimiter.
    let mut record = plaintext.to_vec();
    record.push(2);
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, &cek[..16])
            .map_err(|_| anyhow!("content key"))?,
    );
    let nonce =
        aead::Nonce::try_assume_unique_for_key(&nonce[..12]).map_err(|_| anyhow!("nonce"))?;
    key.seal_in_place_append_tag(nonce, aead::Aad::empty(), &mut record)
        .map_err(|_| anyhow!("encrypting"))?;

    let mut body = Vec::with_capacity(21 + as_public.len() + record.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// Is this a push service endpoint the server may send to?
pub fn endpoint_allowed(config: &Config, endpoint: &str) -> bool {
    if endpoint.len() > 1000 {
        return false;
    }
    if config
        .push_extra_endpoints
        .iter()
        .any(|prefix| endpoint.starts_with(prefix.as_str()))
    {
        return true;
    }
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "https" || url.port().is_some() || !url.username().is_empty() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let matches = |known: &str| host == known || host.ends_with(&format!(".{known}"));
    KNOWN_PUSH_HOSTS.iter().any(|k| matches(k))
        || config.push_allowed_hosts.iter().any(|k| matches(k))
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("Outhouse/0.2 (web push)")
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("building the push HTTP client")
    })
}

#[derive(sqlx::FromRow)]
struct SubRow {
    id: i64,
    endpoint: String,
    p256dh: String,
    auth: String,
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct SendResult {
    pub sent: usize,
    pub failed: usize,
    /// Subscriptions the push service said are gone (deleted here too).
    pub removed: usize,
}

/// Sends one notification to every device the user turned push on for.
pub async fn send_to_user(
    st: &AppState,
    user_id: i64,
    payload: &serde_json::Value,
) -> anyhow::Result<SendResult> {
    let Some(vapid) = st.vapid.clone() else {
        return Ok(SendResult::default());
    };
    let subs = sqlx::query_as::<_, SubRow>(
        "SELECT id, endpoint, p256dh, auth FROM push_subscriptions WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&st.db)
    .await?;
    let body = serde_json::to_vec(payload)?;
    let mut result = SendResult::default();
    for sub in subs {
        match send_one(st, &vapid, &sub, &body).await {
            Ok(Delivery::Sent) => {
                result.sent += 1;
                sqlx::query(
                    "UPDATE push_subscriptions SET failures = 0, last_ok_at = now() WHERE id = $1",
                )
                .bind(sub.id)
                .execute(&st.db)
                .await?;
            }
            Ok(Delivery::Gone) => {
                result.removed += 1;
                sqlx::query("DELETE FROM push_subscriptions WHERE id = $1")
                    .bind(sub.id)
                    .execute(&st.db)
                    .await?;
            }
            Err(e) => {
                result.failed += 1;
                tracing::warn!(error = %e, "push notification failed");
                sqlx::query("UPDATE push_subscriptions SET failures = failures + 1 WHERE id = $1")
                    .bind(sub.id)
                    .execute(&st.db)
                    .await?;
                sqlx::query("DELETE FROM push_subscriptions WHERE id = $1 AND failures >= $2")
                    .bind(sub.id)
                    .bind(MAX_FAILURES)
                    .execute(&st.db)
                    .await?;
            }
        }
    }
    Ok(result)
}

enum Delivery {
    Sent,
    Gone,
}

async fn send_one(
    st: &AppState,
    vapid: &Arc<Vapid>,
    sub: &SubRow,
    body: &[u8],
) -> anyhow::Result<Delivery> {
    if !endpoint_allowed(&st.config, &sub.endpoint) {
        return Ok(Delivery::Gone);
    }
    let ua_public = decode_b64(&sub.p256dh).context("p256dh isn't base64")?;
    let auth = decode_b64(&sub.auth).context("auth isn't base64")?;
    let encrypted = encrypt(&ua_public, &auth, body)?;
    let res = client()
        .post(&sub.endpoint)
        .header(
            "Authorization",
            vapid.authorization(&sub.endpoint, &st.config.vapid_subject)?,
        )
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("TTL", TTL_SECONDS.to_string())
        .header("Urgency", "high")
        .body(encrypted)
        .send()
        .await?;
    let status = res.status().as_u16();
    match status {
        200..=299 => Ok(Delivery::Sent),
        404 | 410 => Ok(Delivery::Gone),
        _ => {
            let text: String = res
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            Err(anyhow!("push service answered {status}: {text}"))
        }
    }
}

// ---- Who gets told about a new message ---------------------------------------

/// Notification level for an outhouse (guild 0 = direct messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    All,
    Mentions,
    Nothing,
}

impl Level {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "all" => Some(Self::All),
            "mentions" => Some(Self::Mentions),
            "nothing" => Some(Self::Nothing),
            _ => None,
        }
    }

    pub fn default_for(guild_id: i64) -> Self {
        if guild_id == DM_GUILD {
            Self::All
        } else {
            Self::Mentions
        }
    }
}

/// Does this message mention the user (directly, a role of theirs,
/// @everyone/@here) or reply to them?
pub fn mentions(message: &Message, user_id: i64, role_ids: &[i64]) -> bool {
    if message.reply_to.as_ref().map(|r| r.author_id) == Some(user_id) {
        return true;
    }
    let content = &message.content;
    if content.contains(&format!("<@{user_id}>")) {
        return true;
    }
    if content.split_whitespace().any(|w| {
        let w = w.trim_end_matches(|c: char| !c.is_alphanumeric());
        w == "@everyone" || w == "@here"
    }) {
        return true;
    }
    role_ids
        .iter()
        .any(|id| content.contains(&format!("<@&{id}>")))
}

/// IDs inside `<@123>` / `<@&123>` tokens.
pub(crate) fn mention_ids(content: &str, prefix: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find(prefix) {
        let after = &rest[start + prefix.len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && after[digits.len()..].starts_with('>') {
            if let Ok(id) = digits.parse() {
                out.push(id);
            }
        }
        rest = after;
    }
    out
}

/// Message text for a notification: mentions as @names, custom emoji as
/// :name:, no markdown stars, kept short.
async fn plain_text(db: &PgPool, guild_id: i64, content: &str) -> String {
    let mut text = content.to_string();
    let users = mention_ids(content, "<@");
    if !users.is_empty() {
        let names: Vec<(i64, String)> = sqlx::query_as(
            "SELECT u.id, COALESCE(m.nickname, u.display_name)
             FROM users u
             LEFT JOIN guild_members m ON m.user_id = u.id AND m.guild_id = $2
             WHERE u.id = ANY($1)",
        )
        .bind(&users)
        .bind(guild_id)
        .fetch_all(db)
        .await
        .unwrap_or_default();
        for (id, name) in names {
            text = text.replace(&format!("<@{id}>"), &format!("@{name}"));
        }
    }
    let roles = mention_ids(content, "<@&");
    if !roles.is_empty() {
        let names: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, name FROM roles WHERE id = ANY($1)")
                .bind(&roles)
                .fetch_all(db)
                .await
                .unwrap_or_default();
        for (id, name) in names {
            text = text.replace(&format!("<@&{id}>"), &format!("@{name}"));
        }
    }
    // <:name:123> → :name:
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('>') {
            Some(end) if end <= 60 => {
                let inner = &after[..end];
                let name = inner.split(':').next().unwrap_or("");
                out.push(':');
                out.push_str(name);
                out.push(':');
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("<:");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    // <t:…> time tags: the server can't know the reader's zone, so UTC.
    let out = crate::timetag::plain(&out);
    let out = out.replace("**", "").replace("__", "");
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > MAX_BODY_CHARS {
        let cut: String = out.chars().take(MAX_BODY_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        out
    }
}

#[derive(sqlx::FromRow)]
struct Candidate {
    user_id: i64,
    level: Option<String>,
}

/// Called after a message is posted: pushes it to people who should hear
/// about it and aren't looking at Outhouse right now. Runs in the background.
pub fn spawn_message_push(st: &AppState, guild_id: i64, message: &Message) {
    if st.vapid.is_none() {
        return;
    }
    let st = st.clone();
    let message = message.clone();
    tokio::spawn(async move {
        if let Err(e) = message_push(&st, guild_id, &message).await {
            tracing::warn!(error = %e, "push notifications for a message failed");
        }
    });
}

async fn message_push(st: &AppState, guild_id: i64, message: &Message) -> anyhow::Result<()> {
    let channel_id = message.channel_id;
    // Only people who turned push on somewhere, and haven't muted the channel.
    let candidates = if guild_id == DM_GUILD {
        sqlx::query_as::<_, Candidate>(
            "SELECT DISTINCT p.user_id, l.level
             FROM dm_participants p
             JOIN push_subscriptions s ON s.user_id = p.user_id
             LEFT JOIN notify_levels l ON l.user_id = p.user_id AND l.guild_id = 0
             WHERE p.channel_id = $1 AND p.user_id <> $2
               AND NOT EXISTS (SELECT 1 FROM user_blocks b
                               WHERE b.user_id = p.user_id AND b.blocked_id = $2)
               AND NOT EXISTS (SELECT 1 FROM channel_mutes cm
                               WHERE cm.user_id = p.user_id AND cm.channel_id = $1
                                 AND (cm.muted_until IS NULL OR cm.muted_until > now()))",
        )
        .bind(channel_id)
        .bind(message.author_id)
        .fetch_all(&st.db)
        .await?
    } else {
        sqlx::query_as::<_, Candidate>(
            "SELECT DISTINCT m.user_id, l.level
             FROM guild_members m
             JOIN push_subscriptions s ON s.user_id = m.user_id
             LEFT JOIN notify_levels l ON l.user_id = m.user_id AND l.guild_id = $1
             WHERE m.guild_id = $1 AND m.user_id <> $3
               AND NOT EXISTS (SELECT 1 FROM user_blocks b
                               WHERE b.user_id = m.user_id AND b.blocked_id = $3)
               AND NOT EXISTS (SELECT 1 FROM channel_mutes cm
                               WHERE cm.user_id = m.user_id AND cm.channel_id = $2
                                 AND (cm.muted_until IS NULL OR cm.muted_until > now()))",
        )
        .bind(guild_id)
        .bind(channel_id)
        .bind(message.author_id)
        .fetch_all(&st.db)
        .await?
    };
    if candidates.is_empty() {
        return Ok(());
    }

    let (guild_name, channel_name): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT g.name, c.name FROM channels c LEFT JOIN guilds g ON g.id = c.guild_id
         WHERE c.id = $1",
    )
    .bind(channel_id)
    .fetch_optional(&st.db)
    .await?
    .unwrap_or((None, None));
    // Thread replies say which thread (and open it).
    let thread_name: Option<String> = match message.thread_id {
        Some(thread_id) => {
            sqlx::query_scalar("SELECT name FROM threads WHERE id = $1")
                .bind(thread_id)
                .fetch_optional(&st.db)
                .await?
        }
        None => None,
    };
    let place = match &thread_name {
        Some(t) => format!("🧵 {t} (#{})", channel_name.as_deref().unwrap_or("chat")),
        None => format!("#{}", channel_name.as_deref().unwrap_or("chat")),
    };
    let tag = match message.thread_id {
        Some(t) => format!("th-{t}"),
        None => format!("ch-{channel_id}"),
    };
    let thread_q = message
        .thread_id
        .map(|t| format!("&thread={t}"))
        .unwrap_or_default();
    let mut body = plain_text(&st.db, guild_id, &message.content).await;
    if body.is_empty() {
        body = if let Some(poll) = &message.poll {
            format!("📊 {}", poll.question)
        } else if let Some(ev) = &message.event {
            format!("📅 {}", ev.title)
        } else if let Some(s) = &message.sticker {
            format!("🏷 Sent a sticker: {}", s.name)
        } else if let Some(f) = &message.forward {
            format!("↪ Forwarded a message from {}", f.author_display_name)
        } else if message.attachments.iter().any(|a| a.voice) {
            "🎤 Voice message".into()
        } else if !message.attachments.is_empty() {
            "Sent an attachment".into()
        } else if let Some(e) = message.embeds.first() {
            e.title.clone().unwrap_or_else(|| "Sent a link".into())
        } else {
            "New message".into()
        };
    }
    let author = &message.author_display_name;
    let raid_reminder = message.author_bot && message.content.starts_with('⏰');
    let promoted = guild_id == DM_GUILD && message.author_bot && message.content.starts_with("🎉");

    let mut role_cache: HashMap<i64, Vec<i64>> = HashMap::new();
    for c in candidates {
        let level = c
            .level
            .as_deref()
            .and_then(Level::parse)
            .unwrap_or(Level::default_for(guild_id));
        if level == Level::Nothing || !st.is_away(c.user_id) {
            continue;
        }
        let kind = if guild_id == DM_GUILD {
            if promoted {
                "raid"
            } else {
                "dm"
            }
        } else {
            if !perms::can_view(&st.db, guild_id, channel_id, c.user_id).await {
                continue;
            }
            let role_ids = match role_cache.get(&c.user_id) {
                Some(r) => r.clone(),
                None => {
                    let r: Vec<i64> = sqlx::query_scalar(
                        "SELECT role_id FROM member_roles WHERE guild_id = $1 AND user_id = $2",
                    )
                    .bind(guild_id)
                    .bind(c.user_id)
                    .fetch_all(&st.db)
                    .await?;
                    role_cache.insert(c.user_id, r.clone());
                    r
                }
            };
            let mentioned = mentions(message, c.user_id, &role_ids);
            if level == Level::Mentions && !mentioned {
                continue;
            }
            if raid_reminder && mentioned {
                "raid"
            } else if message.reply_to.as_ref().map(|r| r.author_id) == Some(c.user_id) {
                "reply"
            } else if mentioned {
                "mention"
            } else {
                "message"
            }
        };
        // Age-restricted channel they haven't said they're 18+ for: say
        // something arrived, but not who or what.
        if guild_id != DM_GUILD && !perms::content_allowed(&st.db, channel_id, c.user_id).await {
            let payload = json!({
                "title": format!(
                    "New message in #{}",
                    channel_name.as_deref().unwrap_or("chat")
                ),
                "body": "This channel is age-restricted. Open Outhouse to see it.",
                "kind": kind,
                "guild_id": guild_id.to_string(),
                "channel_id": channel_id.to_string(),
                "message_id": message.id.to_string(),
                "tag": format!("ch-{channel_id}"),
                "url": format!("/?guild={guild_id}&channel={channel_id}"),
            });
            send_to_user(st, c.user_id, &payload).await?;
            continue;
        }
        let title = match (kind, guild_id == DM_GUILD) {
            ("raid", true) => "🎉 You're off the bench!".to_string(),
            ("raid", false) => "⏰ Raid reminder".to_string(),
            (_, true) => match &channel_name {
                Some(name) if !name.is_empty() => format!("{author} in {name}"),
                _ => author.clone(),
            },
            ("reply", false) => format!("{author} replied in {place}"),
            (_, false) => format!(
                "{author} in {place}{}",
                guild_name
                    .as_deref()
                    .map(|g| format!(" ({g})"))
                    .unwrap_or_default()
            ),
        };
        let payload = json!({
            "title": title,
            "body": body,
            "kind": kind,
            "guild_id": guild_id.to_string(),
            "channel_id": channel_id.to_string(),
            "message_id": message.id.to_string(),
            "thread_id": message.thread_id.map(|t| t.to_string()),
            "tag": tag,
            "url": format!("/app?guild={guild_id}&channel={channel_id}{thread_q}"),
        });
        send_to_user(st, c.user_id, &payload).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decrypt(
        ua_private: agreement::EphemeralPrivateKey,
        ua_public: &[u8],
        auth: &[u8],
        body: &[u8],
    ) -> Vec<u8> {
        let salt = &body[..16];
        let idlen = body[20] as usize;
        let as_public = &body[21..21 + idlen];
        let ciphertext = &body[21 + idlen..];
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_public);
        let ecdh = agreement::agree_ephemeral(ua_private, &peer, |s| s.to_vec()).unwrap();
        let prk_key = hmac_sha256(auth, &ecdh);
        let mut info = b"WebPush: info\0".to_vec();
        info.extend_from_slice(ua_public);
        info.extend_from_slice(as_public);
        info.push(1);
        let ikm = hmac_sha256(&prk_key, &info);
        let prk = hmac_sha256(salt, &ikm);
        let cek = hmac_sha256(&prk, b"Content-Encoding: aes128gcm\0\x01");
        let nonce = hmac_sha256(&prk, b"Content-Encoding: nonce\0\x01");
        let key =
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek[..16]).unwrap());
        let mut buf = ciphertext.to_vec();
        let plain = key
            .open_in_place(
                aead::Nonce::try_assume_unique_for_key(&nonce[..12]).unwrap(),
                aead::Aad::empty(),
                &mut buf,
            )
            .unwrap();
        assert_eq!(plain.last(), Some(&2));
        plain[..plain.len() - 1].to_vec()
    }

    #[test]
    fn encrypts_for_the_browser_key() {
        let rng = SystemRandom::new();
        let ua = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let ua_public = ua.compute_public_key().unwrap().as_ref().to_vec();
        let auth = [7u8; 16];
        let body = encrypt(&ua_public, &auth, b"{\"title\":\"hi\"}").unwrap();
        assert_eq!(
            u32::from_be_bytes([body[16], body[17], body[18], body[19]]),
            RECORD_SIZE
        );
        assert_eq!(decrypt(ua, &ua_public, &auth, &body), b"{\"title\":\"hi\"}");
    }

    #[test]
    fn finds_mention_ids() {
        assert_eq!(mention_ids("hi <@12> and <@&34> <@x> <@56", "<@"), vec![12]);
        assert_eq!(mention_ids("hi <@12> and <@&34>", "<@&"), vec![34]);
    }

    #[test]
    fn decodes_both_base64_flavours() {
        assert_eq!(decode_b64("AQID").unwrap(), vec![1, 2, 3]);
        assert_eq!(decode_b64("_-8").unwrap(), vec![255, 239]);
        assert_eq!(decode_b64("_-8=").unwrap(), vec![255, 239]);
    }
}
