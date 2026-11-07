//! Serves uploaded files at `/files/{key}/{filename}`.
//!
//! The random key in the URL is what grants access (like Discord's CDN
//! links). Files are served with strict headers so an uploaded HTML or SVG
//! file can never run as a web page on the Outhouse site.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::response::Response;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(sqlx::FromRow)]
struct FileRow {
    filename: String,
    content_type: String,
}

/// Content types we're happy for browsers to show inline.
fn safe_type(content_type: &str) -> (&str, bool) {
    match content_type {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "video/mp4" | "video/webm"
        | "audio/mpeg" | "audio/ogg" | "audio/wav" => (content_type, true),
        "text/plain" => ("text/plain; charset=utf-8", true),
        "application/pdf" => ("application/pdf", false),
        _ => ("application/octet-stream", false),
    }
}

pub async fn serve(
    State(st): State<AppState>,
    Path((key, _name)): Path<(String, String)>,
) -> AppResult<Response> {
    if key.len() != 24 || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(AppError::NotFound);
    }
    let row = sqlx::query_as::<_, FileRow>(
        "SELECT filename, content_type FROM attachments WHERE storage_key = $1
         UNION ALL
         SELECT name AS filename, content_type FROM sounds WHERE storage_key = $1
         UNION ALL
         SELECT filename, content_type FROM profile_files WHERE storage_key = $1
         UNION ALL
         SELECT name AS filename, content_type FROM emojis WHERE storage_key = $1
         LIMIT 1",
    )
    .bind(&key)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    send_file(&st, &key, &row.filename, &row.content_type).await
}

/// A custom emoji's picture, by emoji ID (`/emojis/{id}`), so messages only
/// need to store `<:name:id>`.
pub async fn serve_emoji(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let id: i64 = id
        .trim_end_matches(".png")
        .parse()
        .map_err(|_| AppError::NotFound)?;
    let row = sqlx::query_as::<_, EmojiFileRow>(
        "SELECT storage_key, name, content_type FROM emojis WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    send_file(&st, &row.storage_key, &row.name, &row.content_type).await
}

#[derive(sqlx::FromRow)]
struct EmojiFileRow {
    storage_key: String,
    name: String,
    content_type: String,
}

async fn send_file(
    st: &AppState,
    key: &str,
    filename: &str,
    content_type: &str,
) -> AppResult<Response> {
    let data = tokio::fs::read(st.config.upload_dir.join(key))
        .await
        .map_err(|_| AppError::NotFound)?;

    let (content_type, inline) = safe_type(content_type);
    let ascii_name: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let disposition = format!(
        "{}; filename=\"{ascii_name}\"",
        if inline { "inline" } else { "attachment" }
    );

    Response::builder()
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_DISPOSITION, disposition)
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(CONTENT_SECURITY_POLICY, "default-src 'none'; sandbox")
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from(data))
        .map_err(|e| AppError::Other(e.into()))
}
