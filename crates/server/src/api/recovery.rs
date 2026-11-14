//! Email addresses and getting back into your account:
//!
//! - every new account gives an email; it's confirmed with a one-time link
//!   (24 hours). Unconfirmed accounts work as normal, but password resets
//!   only ever go to a confirmed address;
//! - "Forgot password?": a one-time link by email (30 minutes). The answer
//!   is always the same, whether or not the account or address exists;
//! - the reset page also asks for a two-factor code when the account has
//!   two-factor on (an inbox alone isn't enough to get in);
//! - the site owner can make a reset link by hand (People), for someone
//!   who lost their email.
//!
//! Addresses are kept lower-cased. Two accounts may name the same address
//! while it's unconfirmed; confirming it makes it that account's alone.
//! Nothing here ever says whether an address is in use: signing up (or
//! changing to) an address another account has confirmed looks exactly the
//! same, but no link is sent: that address gets a short "you already have
//! an account" note instead, and the new account can never confirm it.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::mail;
use crate::security::{self, ClientIp};
use crate::state::AppState;

pub const VERIFY_HOURS: i32 = 24;
pub const RESET_MINUTES: i32 = 30;
pub const OWNER_RESET_HOURS: i32 = 24;
const MAX_EMAIL: usize = 254;

/// The one answer "Forgot password?" ever gives (when email is set up).
pub const FORGOT_SENT: &str = "If that account has a confirmed email, we've sent a link. It works for 30 minutes. Check your spam folder too.";
/// ...and when this server can't send email at all.
pub const FORGOT_NO_EMAIL: &str = "This server can't send email yet, so password resets go through the server owner. Ask them for a reset link.";
const LINK_DEAD: &str = "This link has expired or was already used. Ask for a new one.";

// ---------- addresses ----------

/// A tidy, lower-cased address, or why it isn't one.
pub fn normalize_email(raw: &str) -> AppResult<String> {
    let bad = || {
        AppError::BadRequest(
            "That doesn't look like an email address (like name@example.com).".into(),
        )
    };
    let e = raw.trim().to_lowercase();
    if e.is_empty() {
        return Err(AppError::BadRequest(
            "Add an email address, so you can reset your password if you forget it.".into(),
        ));
    }
    if e.chars().count() > MAX_EMAIL {
        return Err(AppError::BadRequest(
            "Email addresses can be up to 254 characters.".into(),
        ));
    }
    let (local, domain) = e.rsplit_once('@').ok_or_else(bad)?;
    let ok_local = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .chars()
            .all(|c| c.is_alphanumeric() || "!#$%&'*+-/=?^_`{|}~.".contains(c));
    let labels: Vec<&str> = domain.split('.').collect();
    let ok_domain = labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        && labels.last().is_some_and(|tld| tld.chars().count() >= 2);
    if ok_local && ok_domain {
        Ok(e)
    } else {
        Err(bad())
    }
}

/// `jo•••@example.com`, for the site owner and notices.
pub fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let shown: String = local.chars().take(2).collect();
            format!("{shown}•••@{domain}")
        }
        None => "•••".into(),
    }
}

// ---------- one-time links ----------

/// Makes a one-time token and returns it (only its hash is kept). A new
/// reset link replaces any older one for the account.
pub async fn new_link_token(
    db: &PgPool,
    user_id: i64,
    purpose: &str,
    email: Option<&str>,
    minutes: i32,
    created_by: Option<i64>,
) -> AppResult<String> {
    let token = auth::new_token();
    let mut tx = db.begin().await?;
    sqlx::query(
        "DELETE FROM email_tokens WHERE user_id = $1 AND purpose = $2
           AND (used_at IS NOT NULL OR expires_at <= now() OR $2 = 'reset')",
    )
    .bind(user_id)
    .bind(purpose)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO email_tokens (token_hash, user_id, purpose, email, created_by, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(mins => $6))",
    )
    .bind(auth::token_hash(&token))
    .bind(user_id)
    .bind(purpose)
    .bind(email)
    .bind(created_by)
    .bind(minutes)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(token)
}

fn token_ok(token: &str) -> bool {
    token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit())
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    user_id: i64,
    email: Option<String>,
    created_by: Option<i64>,
}

async fn live_link(db: &PgPool, token: &str, purpose: &str) -> AppResult<Option<LinkRow>> {
    if !token_ok(token) {
        return Ok(None);
    }
    Ok(sqlx::query_as::<_, LinkRow>(
        "SELECT user_id, email, created_by FROM email_tokens
         WHERE token_hash = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(auth::token_hash(token))
    .bind(purpose)
    .fetch_optional(db)
    .await?)
}

/// Signs someone out everywhere: sessions, trusted devices, half-finished
/// two-factor sign-ins and reset links (after a password change or reset).
pub async fn revoke_everything(
    db: &PgPool,
    user_id: i64,
    keep_session: Option<&str>,
) -> AppResult<()> {
    let mut tx = db.begin().await?;
    match keep_session {
        Some(token) => {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_hash <> $2")
                .bind(user_id)
                .bind(auth::token_hash(token))
                .execute(&mut *tx)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
    }
    for sql in [
        "DELETE FROM trusted_devices WHERE user_id = $1",
        "DELETE FROM pending_logins WHERE user_id = $1",
        "DELETE FROM email_tokens WHERE user_id = $1 AND purpose = 'reset'",
    ] {
        sqlx::query(sql).bind(user_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------- emails ----------

#[derive(sqlx::FromRow)]
struct Who {
    username: String,
    display_name: String,
    email: Option<String>,
    verified: bool,
}

async fn who(db: &PgPool, user_id: i64) -> AppResult<Who> {
    Ok(sqlx::query_as::<_, Who>(
        "SELECT username, display_name, email, email_verified_at IS NOT NULL AS verified
         FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

/// The confirmed address of an account, for security notices.
pub async fn confirmed_email(db: &PgPool, user_id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT email FROM users WHERE id = $1 AND email_verified_at IS NOT NULL")
        .bind(user_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

/// A security notice to someone's confirmed address (if they have one).
pub async fn notify(db: &PgPool, user_id: i64, subject: &str, paragraphs: Vec<String>) {
    if !mail::configured() {
        return;
    }
    let Some(to) = confirmed_email(db, user_id).await else {
        return;
    };
    mail::queue(mail::compose(&to, subject, &paragraphs, None, mail::FOOTER));
}

async fn email_taken(db: &PgPool, email: &str, user_id: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE email = $1 AND email_verified_at IS NOT NULL AND id <> $2)",
    )
    .bind(email)
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

/// Sends the confirmation link for `email` (or, when another account has
/// already confirmed that address, a note saying so instead: see the top of
/// this file). Does nothing when email isn't set up.
pub async fn send_confirmation(st: &AppState, user_id: i64, email: &str) -> AppResult<()> {
    if !mail::configured() {
        return Ok(());
    }
    let w = who(&st.db, user_id).await?;
    if email_taken(&st.db, email, user_id).await? {
        let sign_in = mail::link("/login").unwrap_or_default();
        mail::queue(mail::compose(
            email,
            "You already have an Outhouse account",
            &[
                "Hi! Someone just asked to use this address for an Outhouse account, but it already belongs to one, so we didn't add it.".into(),
                "If that was you, sign in to the account you already have. Forgot its password? Use \"Forgot password?\" on the sign-in screen.".into(),
                "If it wasn't you, you can ignore this email.".into(),
            ],
            Some(("Sign in", &sign_in)),
            mail::FOOTER,
        ));
        return Ok(());
    }
    let token = new_link_token(
        &st.db,
        user_id,
        "verify",
        Some(email),
        VERIFY_HOURS * 60,
        None,
    )
    .await?;
    let Some(url) = mail::link(&format!("/verify-email?token={token}")) else {
        return Ok(());
    };
    mail::queue(mail::compose(
        email,
        "Confirm your email for Outhouse",
        &[
            format!("Hi {},", w.display_name),
            format!(
                "Tap the button to confirm this is the email for your Outhouse account (@{}). Then, if you ever forget your password, we can send you a reset link here.",
                w.username
            ),
            "The link works once, for 24 hours. If you didn't make an Outhouse account, you can ignore this.".into(),
        ],
        Some(("Confirm my email", &url)),
        mail::FOOTER,
    ));
    Ok(())
}

// ---------- what /api/users/@me says about it ----------

/// Email and two-factor details for `/api/users/@me` (the banners and
/// settings use them).
pub async fn account_flags(st: &AppState, user_id: i64) -> AppResult<Value> {
    #[derive(sqlx::FromRow)]
    struct Row {
        email: Option<String>,
        verified: bool,
        email_pending: Option<String>,
        two_factor: bool,
        moderator: bool,
    }
    let r = sqlx::query_as::<_, Row>(
        "SELECT u.email, u.email_verified_at IS NOT NULL AS verified, u.email_pending,
                u.totp_secret IS NOT NULL AS two_factor,
                (EXISTS(SELECT 1 FROM guilds g WHERE g.owner_id = u.id)
                 OR EXISTS(SELECT 1 FROM member_roles mr JOIN roles r ON r.id = mr.role_id
                           WHERE mr.user_id = u.id AND (r.permissions & $2) <> 0)) AS moderator
         FROM users u WHERE u.id = $1",
    )
    .bind(user_id)
    .bind(
        (outhouse_common::permissions::MANAGE_GUILD
            | outhouse_common::permissions::BAN_MEMBERS
            | outhouse_common::permissions::ADMINISTRATOR) as i64,
    )
    .fetch_one(&st.db)
    .await?;
    let owner = super::audit::is_site_owner(&st.db, user_id).await?;
    let nudge = if r.two_factor {
        None
    } else if owner || r.moderator {
        Some("strong")
    } else {
        Some("gentle")
    };
    Ok(json!({
        "email": r.email,
        "email_verified": r.verified,
        "email_pending": r.email_pending,
        // An account from before email was needed: the "Add an email" warning.
        "email_missing": r.email.is_none(),
        // Can this server send email at all?
        "email_available": mail::configured(),
        "two_factor": r.two_factor,
        // "Protect your account with two-factor": gentle, or stronger for
        // the site owner and anyone who can manage or ban in an outhouse.
        "two_factor_nudge": nudge,
        "site_owner": owner,
    }))
}

// ---------- your own email ----------

#[derive(Deserialize)]
pub struct EmailChange {
    pub email: String,
    #[serde(default)]
    pub password: String,
}

/// `PUT /api/users/@me/email` `{email, password}`: add an email (accounts
/// from before it was needed) or change it. Changing a confirmed address
/// keeps the old one until the new one is confirmed, and the old one gets
/// a heads-up. It can't be removed (only changed).
pub async fn change_email(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<EmailChange>,
) -> AppResult<Json<Value>> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    let email = normalize_email(&req.email)?;
    if security::get().rate_limits
        && !security::limiter().allow(
            &format!("email-change:{}", user.id),
            5,
            std::time::Duration::from_secs(3600),
        )
    {
        return Err(AppError::TooManyRequests(
            "That's a lot of email changes. Try again in an hour.".into(),
        ));
    }
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&st.db)
        .await?;
    if req.password.chars().count() > super::account::MAX_PASSWORD_CHARS
        || !auth::verify_password(req.password.clone(), hash).await
    {
        return Err(AppError::BadRequest("Your password isn't right.".into()));
    }
    let w = who(&st.db, user.id).await?;
    if w.email.as_deref() == Some(email.as_str()) {
        if w.verified {
            return Err(AppError::BadRequest("That's already your email.".into()));
        }
        // The same unconfirmed address again: just send a fresh link.
        send_confirmation(&st, user.id, &email).await?;
        return Ok(Json(account_flags(&st, user.id).await?));
    }
    if w.verified {
        // Keep the confirmed address until the new one is confirmed.
        sqlx::query("UPDATE users SET email_pending = $2 WHERE id = $1")
            .bind(user.id)
            .bind(&email)
            .execute(&st.db)
            .await?;
        if let Some(old) = &w.email {
            if mail::configured() {
                mail::queue(mail::compose(
                    old,
                    "Your Outhouse email is being changed",
                    &[
                        format!("Hi {},", w.display_name),
                        format!(
                            "Someone signed in as @{} asked to change the account's email to {}. This address stays on the account until the new one is confirmed.",
                            w.username,
                            mask_email(&email)
                        ),
                        "If that was you, there's nothing to do. If it wasn't, change your password now (or use \"Forgot password?\" on the sign-in screen) and tell the server owner.".into(),
                    ],
                    None,
                    mail::FOOTER,
                ));
            }
        }
    } else {
        sqlx::query(
            "UPDATE users SET email = $2, email_verified_at = NULL, email_pending = NULL WHERE id = $1",
        )
        .bind(user.id)
        .bind(&email)
        .execute(&st.db)
        .await?;
    }
    sqlx::query("DELETE FROM email_tokens WHERE user_id = $1 AND purpose = 'verify'")
        .bind(user.id)
        .execute(&st.db)
        .await?;
    send_confirmation(&st, user.id, &email).await?;
    super::safety::log_action(
        &st,
        Some(user.id),
        "email_changed",
        None,
        Some(user.id),
        if w.email.is_none() {
            format!("Added an email ({}), not confirmed yet", mask_email(&email))
        } else {
            format!(
                "Asked to change their email to {} (not confirmed yet)",
                mask_email(&email)
            )
        },
    )
    .await;
    Ok(Json(account_flags(&st, user.id).await?))
}

/// `POST /api/users/@me/email/resend`: another confirmation link.
pub async fn resend(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    if !mail::configured() {
        return Err(AppError::BadRequest(
            "This server can't send email yet. Your link will come once the server owner sets it up.".into(),
        ));
    }
    let (email, pending, verified): (Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT email, email_pending, email_verified_at IS NOT NULL FROM users WHERE id = $1",
    )
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    let target = match (pending, email, verified) {
        (Some(p), _, _) => p,
        (None, Some(e), false) => e,
        (None, Some(_), true) => {
            return Err(AppError::BadRequest(
                "Your email is already confirmed.".into(),
            ))
        }
        (None, None, _) => return Err(AppError::BadRequest("Add an email first.".into())),
    };
    if security::get().rate_limits
        && !security::limiter().allow(
            &format!("email-resend:{}", user.id),
            3,
            std::time::Duration::from_secs(3600),
        )
    {
        return Err(AppError::TooManyRequests(
            "We've sent a few links already. Check your spam folder, or try again in an hour."
                .into(),
        ));
    }
    send_confirmation(&st, user.id, &target).await?;
    Ok(Json(json!({ "sent_to": mask_email(&target) })))
}

#[derive(Deserialize)]
pub struct TokenBody {
    #[serde(default)]
    pub token: String,
}

/// `POST /api/auth/verify-email` `{token}` (the page the email links to).
pub async fn verify_email(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<TokenBody>,
) -> AppResult<Json<Value>> {
    if security::get().rate_limits
        && !security::limiter().allow(
            &format!("verify-ip:{ip}"),
            30,
            std::time::Duration::from_secs(3600),
        )
    {
        return Err(AppError::TooManyRequests(
            "Too many tries. Wait an hour.".into(),
        ));
    }
    let Some(link) = live_link(&st.db, req.token.trim(), "verify").await? else {
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    };
    let Some(email) = link.email else {
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    };
    let mut tx = st.db.begin().await?;
    // Only one confirmation per link, even if it's opened twice at once.
    let used = sqlx::query(
        "UPDATE email_tokens SET used_at = now()
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(auth::token_hash(req.token.trim()))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if used == 0 {
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    }
    let result = sqlx::query(
        "UPDATE users SET email = $2, email_verified_at = now(),
                email_pending = CASE WHEN email_pending = $2 THEN NULL ELSE email_pending END
         WHERE id = $1 AND (email = $2 OR email_pending = $2)",
    )
    .bind(link.user_id)
    .bind(&email)
    .execute(&mut *tx)
    .await;
    let changed = match result {
        Ok(r) => r.rows_affected(),
        // Another account confirmed this address first.
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => 0,
        Err(e) => return Err(e.into()),
    };
    if changed == 0 {
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    }
    tx.commit().await?;
    // Other unconfirmed accounts naming the same address can't confirm it now.
    super::safety::log_action(
        &st,
        Some(link.user_id),
        "email_changed",
        None,
        Some(link.user_id),
        format!("Confirmed their email ({})", mask_email(&email)),
    )
    .await;
    Ok(Json(json!({ "ok": true, "email": mask_email(&email) })))
}

// ---------- forgot password ----------

#[derive(Deserialize)]
pub struct ForgotBody {
    /// A username or an email address.
    #[serde(default)]
    pub login: String,
}

/// `POST /api/auth/forgot` `{login}`: always the same answer.
pub async fn forgot(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<ForgotBody>,
) -> AppResult<Json<Value>> {
    let limits = security::get().rate_limits;
    let hour = std::time::Duration::from_secs(3600);
    if limits && !security::limiter().allow(&format!("forgot-ip:{ip}"), 5, hour) {
        return Err(AppError::TooManyRequests(
            "That's a lot of reset requests. Try again in an hour.".into(),
        ));
    }
    if !mail::configured() {
        return Ok(Json(
            json!({ "sent": false, "email_available": false, "message": FORGOT_NO_EMAIL }),
        ));
    }
    let neutral = Json(json!({ "sent": true, "email_available": true, "message": FORGOT_SENT }));
    let login = req.login.trim().to_lowercase();
    if login.is_empty() || login.chars().count() > MAX_EMAIL {
        return Err(AppError::BadRequest("Enter your username or email.".into()));
    }
    // A blocked network: refused, as sign-in is.
    if let Some(block) = crate::ipblock::blocked(ip) {
        let uid: Option<i64> =
            sqlx::query_scalar("SELECT id FROM users WHERE username = $1 AND NOT is_bot")
                .bind(&login)
                .fetch_optional(&st.db)
                .await?;
        let allowed = match uid {
            Some(id) => crate::ipblock::exempt(&st.db, id).await,
            None => false,
        };
        if !allowed {
            crate::ipblock::record_attempt(&st, block, ip, "reset", uid, &login).await;
            return Err(AppError::Refused(crate::ipblock::USE_REFUSED.into()));
        }
    }
    let found: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT id, username, display_name, email FROM users
         WHERE NOT is_bot AND email_verified_at IS NOT NULL AND email IS NOT NULL
           AND (username = $1 OR email = $1)
         ORDER BY (username = $1) DESC LIMIT 1",
    )
    .bind(&login)
    .fetch_optional(&st.db)
    .await?;
    let Some((user_id, username, display_name, email)) = found else {
        tracing::info!(%ip, "password reset asked for an account without a confirmed email");
        return Ok(neutral);
    };
    // Per account: quietly stop sending (the answer doesn't change).
    if limits && !security::limiter().allow(&format!("forgot-user:{user_id}"), 5, hour) {
        return Ok(neutral);
    }
    let token = new_link_token(&st.db, user_id, "reset", None, RESET_MINUTES, None).await?;
    if let Some(url) = mail::link(&format!("/reset?token={token}")) {
        mail::queue(mail::compose(
            &email,
            "Reset your Outhouse password",
            &[
                format!("Hi {display_name},"),
                format!("Someone (hopefully you) asked to reset the password for your Outhouse account, @{username}."),
                "The link works once, for 30 minutes. If you didn't ask, ignore this email: your password stays the same.".into(),
            ],
            Some(("Choose a new password", &url)),
            mail::FOOTER,
        ));
    }
    tracing::info!(%ip, user = %username, "password reset link sent");
    Ok(neutral)
}

#[derive(Deserialize)]
pub struct ResetBody {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub password: String,
    /// A two-factor code (or recovery code), when the account has it on.
    #[serde(default)]
    pub code: Option<String>,
}

/// `POST /api/auth/reset/check` `{token}`: is the link still good, and does
/// the page need to ask for a two-factor code too?
pub async fn reset_check(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<TokenBody>,
) -> AppResult<Json<Value>> {
    if security::get().rate_limits
        && !security::limiter().allow(
            &format!("reset-check:{ip}"),
            60,
            std::time::Duration::from_secs(3600),
        )
    {
        return Err(AppError::TooManyRequests(
            "Too many tries. Wait an hour.".into(),
        ));
    }
    let Some(link) = live_link(&st.db, req.token.trim(), "reset").await? else {
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    };
    let (username, two_factor): (String, bool) =
        sqlx::query_as("SELECT username, totp_secret IS NOT NULL FROM users WHERE id = $1")
            .bind(link.user_id)
            .fetch_one(&st.db)
            .await?;
    Ok(Json(
        json!({ "ok": true, "username": username, "two_factor": two_factor }),
    ))
}

/// `POST /api/auth/reset` `{token, password, code?}`: the new password.
/// Everyone signed in as them is signed out (trusted devices too).
pub async fn reset(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<ResetBody>,
) -> AppResult<Json<Value>> {
    let limits = security::get().rate_limits;
    let lim = security::limiter();
    if limits && lim.is_limited(&format!("reset-ip:{ip}"), 20, security::LOGIN_WINDOW) {
        return Err(AppError::TooManyRequests(
            "Too many tries. Wait 15 minutes and try again.".into(),
        ));
    }
    let token = req.token.trim().to_string();
    let Some(link) = live_link(&st.db, &token, "reset").await? else {
        if limits {
            lim.hit(&format!("reset-ip:{ip}"), security::LOGIN_WINDOW);
        }
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    };
    let user_id = link.user_id;
    if let Some(block) = crate::ipblock::blocked(ip) {
        if !crate::ipblock::exempt(&st.db, user_id).await {
            let name: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_one(&st.db)
                .await?;
            crate::ipblock::record_attempt(&st, block, ip, "reset", Some(user_id), &name).await;
            return Err(AppError::Refused(crate::ipblock::USE_REFUSED.into()));
        }
    }
    super::account::check_new_password(&req.password)?;
    // Two-factor on: the inbox alone isn't enough.
    let used_recovery = if super::twofactor::is_enabled(&st.db, user_id).await? {
        let attempts_key = format!("reset-2fa:{}", auth::token_hash(&token));
        if limits && lim.is_limited(&attempts_key, 5, std::time::Duration::from_secs(3600)) {
            sqlx::query("DELETE FROM email_tokens WHERE token_hash = $1")
                .bind(auth::token_hash(&token))
                .execute(&st.db)
                .await?;
            return Err(AppError::BadRequest(
                "Too many wrong codes. Ask for a new reset link.".into(),
            ));
        }
        let code = req.code.as_deref().unwrap_or("").trim().to_string();
        if code.is_empty() {
            return Err(AppError::BadRequest(
                "Enter the 6-digit code from your authenticator app (or a recovery code).".into(),
            ));
        }
        match super::twofactor::check_second_factor(&st, user_id, &code).await? {
            Some(factor) => factor == super::twofactor::Factor::Recovery,
            None => {
                if limits {
                    lim.hit(&attempts_key, std::time::Duration::from_secs(3600));
                    lim.hit(&format!("reset-ip:{ip}"), security::LOGIN_WINDOW);
                }
                return Err(AppError::BadRequest(
                    "That code isn't right. Codes change every 30 seconds.".into(),
                ));
            }
        }
    } else {
        false
    };
    // Use the link (once only, even if it's sent twice at the same moment).
    let used = sqlx::query(
        "UPDATE email_tokens SET used_at = now()
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(auth::token_hash(&token))
    .execute(&st.db)
    .await?
    .rows_affected();
    if used == 0 {
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    }
    let hash = auth::hash_password(req.password).await?;
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(user_id)
        .bind(&hash)
        .execute(&st.db)
        .await?;
    revoke_everything(&st.db, user_id, None).await?;
    let how = if link.created_by.is_some() {
        "with a reset link the site owner made"
    } else {
        "with a reset link sent by email"
    };
    super::safety::log_action(
        &st,
        Some(user_id),
        "password_reset",
        None,
        Some(user_id),
        format!(
            "Password reset {how}; signed out everywhere{}",
            if used_recovery {
                " (a recovery code was used)"
            } else {
                ""
            }
        ),
    )
    .await;
    let w = who(&st.db, user_id).await?;
    password_changed_notice(&st, user_id, &w.username, &w.display_name, true).await;
    tracing::info!(%ip, user = %w.username, "password reset");
    let suspended = super::safety::suspension_message(&st.db, user_id)
        .await?
        .is_some();
    Ok(Json(json!({
        "ok": true,
        "username": w.username,
        // A reset doesn't lift a suspension.
        "suspended": suspended,
    })))
}

/// "Your password was changed" to their confirmed address.
pub async fn password_changed_notice(
    st: &AppState,
    user_id: i64,
    username: &str,
    display_name: &str,
    reset: bool,
) {
    notify(
        &st.db,
        user_id,
        "Your Outhouse password was changed",
        vec![
            format!("Hi {display_name},"),
            format!(
                "The password for your Outhouse account, @{username}, was just {}. Every device signed in as you has been signed out{}.",
                if reset { "reset" } else { "changed" },
                if reset { "" } else { " (except the one that changed it)" }
            ),
            "If this was you, there's nothing to do. If it wasn't, reset your password straight away with \"Forgot password?\" on the sign-in screen and tell the server owner.".into(),
        ],
    )
    .await;
}

// ---------- the site owner ----------

/// `POST /api/safety/users/{id}/reset-link`: a one-time reset link (24
/// hours) for someone who lost their email. Shown once; logged.
pub async fn owner_reset_link(
    State(st): State<AppState>,
    AuthUser(me): AuthUser,
    Path(target): Path<i64>,
) -> AppResult<Json<Value>> {
    super::safety::require_owner(&st, &me).await?;
    let bot: Option<bool> = sqlx::query_scalar("SELECT is_bot FROM users WHERE id = $1")
        .bind(target)
        .fetch_optional(&st.db)
        .await?;
    match bot {
        None => return Err(AppError::NotFound),
        Some(true) => return Err(AppError::BadRequest("Bots don't have passwords.".into())),
        Some(false) => {}
    }
    let token = new_link_token(
        &st.db,
        target,
        "reset",
        None,
        OWNER_RESET_HOURS * 60,
        Some(me.id),
    )
    .await?;
    let expires_at: DateTime<Utc> = Utc::now() + chrono::Duration::hours(OWNER_RESET_HOURS as i64);
    super::safety::log_action(
        &st,
        Some(me.id),
        "reset_link",
        None,
        Some(target),
        "Made a one-time password reset link (works for 24 hours; any older link stopped working)",
    )
    .await;
    Ok(Json(json!({
        "path": format!("/reset?token={token}"),
        "url": mail::link(&format!("/reset?token={token}")),
        "expires_at": expires_at,
        "two_factor": super::twofactor::is_enabled(&st.db, target).await?,
    })))
}

#[derive(Deserialize)]
pub struct TestEmail {
    #[serde(default)]
    pub to: Option<String>,
}

/// `POST /api/safety/email-test` `{to?}`: sends a test email now (to the
/// owner's own address unless another is given) and says what happened.
pub async fn email_test(
    State(st): State<AppState>,
    AuthUser(me): AuthUser,
    headers: HeaderMap,
    Json(req): Json<TestEmail>,
) -> AppResult<Json<Value>> {
    let _ = headers;
    super::safety::require_owner(&st, &me).await?;
    if !mail::configured() {
        return Err(AppError::BadRequest(
            "Email isn't set up yet: see Server status for what's missing.".into(),
        ));
    }
    let to = match req.to.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => normalize_email(t)?,
        None => sqlx::query_scalar::<_, Option<String>>("SELECT email FROM users WHERE id = $1")
            .bind(me.id)
            .fetch_one(&st.db)
            .await?
            .ok_or_else(|| {
                AppError::BadRequest("Add your own email first, or type an address.".into())
            })?,
    };
    if security::get().rate_limits
        && !security::limiter().allow(
            &format!("email-test:{}", me.id),
            10,
            std::time::Duration::from_secs(3600),
        )
    {
        return Err(AppError::TooManyRequests(
            "Enough test emails for now.".into(),
        ));
    }
    let site = mail::link("").unwrap_or_default();
    let result = mail::send_now(mail::compose(
        &to,
        "Outhouse test email",
        &[
            "It works! This is a test email from your Outhouse server.".into(),
            format!("Confirmation links and password resets will come from this address and link to {site}."),
            "If this landed in spam, check the SPF and DKIM records for your sending domain (deploy/README.md, \"Email\").".into(),
        ],
        None,
        "Sent from Outhouse settings → Server status.",
    ))
    .await;
    match result {
        Ok(()) => Ok(Json(json!({ "ok": true, "to": to }))),
        Err(e) => Err(AppError::BadRequest(format!("It didn't send: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_addresses() {
        assert_eq!(
            normalize_email("  Ann.B+oh@Example.COM ").unwrap(),
            "ann.b+oh@example.com"
        );
        assert!(normalize_email("a@b.co").is_ok());
        assert!(normalize_email("anne@sub.mail.example.org").is_ok());
        for bad in [
            "",
            "plain",
            "@example.com",
            "a@",
            "a@b",
            "a@b.c",
            "a b@example.com",
            "a@exa mple.com",
            "a@-x.com",
            "a..b@example.com",
            ".a@example.com",
            "a@example..com",
            "<a@example.com>",
            "a@exa\nmple.com",
            "a\"b@example.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?} should be refused");
        }
        let long = format!("{}@example.com", "a".repeat(250));
        assert!(normalize_email(&long).is_err());
    }

    #[test]
    fn masks() {
        assert_eq!(mask_email("johnny@example.com"), "jo•••@example.com");
        assert_eq!(mask_email("j@x.io"), "j•••@x.io");
    }

    #[test]
    fn tokens_are_long_and_hashed() {
        let t = auth::new_token();
        assert!(token_ok(&t));
        assert_eq!(t.len(), 64); // 256 bits
        assert_ne!(auth::token_hash(&t), t);
        assert_eq!(auth::token_hash(&t).len(), 64);
        assert!(!token_ok("abc"));
        assert!(!token_ok(&"z".repeat(64)));
    }
}
