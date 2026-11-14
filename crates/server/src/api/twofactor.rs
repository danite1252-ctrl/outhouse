//! Two-factor sign-in (authenticator-app codes) for people: setting it up,
//! the code step when signing in, "trust this device", recovery codes,
//! turning it off, and the site owner removing it for someone who lost
//! their phone and their codes. Not required, but strongly suggested.
//! Bots, webhooks and feeds never use it (they don't sign in).
//!
//! Signing in with two-factor on: the password step answers
//! `{"mfa_required": true, "ticket": "…"}` instead of a session. The ticket
//! lives 5 minutes (it is not a session: it can only be swapped for one
//! with a right code at `POST /api/auth/login/2fa`), and allows 5 wrong
//! codes; an account allows 10 wrong codes per 15 minutes in all.
//!
//! "Trust this device for N days" is an httpOnly, SameSite=Strict cookie
//! (Secure on https) for `/api/auth/` only, one per account, so the web
//! client, the Windows app and the Android app (all the same pages in a
//! webview, which keeps cookies) skip the code step there. Changing or
//! resetting the password, or turning two-factor off, forgets every one.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::header::{COOKIE, SET_COOKIE, USER_AGENT};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::models::AuthResponse;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::security::{self, ClientIp};
use crate::state::AppState;
use crate::twofa;

pub const TICKET_MINUTES: i32 = 5;
pub const TICKET_ATTEMPTS: i32 = 5;
pub const ACCOUNT_FAILS: u32 = 10;
pub const TRUST_DAYS: &[i32] = &[7, 30, 90];
const COOKIE_PREFIX: &str = "oh_td_";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Factor {
    Totp,
    Recovery,
}

pub async fn is_enabled(db: &PgPool, user_id: i64) -> AppResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT totp_secret IS NOT NULL FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?
            .unwrap_or(false),
    )
}

/// Checks a 6-digit code (each 30-second code works once) or a recovery
/// code (each works once). Some(which) when it's right.
pub async fn check_second_factor(
    st: &AppState,
    user_id: i64,
    code: &str,
) -> AppResult<Option<Factor>> {
    if twofa::looks_like_recovery(code) {
        let used = sqlx::query(
            "UPDATE recovery_codes SET used_at = now()
             WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL",
        )
        .bind(user_id)
        .bind(twofa::recovery_hash(code))
        .execute(&st.db)
        .await?
        .rows_affected();
        if used == 0 {
            return Ok(None);
        }
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM recovery_codes WHERE user_id = $1 AND used_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
        super::safety::log_action(
            st,
            Some(user_id),
            "recovery_code_used",
            None,
            Some(user_id),
            format!("Used a two-factor recovery code ({left} left)"),
        )
        .await;
        return Ok(Some(Factor::Recovery));
    }
    let row: Option<(Option<String>, i64)> =
        sqlx::query_as("SELECT totp_secret, totp_last_step FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&st.db)
            .await?;
    let Some((Some(sealed), last_step)) = row else {
        return Ok(None);
    };
    let Some(secret) = twofa::decrypt(user_id, &sealed) else {
        tracing::error!(
            user_id,
            "couldn't decrypt a two-factor secret (did AUTH_SECRET_KEY change?)"
        );
        return Ok(None);
    };
    let Some(step) = twofa::check_code(&secret, code, twofa::now_unix(), last_step) else {
        return Ok(None);
    };
    // Remember the step (a second use of the same code, even at the same
    // moment, finds it taken).
    let taken =
        sqlx::query("UPDATE users SET totp_last_step = $2 WHERE id = $1 AND totp_last_step < $2")
            .bind(user_id)
            .bind(step)
            .execute(&st.db)
            .await?
            .rows_affected();
    Ok((taken == 1).then_some(Factor::Totp))
}

fn account_key(user_id: i64) -> String {
    format!("2fa-user:{user_id}")
}

fn too_many_codes() -> AppError {
    AppError::TooManyRequests("Too many wrong codes. Wait 15 minutes and try again.".into())
}

/// Checks a code for an action on your own account (turning two-factor
/// off, new recovery codes), with the same per-account limit as sign-in.
async fn require_code(st: &AppState, user_id: i64, code: &str) -> AppResult<Factor> {
    let limits = security::get().rate_limits;
    let lim = security::limiter();
    if limits && lim.is_limited(&account_key(user_id), ACCOUNT_FAILS, security::LOGIN_WINDOW) {
        return Err(too_many_codes());
    }
    match check_second_factor(st, user_id, code).await? {
        Some(f) => Ok(f),
        None => {
            if limits {
                lim.hit(&account_key(user_id), security::LOGIN_WINDOW);
            }
            Err(AppError::BadRequest(
                "That code isn't right. Codes change every 30 seconds, and each works once.".into(),
            ))
        }
    }
}

async fn require_password(db: &PgPool, user_id: i64, password: &str) -> AppResult<()> {
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(db)
        .await?;
    if password.chars().count() > super::account::MAX_PASSWORD_CHARS
        || !auth::verify_password(password.to_string(), hash).await
    {
        return Err(AppError::BadRequest("Your password isn't right.".into()));
    }
    Ok(())
}

async fn store_recovery_codes(db: &PgPool, user_id: i64) -> AppResult<Vec<String>> {
    let codes = twofa::new_recovery_codes();
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for c in &codes {
        sqlx::query("INSERT INTO recovery_codes (user_id, code_hash) VALUES ($1, $2)")
            .bind(user_id)
            .bind(twofa::recovery_hash(c))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(codes)
}

// ---------- trusted devices ----------

fn cookie_name(user_id: i64) -> String {
    format!("{COOKIE_PREFIX}{user_id}")
}

fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim().to_string())
}

/// Secure cookies unless the site is plain http (a PC or the tests).
fn secure(st: &AppState, headers: &HeaderMap) -> bool {
    match st.config.public_url.as_deref() {
        Some(u) => u.starts_with("https://"),
        None => headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("https")),
    }
}

fn trust_cookie(
    st: &AppState,
    headers: &HeaderMap,
    user_id: i64,
    token: &str,
    days: i32,
) -> String {
    format!(
        "{}={token}; Path=/api/auth/; Max-Age={}; HttpOnly; SameSite=Strict{}",
        cookie_name(user_id),
        i64::from(days) * 86400,
        if secure(st, headers) { "; Secure" } else { "" }
    )
}

fn clear_cookie(st: &AppState, headers: &HeaderMap, user_id: i64) -> String {
    format!(
        "{}=; Path=/api/auth/; Max-Age=0; HttpOnly; SameSite=Strict{}",
        cookie_name(user_id),
        if secure(st, headers) { "; Secure" } else { "" }
    )
}

/// Is this device trusted for this account (a live cookie)? Notes the use.
pub async fn device_trusted(db: &PgPool, headers: &HeaderMap, user_id: i64) -> AppResult<bool> {
    let Some(token) = read_cookie(headers, &cookie_name(user_id)) else {
        return Ok(false);
    };
    if token.len() != 64 {
        return Ok(false);
    }
    let n = sqlx::query(
        "UPDATE trusted_devices SET last_used_at = now()
         WHERE token_hash = $1 AND user_id = $2 AND expires_at > now()",
    )
    .bind(auth::token_hash(&token))
    .bind(user_id)
    .execute(db)
    .await?
    .rows_affected();
    Ok(n == 1)
}

/// "Chrome on Windows", "Outhouse app on Android", ... for the device list.
pub fn device_label(headers: &HeaderMap) -> String {
    let ua = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let os = if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("CrOS") {
        "ChromeOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "an unknown system"
    };
    let browser = if ua.contains("Outhouse") || ua.contains("; wv)") {
        "Outhouse app"
    } else if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("OPR/") {
        "Opera"
    } else if ua.contains("Chrome/") || ua.contains("HeadlessChrome") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "A browser"
    };
    format!("{browser} on {os}")
}

// ---------- signing in ----------

/// After a right password: a ticket for the code step when two-factor is on
/// and this device isn't trusted; None to sign straight in.
pub async fn ticket_if_needed(
    st: &AppState,
    headers: &HeaderMap,
    user_id: i64,
) -> AppResult<Option<String>> {
    if !is_enabled(&st.db, user_id).await? || device_trusted(&st.db, headers, user_id).await? {
        return Ok(None);
    }
    let ticket = auth::new_token();
    sqlx::query("DELETE FROM pending_logins WHERE expires_at <= now()")
        .execute(&st.db)
        .await?;
    sqlx::query(
        "INSERT INTO pending_logins (token_hash, user_id, expires_at)
         VALUES ($1, $2, now() + make_interval(mins => $3))",
    )
    .bind(auth::token_hash(&ticket))
    .bind(user_id)
    .bind(TICKET_MINUTES)
    .execute(&st.db)
    .await?;
    Ok(Some(ticket))
}

#[derive(Deserialize)]
pub struct CodeLogin {
    #[serde(default)]
    pub ticket: String,
    /// The 6-digit code, or a recovery code.
    #[serde(default)]
    pub code: String,
    /// 0 = don't trust this device; 7, 30 or 90 days.
    #[serde(default)]
    pub trust_days: Option<i32>,
}

/// `POST /api/auth/login/2fa` `{ticket, code, trust_days}`: the code step.
pub async fn login_code(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(req): Json<CodeLogin>,
) -> AppResult<Response> {
    let expired = || AppError::BadRequest("That took too long. Sign in again.".into());
    if req.ticket.len() != 64 {
        return Err(expired());
    }
    let ticket_hash = auth::token_hash(&req.ticket);
    let row: Option<(i64, i32)> = sqlx::query_as(
        "SELECT user_id, attempts FROM pending_logins WHERE token_hash = $1 AND expires_at > now()",
    )
    .bind(&ticket_hash)
    .fetch_optional(&st.db)
    .await?;
    let Some((user_id, attempts)) = row else {
        return Err(expired());
    };
    let limits = security::get().rate_limits;
    let lim = security::limiter();
    if limits && lim.is_limited(&account_key(user_id), ACCOUNT_FAILS, security::LOGIN_WINDOW) {
        return Err(too_many_codes());
    }
    if attempts >= TICKET_ATTEMPTS {
        sqlx::query("DELETE FROM pending_logins WHERE token_hash = $1")
            .bind(&ticket_hash)
            .execute(&st.db)
            .await?;
        return Err(AppError::BadRequest(
            "Too many wrong codes. Sign in again.".into(),
        ));
    }
    // The same checks as the password step, again (things change in 5 minutes).
    if let Some(block) = crate::ipblock::blocked(ip) {
        if !crate::ipblock::exempt(&st.db, user_id).await {
            let name: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_one(&st.db)
                .await?;
            crate::ipblock::record_attempt(&st, block, ip, "login", Some(user_id), &name).await;
            return Err(AppError::Refused(crate::ipblock::USE_REFUSED.into()));
        }
    }
    if let Some(why) = super::safety::suspension_message(&st.db, user_id).await? {
        return Err(AppError::Refused(why));
    }
    if check_second_factor(&st, user_id, &req.code)
        .await?
        .is_none()
    {
        sqlx::query("UPDATE pending_logins SET attempts = attempts + 1 WHERE token_hash = $1")
            .bind(&ticket_hash)
            .execute(&st.db)
            .await?;
        if limits {
            lim.hit(&account_key(user_id), security::LOGIN_WINDOW);
        }
        tracing::info!(%ip, user_id, "wrong two-factor code");
        let left = TICKET_ATTEMPTS - attempts - 1;
        return Err(AppError::BadRequest(if left > 0 {
            "That code isn't right. Codes change every 30 seconds, and each works once.".into()
        } else {
            "That code isn't right either. Sign in again to get more tries.".into()
        }));
    }
    let gone = sqlx::query("DELETE FROM pending_logins WHERE token_hash = $1")
        .bind(&ticket_hash)
        .execute(&st.db)
        .await?
        .rows_affected();
    if gone == 0 {
        return Err(expired());
    }
    let user = auth::fetch_user(&st.db, user_id).await?;
    let token = auth::create_session(&st.db, user.id).await?;
    crate::ips::signed_in(&st, user.id, &token, ip, crate::ips::Kind::Login).await;
    let mut res = Json(AuthResponse { token, user }).into_response();
    if let Some(days) = req.trust_days.filter(|d| TRUST_DAYS.contains(d)) {
        let device = auth::new_token();
        sqlx::query(
            "INSERT INTO trusted_devices (id, user_id, token_hash, label, expires_at)
             VALUES ($1, $2, $3, $4, now() + make_interval(days => $5))",
        )
        .bind(st.ids.next_id())
        .bind(user_id)
        .bind(auth::token_hash(&device))
        .bind(device_label(&headers))
        .bind(days)
        .execute(&st.db)
        .await?;
        if let Ok(v) = HeaderValue::from_str(&trust_cookie(&st, &headers, user_id, &device, days)) {
            res.headers_mut().append(SET_COOKIE, v);
        }
    }
    Ok(res)
}

// ---------- your own settings ----------

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: i64,
    label: String,
    created_at: DateTime<Utc>,
    last_used_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    token_hash: String,
}

/// `GET /api/auth/2fa`: on or off, recovery codes left, trusted devices.
pub async fn status(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    let (enabled_at, on): (Option<DateTime<Utc>>, bool) =
        sqlx::query_as("SELECT totp_enabled_at, totp_secret IS NOT NULL FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&st.db)
            .await?;
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    let devices = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, label, created_at, last_used_at, expires_at, token_hash FROM trusted_devices
         WHERE user_id = $1 AND expires_at > now() ORDER BY last_used_at DESC",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let here = read_cookie(&headers, &cookie_name(user.id)).map(|t| auth::token_hash(&t));
    Ok(Json(json!({
        "enabled": on,
        "enabled_at": enabled_at,
        "recovery_codes_left": if on { left } else { 0 },
        "available": twofa::key_source() != twofa::KeySource::Missing,
        "trust_days": TRUST_DAYS,
        "trusted_devices": devices.iter().map(|d| json!({
            "id": d.id.to_string(),
            "label": d.label,
            "created_at": d.created_at,
            "last_used_at": d.last_used_at,
            "expires_at": d.expires_at,
            "current": here.as_deref() == Some(d.token_hash.as_str()),
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct PasswordBody {
    #[serde(default)]
    pub password: String,
}

/// `POST /api/auth/2fa/setup` `{password}`: a new secret to scan (QR code
/// and the text for typing it in). Nothing changes until it's confirmed.
pub async fn setup(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<PasswordBody>,
) -> AppResult<Json<Value>> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    if security::get().rate_limits
        && !security::limiter().allow(
            &format!("2fa-setup:{}", user.id),
            10,
            security::LOGIN_WINDOW,
        )
    {
        return Err(AppError::TooManyRequests(
            "Too many tries. Wait 15 minutes.".into(),
        ));
    }
    require_password(&st.db, user.id, &req.password).await?;
    if is_enabled(&st.db, user.id).await? {
        return Err(AppError::BadRequest("Two-factor is already on.".into()));
    }
    let secret = twofa::new_secret();
    let sealed = twofa::encrypt(user.id, &secret).ok_or_else(|| {
        AppError::BadRequest(
            "Two-factor can't be turned on until the server owner sets AUTH_SECRET_KEY.".into(),
        )
    })?;
    sqlx::query("UPDATE users SET totp_pending = $2 WHERE id = $1")
        .bind(user.id)
        .bind(&sealed)
        .execute(&st.db)
        .await?;
    let text = twofa::base32_encode(&secret);
    let uri = twofa::otpauth_uri(&user.username, &text);
    Ok(Json(json!({
        "secret": text,
        // Grouped in fours for typing in by hand.
        "secret_grouped": text.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).to_string()).collect::<Vec<_>>().join(" "),
        "uri": uri,
        "qr_svg": twofa::qr_svg(&uri),
        "issuer": twofa::ISSUER,
        "account": user.username,
    })))
}

#[derive(Deserialize)]
pub struct CodeBody {
    #[serde(default)]
    pub code: String,
}

/// `POST /api/auth/2fa/confirm` `{code}`: the first code from the app turns
/// it on, and the 10 recovery codes are shown (this once).
pub async fn confirm(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(req): Json<CodeBody>,
) -> AppResult<Json<Value>> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    let limits = security::get().rate_limits;
    let lim = security::limiter();
    if limits && lim.is_limited(&account_key(user.id), ACCOUNT_FAILS, security::LOGIN_WINDOW) {
        return Err(too_many_codes());
    }
    let pending: Option<String> =
        sqlx::query_scalar("SELECT totp_pending FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&st.db)
            .await?;
    let Some(sealed) = pending else {
        return Err(AppError::BadRequest("Start setting it up first.".into()));
    };
    let secret = twofa::decrypt(user.id, &sealed)
        .ok_or_else(|| AppError::BadRequest("Start setting it up again.".into()))?;
    let Some(step) = twofa::check_code(&secret, &req.code, twofa::now_unix(), 0) else {
        if limits {
            lim.hit(&account_key(user.id), security::LOGIN_WINDOW);
        }
        return Err(AppError::BadRequest(
            "That code isn't right. Check the time on your phone is set automatically, and try the newest code.".into(),
        ));
    };
    let changed = sqlx::query(
        "UPDATE users SET totp_secret = totp_pending, totp_pending = NULL, totp_enabled_at = now(),
                totp_last_step = $2
         WHERE id = $1 AND totp_pending = $3 AND totp_secret IS NULL",
    )
    .bind(user.id)
    .bind(step)
    .bind(&sealed)
    .execute(&st.db)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(AppError::BadRequest("Two-factor is already on.".into()));
    }
    let codes = store_recovery_codes(&st.db, user.id).await?;
    super::safety::log_action(
        &st,
        Some(user.id),
        "2fa_enabled",
        None,
        Some(user.id),
        "Turned on two-factor sign-in (authenticator app)",
    )
    .await;
    let w: (String, String) =
        sqlx::query_as("SELECT username, display_name FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&st.db)
            .await?;
    super::recovery::notify(
        &st.db,
        user.id,
        "Two-factor sign-in is on",
        vec![
            format!("Hi {},", w.1),
            format!("Two-factor sign-in is now on for @{}: signing in needs your password and a code from your authenticator app.", w.0),
            "Keep your recovery codes somewhere safe. If this wasn't you, reset your password straight away and tell the server owner.".into(),
        ],
    )
    .await;
    let _ = headers;
    Ok(Json(json!({ "enabled": true, "recovery_codes": codes })))
}

/// `POST /api/auth/2fa/recovery-codes` `{code}`: 10 new recovery codes
/// (the old ones stop working).
pub async fn new_recovery_codes(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<CodeBody>,
) -> AppResult<Json<Value>> {
    if user.bot || !is_enabled(&st.db, user.id).await? {
        return Err(AppError::BadRequest("Two-factor isn't on.".into()));
    }
    require_code(&st, user.id, &req.code).await?;
    let codes = store_recovery_codes(&st.db, user.id).await?;
    super::safety::log_action(
        &st,
        Some(user.id),
        "2fa_recovery_codes",
        None,
        Some(user.id),
        "Made new two-factor recovery codes (the old ones stopped working)",
    )
    .await;
    Ok(Json(json!({ "recovery_codes": codes })))
}

#[derive(Deserialize)]
pub struct DisableBody {
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub code: String,
}

async fn turn_off(db: &PgPool, user_id: i64) -> AppResult<()> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "UPDATE users SET totp_secret = NULL, totp_pending = NULL, totp_enabled_at = NULL WHERE id = $1",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    for sql in [
        "DELETE FROM recovery_codes WHERE user_id = $1",
        "DELETE FROM trusted_devices WHERE user_id = $1",
        "DELETE FROM pending_logins WHERE user_id = $1",
    ] {
        sqlx::query(sql).bind(user_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// `POST /api/auth/2fa/disable` `{password, code}`.
pub async fn disable(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(req): Json<DisableBody>,
) -> AppResult<Response> {
    if user.bot || !is_enabled(&st.db, user.id).await? {
        return Err(AppError::BadRequest("Two-factor isn't on.".into()));
    }
    require_password(&st.db, user.id, &req.password).await?;
    require_code(&st, user.id, &req.code).await?;
    turn_off(&st.db, user.id).await?;
    super::safety::log_action(
        &st,
        Some(user.id),
        "2fa_disabled",
        None,
        Some(user.id),
        "Turned off two-factor sign-in",
    )
    .await;
    super::recovery::notify(
        &st.db,
        user.id,
        "Two-factor sign-in was turned off",
        vec![
            format!("Two-factor sign-in was just turned off for @{}. Signing in now needs only your password.", user.username),
            "If this wasn't you, reset your password straight away and tell the server owner.".into(),
        ],
    )
    .await;
    let mut res = Json(json!({ "enabled": false })).into_response();
    if let Ok(v) = HeaderValue::from_str(&clear_cookie(&st, &headers, user.id)) {
        res.headers_mut().append(SET_COOKIE, v);
    }
    Ok(res)
}

/// `DELETE /api/auth/2fa/devices/{id}`: forget one trusted device.
pub async fn forget_device(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    let n = sqlx::query("DELETE FROM trusted_devices WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&st.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/auth/2fa/devices`: forget them all.
pub async fn forget_all_devices(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    let n = sqlx::query("DELETE FROM trusted_devices WHERE user_id = $1")
        .bind(user.id)
        .execute(&st.db)
        .await?
        .rows_affected();
    Ok(Json(json!({ "ok": true, "forgotten": n })))
}

// ---------- the site owner ----------

#[derive(Deserialize)]
pub struct RemoveBody {
    #[serde(default)]
    pub reason: String,
}

/// `POST /api/safety/users/{id}/remove-2fa` `{reason}`: for someone who lost
/// their phone and their recovery codes. Logged, and they get an email.
pub async fn owner_remove(
    State(st): State<AppState>,
    AuthUser(me): AuthUser,
    Path(target): Path<i64>,
    Json(req): Json<RemoveBody>,
) -> AppResult<Json<Value>> {
    super::safety::require_owner(&st, &me).await?;
    let reason: String = req.reason.trim().chars().take(300).collect();
    if reason.is_empty() {
        return Err(AppError::BadRequest(
            "Say why (it goes in the safety log and the email to them).".into(),
        ));
    }
    if !is_enabled(&st.db, target).await? {
        return Err(AppError::BadRequest(
            "They don't have two-factor on.".into(),
        ));
    }
    turn_off(&st.db, target).await?;
    super::safety::log_action(
        &st,
        Some(me.id),
        "2fa_reset",
        None,
        Some(target),
        format!("Removed their two-factor sign-in. Reason: {reason}"),
    )
    .await;
    let (username, display_name): (String, String) =
        sqlx::query_as("SELECT username, display_name FROM users WHERE id = $1")
            .bind(target)
            .fetch_one(&st.db)
            .await?;
    super::recovery::notify(
        &st.db,
        target,
        "Two-factor sign-in was removed from your account",
        vec![
            format!("Hi {display_name},"),
            format!("The site owner turned off two-factor sign-in for @{username}. Their note: \"{reason}\""),
            "You can sign in with just your password now. Please turn two-factor back on in Settings → My account. If you didn't ask for this, reset your password and contact the server owner.".into(),
        ],
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// For People (site owner): two-factor on, since when, codes left.
pub async fn owner_summary(db: &PgPool, user_id: i64) -> AppResult<Value> {
    let (enabled_at, on): (Option<DateTime<Utc>>, bool) =
        sqlx::query_as("SELECT totp_enabled_at, totp_secret IS NOT NULL FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(db)
            .await?;
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    let devices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM trusted_devices WHERE user_id = $1 AND expires_at > now()",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(
        json!({ "enabled": on, "enabled_at": enabled_at, "recovery_codes_left": left, "trusted_devices": devices }),
    )
}

/// Unused-ticket tidy-up interval for the background task.
pub const TIDY_EVERY: Duration = Duration::from_secs(3600);

/// Clears expired tickets, links and trusted devices once an hour.
pub fn spawn_tidier(st: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TIDY_EVERY).await;
            for sql in [
                "DELETE FROM pending_logins WHERE expires_at <= now()",
                "DELETE FROM trusted_devices WHERE expires_at <= now()",
                "DELETE FROM email_tokens WHERE expires_at <= now() - interval '1 day'",
            ] {
                if let Err(e) = sqlx::query(sql).execute(&st.db).await {
                    tracing::warn!(error = %e, "couldn't tidy sign-in tokens");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_are_read_by_name() {
        let mut h = HeaderMap::new();
        h.insert(COOKIE, "a=1; oh_td_42=abc; oh_td_4=zzz".parse().unwrap());
        assert_eq!(read_cookie(&h, "oh_td_42").as_deref(), Some("abc"));
        assert_eq!(read_cookie(&h, "oh_td_4").as_deref(), Some("zzz"));
        assert_eq!(read_cookie(&h, "oh_td_5"), None);
    }

    #[test]
    fn device_labels() {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36 Edg/130.0".parse().unwrap());
        assert_eq!(device_label(&h), "Edge on Windows");
        h.insert(USER_AGENT, "Mozilla/5.0 (Linux; Android 14; Pixel 8; wv) AppleWebKit/537.36 Chrome/130.0 Mobile Safari/537.36".parse().unwrap());
        assert_eq!(device_label(&h), "Outhouse app on Android");
        h.insert(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0"
                .parse()
                .unwrap(),
        );
        assert_eq!(device_label(&h), "Firefox on Linux");
    }
}
