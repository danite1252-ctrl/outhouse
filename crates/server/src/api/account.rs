//! Sign up, sign in, sign out, and changing your own account.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{AuthResponse, LoginRequest, RegisterRequest, UpdateMeRequest, User};

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::security::{self, ClientIp, Registration};
use crate::state::{AppState, Audience};

/// Longer passwords are refused (hashing megabytes of "password" is just a
/// way to make the server work hard).
pub const MAX_PASSWORD_CHARS: usize = 256;

fn check_new_password(password: &str) -> AppResult<()> {
    let n = password.chars().count();
    if n < 8 {
        return Err(AppError::BadRequest(
            "Passwords need at least 8 characters.".into(),
        ));
    }
    if n > MAX_PASSWORD_CHARS {
        return Err(AppError::BadRequest(format!(
            "Passwords can be up to {MAX_PASSWORD_CHARS} characters."
        )));
    }
    Ok(())
}

/// `GET /api/auth/registration`: how sign-ups work here (`open`, `invite`
/// or `closed`), so the sign-in page can say so.
pub async fn registration_mode() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "mode": security::get().registration.as_str() }))
}

/// Refuses a sign-up when `REGISTRATION` says so. The first account on a
/// new server can always be made (someone has to own it).
async fn check_registration_allowed(st: &AppState, invite: Option<&str>) -> AppResult<()> {
    let mode = security::get().registration;
    if mode == Registration::Open {
        return Ok(());
    }
    let first: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM users WHERE NOT is_bot)")
        .fetch_one(&st.db)
        .await?;
    if first {
        return Ok(());
    }
    if mode == Registration::Invite {
        let code = invite.map(str::trim).filter(|c| {
            !c.is_empty() && c.len() <= 32 && c.chars().all(|c| c.is_ascii_alphanumeric())
        });
        if let Some(code) = code {
            let usable: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM invites WHERE code = $1
                     AND (expires_at IS NULL OR expires_at > now())
                     AND (max_uses IS NULL OR uses < max_uses))",
            )
            .bind(code)
            .fetch_one(&st.db)
            .await?;
            if usable {
                return Ok(());
            }
        }
        return Err(AppError::Refused(
            "New accounts need an invite here. Open the invite link you were sent, then sign up."
                .into(),
        ));
    }
    Err(AppError::Refused(
        "Sign-ups are closed on this server. Ask whoever runs it.".into(),
    ))
}

fn validate_username(name: &str) -> AppResult<()> {
    let ok_len = (2..=32).contains(&name.chars().count());
    let ok_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if ok_len && ok_chars {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "Usernames are 2–32 characters: letters, numbers, dots and underscores.".into(),
        ))
    }
}

pub async fn register(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<RegisterRequest>,
) -> AppResult<Json<AuthResponse>> {
    if security::get().rate_limits
        && !security::limiter().allow(
            &format!("signup:{ip}"),
            security::SIGNUPS_PER_IP,
            security::SIGNUP_WINDOW,
        )
    {
        return Err(AppError::TooManyRequests(
            "Lots of new accounts from here lately. Try again in an hour.".into(),
        ));
    }
    let username = req.username.trim().to_lowercase();
    validate_username(&username)?;
    check_new_password(&req.password)?;
    check_registration_allowed(&st, req.invite.as_deref()).await?;
    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(req.username.trim())
        .chars()
        .take(32)
        .collect::<String>();

    let hash = auth::hash_password(req.password).await?;
    let result = sqlx::query_as::<_, User>(
        "INSERT INTO users (id, username, display_name, password_hash)
         VALUES ($1, $2, $3, $4)
         RETURNING id, username, display_name",
    )
    .bind(st.ids.next_id())
    .bind(&username)
    .bind(&display_name)
    .bind(&hash)
    .fetch_one(&st.db)
    .await;

    let user = match result {
        Ok(user) => user,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(AppError::Conflict("That username is taken.".into()))
        }
        Err(e) => return Err(e.into()),
    };
    let token = auth::create_session(&st.db, user.id).await?;
    Ok(Json(AuthResponse { token, user }))
}

#[derive(sqlx::FromRow)]
struct LoginRow {
    id: i64,
    password_hash: String,
}

/// A hash to check passwords against when the username doesn't exist, so
/// "no such user" takes as long as "wrong password".
async fn dummy_hash() -> AppResult<&'static String> {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(hash) = DUMMY.get() {
        return Ok(hash);
    }
    let hash = auth::hash_password("not-a-real-password".into()).await?;
    Ok(DUMMY.get_or_init(|| hash))
}

pub async fn login(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    let username = req.username.trim().to_lowercase();
    // Brute-force protection: too many wrong passwords from one address, or
    // for one account, and sign-in pauses for a while (checked before the
    // password, so a locked account doesn't say whether a guess was right).
    let limits = security::get().rate_limits;
    let lim = security::limiter();
    let ip_key = format!("login-ip:{ip}");
    let account_key = format!(
        "login-user:{}",
        username.chars().take(64).collect::<String>()
    );
    if limits
        && (lim.is_limited(
            &ip_key,
            security::LOGIN_FAILS_PER_IP,
            security::LOGIN_WINDOW,
        ) || lim.is_limited(
            &account_key,
            security::LOGIN_FAILS_PER_ACCOUNT,
            security::LOGIN_WINDOW,
        ))
    {
        return Err(AppError::TooManyRequests(
            "Too many wrong passwords. Wait 15 minutes and try again.".into(),
        ));
    }
    let failed = || {
        if limits {
            lim.hit(&ip_key, security::LOGIN_WINDOW);
            lim.hit(&account_key, security::LOGIN_WINDOW);
        }
        AppError::InvalidCredentials
    };
    if req.password.chars().count() > MAX_PASSWORD_CHARS {
        return Err(failed());
    }
    let row =
        sqlx::query_as::<_, LoginRow>("SELECT id, password_hash FROM users WHERE username = $1")
            .bind(&username)
            .fetch_optional(&st.db)
            .await?;
    let Some(row) = row else {
        auth::verify_password(req.password, dummy_hash().await?.clone()).await;
        return Err(failed());
    };

    if !auth::verify_password(req.password, row.password_hash).await {
        tracing::info!(%ip, user = %username, "failed sign-in");
        return Err(failed());
    }
    let user = auth::fetch_user(&st.db, row.id).await?;
    let token = auth::create_session(&st.db, user.id).await?;
    Ok(Json(AuthResponse { token, user }))
}

pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> AppResult<StatusCode> {
    if let Some(token) = auth::bearer_token(&headers) {
        auth::delete_session(&st.db, token).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn me(AuthUser(user): AuthUser) -> Json<User> {
    Json(user)
}

/// Tidies a display name or nickname: trimmed, at most 32 characters.
pub fn clean_name(name: &str) -> String {
    name.trim().chars().take(32).collect()
}

pub async fn update_me(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(req): Json<UpdateMeRequest>,
) -> AppResult<Json<User>> {
    if let Some(new_password) = req.new_password {
        check_new_password(&new_password)?;
        let current = req.current_password.unwrap_or_default();
        let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&st.db)
            .await?;
        if !auth::verify_password(current, hash).await {
            return Err(AppError::BadRequest(
                "Your current password isn't right.".into(),
            ));
        }
        let new_hash = auth::hash_password(new_password).await?;
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(user.id)
            .bind(&new_hash)
            .execute(&st.db)
            .await?;
        // Anyone else signed in as you gets signed out.
        if let Some(token) = auth::bearer_token(&headers) {
            auth::delete_other_sessions(&st.db, user.id, token).await?;
        }
    }

    let mut user = user;
    let mut changed = false;
    if let Some(about) = req.about_me {
        let about: String = about.trim().chars().take(190).collect();
        sqlx::query("UPDATE users SET about_me = $2 WHERE id = $1")
            .bind(user.id)
            .bind(&about)
            .execute(&st.db)
            .await?;
    }
    if let Some(theme) = req.profile_theme {
        if !theme.is_empty() && !PROFILE_THEMES.contains(&theme.as_str()) {
            return Err(AppError::BadRequest("Unknown profile background.".into()));
        }
        sqlx::query("UPDATE users SET profile_theme = $2 WHERE id = $1")
            .bind(user.id)
            .bind(&theme)
            .execute(&st.db)
            .await?;
    }
    if let Some(color) = req.accent_color {
        sqlx::query("UPDATE users SET accent_color = $2 WHERE id = $1")
            .bind(user.id)
            .bind(color.clamp(0, 0xFF_FFFF))
            .execute(&st.db)
            .await?;
    }
    if let Some(name) = req.display_name {
        let name = clean_name(&name);
        if name.is_empty() {
            return Err(AppError::BadRequest("Your name can't be empty.".into()));
        }
        sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
            .bind(user.id)
            .bind(&name)
            .execute(&st.db)
            .await?;
        user.display_name = name;
        changed = true;
    }
    if changed {
        refresh_my_guilds(&st, user.id).await?;
    }
    Ok(Json(user))
}

/// The built-in profile backgrounds (the web client draws them).
pub const PROFILE_THEMES: &[&str] = &[
    "ember",
    "dusk",
    "moonwell",
    "felfire",
    "frostfall",
    "verdant",
    "stormwind",
    "abyss",
    "goldleaf",
    "outhouse",
    // Animated ones.
    "emberdrift",
    "starfield",
    "aurora",
];

/// Tells every outhouse you're in to re-fetch members (name or picture changed).
pub async fn refresh_my_guilds(st: &AppState, user_id: i64) -> AppResult<()> {
    let guilds: Vec<i64> =
        sqlx::query_scalar("SELECT guild_id FROM guild_members WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&st.db)
            .await?;
    for guild_id in guilds {
        st.dispatch(
            Audience::Guild(guild_id),
            ServerEvent::GuildRefresh { guild_id },
        );
    }
    Ok(())
}
