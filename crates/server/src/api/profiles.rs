//! Profiles: avatar, banner, "about me", accent colour and background.

use axum::extract::{Multipart, Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::models::{Profile, User};

use super::account::refresh_my_guilds;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const KEY_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const MAX_AVATAR_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_BANNER_BYTES: usize = 10 * 1024 * 1024;

#[derive(sqlx::FromRow)]
struct ProfileRow {
    id: i64,
    username: String,
    display_name: String,
    avatar_key: Option<String>,
    banner_key: Option<String>,
    about_me: String,
    profile_theme: String,
    accent_color: i32,
    is_bot: bool,
    created_at: DateTime<Utc>,
}

/// Someone's profile. You can see people you share an outhouse with.
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(user_id): Path<i64>,
) -> AppResult<Json<Profile>> {
    let row = sqlx::query_as::<_, ProfileRow>(
        "SELECT u.id, u.username, u.display_name, u.avatar_key, u.banner_key, u.about_me,
                u.profile_theme, u.accent_color, u.is_bot, u.created_at
         FROM users u
         WHERE u.id = $1
           AND (u.id = $2 OR EXISTS (
                SELECT 1 FROM guild_members a
                JOIN guild_members b ON a.guild_id = b.guild_id
                WHERE a.user_id = $1 AND b.user_id = $2))",
    )
    .bind(user_id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    // Suspended accounts' profiles are hidden (except from the site owner).
    if row.id != user.id
        && super::safety::hidden_profiles(&st.db, user.id, &[row.id])
            .await?
            .contains(&row.id)
    {
        return Err(AppError::NotFound);
    }
    Ok(Json(Profile {
        id: row.id,
        username: row.username,
        display_name: row.display_name,
        avatar_url: row.avatar_key.map(|k| format!("/files/{k}/avatar")),
        banner_url: row.banner_key.map(|k| format!("/files/{k}/banner")),
        about_me: row.about_me,
        profile_theme: row.profile_theme,
        accent_color: row.accent_color,
        bot: row.is_bot,
        created_at: row.created_at,
        characters: super::wow::characters_of(&st.db, row.id).await?,
    }))
}

fn image_type(content_type: &str, filename: &str) -> Option<&'static str> {
    let by_type = match content_type {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    };
    by_type.or_else(|| {
        let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            _ => None,
        }
    })
}

/// "avatar" or "banner" → the users column it lives in.
fn column(kind: &str) -> &'static str {
    if kind == "banner" {
        "banner_key"
    } else {
        "avatar_key"
    }
}

async fn replace_image(
    st: &AppState,
    user_id: i64,
    kind: &str,
    new_key: Option<String>,
) -> AppResult<()> {
    let col = column(kind);
    let old: Option<String> = sqlx::query_scalar(&format!("SELECT {col} FROM users WHERE id = $1"))
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    sqlx::query(&format!("UPDATE users SET {col} = $2 WHERE id = $1"))
        .bind(user_id)
        .bind(&new_key)
        .execute(&st.db)
        .await?;
    if let Some(old) = old {
        sqlx::query("DELETE FROM profile_files WHERE storage_key = $1")
            .bind(&old)
            .execute(&st.db)
            .await?;
        // Kept while a safety report refers to it.
        if !crate::api::safety::file_retained(&st.db, &old).await {
            let _ = tokio::fs::remove_file(st.config.upload_dir.join(&old)).await;
        }
    }
    refresh_my_guilds(st, user_id).await
}

async fn upload(st: &AppState, user_id: i64, kind: &str, mut form: Multipart) -> AppResult<()> {
    let limit = if kind == "banner" {
        MAX_BANNER_BYTES
    } else {
        MAX_AVATAR_BYTES
    };
    let bad = |e: axum::extract::multipart::MultipartError| {
        AppError::BadRequest(format!("Upload failed: {e}"))
    };
    while let Some(field) = form.next_field().await.map_err(bad)? {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or(kind).to_string();
        let content_type = field.content_type().unwrap_or("").to_string();
        let Some(content_type) = image_type(&content_type, &filename) else {
            return Err(AppError::BadRequest(
                "Pictures must be PNG, JPEG, GIF or WebP.".into(),
            ));
        };
        let data = field.bytes().await.map_err(bad)?;
        if data.is_empty() {
            return Err(AppError::BadRequest("That file is empty.".into()));
        }
        if data.len() > limit {
            return Err(AppError::BadRequest(format!(
                "Pictures can be up to {} MB.",
                limit / 1024 / 1024
            )));
        }
        let key = auth::random_code(24, KEY_ALPHABET);
        tokio::fs::create_dir_all(&st.config.upload_dir)
            .await
            .map_err(|e| anyhow::anyhow!("creating upload folder: {e}"))?;
        tokio::fs::write(st.config.upload_dir.join(&key), &data)
            .await
            .map_err(|e| anyhow::anyhow!("saving upload: {e}"))?;
        sqlx::query(
            "INSERT INTO profile_files (storage_key, user_id, filename, content_type)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&key)
        .bind(user_id)
        .bind(kind)
        .bind(content_type)
        .execute(&st.db)
        .await?;
        return replace_image(st, user_id, kind, Some(key)).await;
    }
    Err(AppError::BadRequest("Choose a picture to upload.".into()))
}

/// Multipart form with one `file` field.
pub async fn upload_avatar(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    form: Multipart,
) -> AppResult<Json<User>> {
    upload(&st, user.id, "avatar", form).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}

pub async fn delete_avatar(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<User>> {
    replace_image(&st, user.id, "avatar", None).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}

pub async fn upload_banner(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    form: Multipart,
) -> AppResult<Json<User>> {
    upload(&st, user.id, "banner", form).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}

pub async fn delete_banner(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<User>> {
    replace_image(&st, user.id, "banner", None).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}
