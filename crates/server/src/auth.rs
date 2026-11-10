//! Passwords, login tokens and the `AuthUser` extractor.

use anyhow::anyhow;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use outhouse_common::models::User;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

const SESSION_DAYS: i32 = 30;

/// Columns for a `User`, from `users u`.
pub const USER_COLUMNS: &str = "u.id, u.username, u.display_name,
    CASE WHEN u.avatar_key IS NULL THEN NULL ELSE '/files/' || u.avatar_key || '/avatar' END AS avatar_url,
    u.is_bot AS bot";

/// SQL for a user's avatar URL from `users u` (NULL if none).
pub const AVATAR_URL_SQL: &str =
    "CASE WHEN u.avatar_key IS NULL THEN NULL ELSE '/files/' || u.avatar_key || '/avatar' END";

pub async fn fetch_user(db: &PgPool, user_id: i64) -> AppResult<User> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users u WHERE u.id = $1");
    sqlx::query_as::<_, User>(&sql)
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

/// A new random token (64 hex characters).
pub fn new_token() -> String {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    hex(&raw)
}

/// Hashes a password with Argon2id. Runs off the async threads because
/// hashing is deliberately slow.
pub async fn hash_password(password: String) -> AppResult<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| anyhow!("hashing password: {e}"))
    })
    .await
    .map_err(|e| anyhow!(e))?
    .map_err(AppError::from)
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A random string of `len` characters from `alphabet`.
pub fn random_code(len: usize, alphabet: &[u8]) -> String {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf.iter()
        .map(|b| alphabet[*b as usize % alphabet.len()] as char)
        .collect()
}

pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Creates a login session and returns the token to give the client.
pub async fn create_session(db: &PgPool, user_id: i64) -> AppResult<String> {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let token = hex(&raw);
    // Tidy up this account's expired sign-ins while we're here.
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND expires_at <= now()")
        .bind(user_id)
        .execute(db)
        .await?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at)
         VALUES ($1, $2, now() + make_interval(days => $3))",
    )
    .bind(token_hash(&token))
    .bind(user_id)
    .bind(SESSION_DAYS)
    .execute(db)
    .await?;
    Ok(token)
}

pub async fn delete_session(db: &PgPool, token: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(token_hash(token))
        .execute(db)
        .await?;
    Ok(())
}

/// Signs a user out everywhere except the session making the request.
pub async fn delete_other_sessions(db: &PgPool, user_id: i64, keep_token: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_hash <> $2")
        .bind(user_id)
        .bind(token_hash(keep_token))
        .execute(db)
        .await?;
    Ok(())
}

/// The user for a login token or a bot token. Suspended accounts (and bots
/// whose owner is suspended) get nothing.
pub async fn user_for_token(db: &PgPool, token: &str) -> AppResult<Option<User>> {
    let token = token.strip_prefix("Bot ").unwrap_or(token).trim();
    let sql = format!(
        "SELECT {USER_COLUMNS} FROM users u
         WHERE u.id = (
             SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()
             UNION ALL
             SELECT user_id FROM bots WHERE token_hash = $1
             LIMIT 1)
           AND (u.suspended_until IS NULL OR u.suspended_until <= now())
           AND NOT EXISTS (SELECT 1 FROM bots b JOIN users o ON o.id = b.owner_id
                           WHERE b.user_id = u.id AND o.suspended_until > now())"
    );
    let user = sqlx::query_as::<_, User>(&sql)
        .bind(token_hash(token))
        .fetch_optional(db)
        .await?;
    Ok(user)
}

/// The token from `Authorization: Bearer <token>` (people) or
/// `Authorization: Bot <token>` (bots).
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("Bot "))
        .map(str::trim)
}

/// Creates a user that webhooks and feeds post as. It can't sign in: its
/// password hash isn't a valid hash, so every login attempt fails.
pub async fn create_bot_user<'e, E>(db: E, id: i64, display_name: &str) -> AppResult<()>
where
    E: sqlx::PgExecutor<'e>,
{
    let name: String = display_name.trim().chars().take(32).collect();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, is_bot)
         VALUES ($1, $2, $3, '!', true)",
    )
    .bind(id)
    .bind(format!("bot.{id}"))
    .bind(if name.is_empty() {
        "Bot".to_string()
    } else {
        name
    })
    .execute(db)
    .await?;
    Ok(())
}

/// Extracts the signed-in user from `Authorization: Bearer <token>`.
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let token = bearer_token(&parts.headers).ok_or(AppError::Unauthorized)?;
        let user = user_for_token(&state.db, token)
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(AuthUser(user))
    }
}
