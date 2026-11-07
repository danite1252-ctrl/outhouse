//! Sign up, sign in, sign out, and changing your own account.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{AuthResponse, LoginRequest, RegisterRequest, UpdateMeRequest, User};

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

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
    Json(req): Json<RegisterRequest>,
) -> AppResult<Json<AuthResponse>> {
    let username = req.username.trim().to_lowercase();
    validate_username(&username)?;
    if req.password.chars().count() < 8 {
        return Err(AppError::BadRequest(
            "Passwords need at least 8 characters.".into(),
        ));
    }
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

pub async fn login(
    State(st): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    let username = req.username.trim().to_lowercase();
    let row = sqlx::query_as::<_, LoginRow>(
        "SELECT id, password_hash FROM users WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::InvalidCredentials)?;

    if !auth::verify_password(req.password, row.password_hash).await {
        return Err(AppError::InvalidCredentials);
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
        if new_password.chars().count() < 8 {
            return Err(AppError::BadRequest(
                "Passwords need at least 8 characters.".into(),
            ));
        }
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
    "ember", "dusk", "moonwell", "felfire", "frostfall", "verdant", "stormwind", "abyss",
    "goldleaf", "outhouse",
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
