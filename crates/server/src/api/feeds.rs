//! Managing RSS/Atom news feeds.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::models::{CreateFeedRequest, Feed};
use outhouse_common::permissions as p;

use crate::auth::{self, AuthUser};
use crate::embeds::clip;
use crate::error::{AppError, AppResult};
use crate::feeds::{self, FEED_SELECT};
use crate::perms::{channel_access, member_ctx};
use crate::state::AppState;

async fn fetch(st: &AppState, feed_id: i64) -> AppResult<Feed> {
    let sql = format!("{FEED_SELECT} WHERE id = $1");
    sqlx::query_as::<_, Feed>(&sql)
        .bind(feed_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Feed>>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_WEBHOOKS)?;
    let sql = format!("{FEED_SELECT} WHERE guild_id = $1 ORDER BY created_at");
    let list = sqlx::query_as::<_, Feed>(&sql)
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(list))
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateFeedRequest>,
) -> AppResult<Json<Feed>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::MANAGE_WEBHOOKS)?;
    let url = req.url.trim().to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AppError::BadRequest(
            "Feed addresses start with https://".into(),
        ));
    }

    // Read it once now, so a bad address fails straight away.
    let parsed = feeds::fetch_feed(&url)
        .await
        .map_err(|e| AppError::BadRequest(format!("Couldn't read that feed: {e:#}")))?;
    let title = clip(parsed.title, 80);

    let id = st.ids.next_id();
    let bot_id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    auth::create_bot_user(&mut *tx, bot_id, &title).await?;
    sqlx::query(
        "INSERT INTO feeds (id, guild_id, channel_id, user_id, url, title,
                            include_keywords, exclude_keywords, creator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(access.channel.guild_id)
    .bind(channel_id)
    .bind(bot_id)
    .bind(&url)
    .bind(&title)
    .bind(req.include_keywords.trim())
    .bind(req.exclude_keywords.trim())
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let feed = fetch(&st, id).await?;
    crate::audit::Entry::new(access.channel.guild_id, user.id, "feed_create")
        .target("feed", id, title.clone())
        .details(format!("{url} → #{}", access.channel.name))
        .save(&st)
        .await;
    // Post the newest item so you can see it working.
    feeds::check_and_record(&st, &feed, true).await;
    Ok(Json(fetch(&st, id).await?))
}

pub async fn check_now(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(feed_id): Path<i64>,
) -> AppResult<Json<Feed>> {
    let feed = fetch(&st, feed_id).await?;
    member_ctx(&st.db, feed.guild_id, user.id)
        .await?
        .require(p::MANAGE_WEBHOOKS)?;
    feeds::check_and_record(&st, &feed, false).await;
    Ok(Json(fetch(&st, feed_id).await?))
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(feed_id): Path<i64>,
) -> AppResult<StatusCode> {
    let feed = fetch(&st, feed_id).await?;
    member_ctx(&st.db, feed.guild_id, user.id)
        .await?
        .require(p::MANAGE_WEBHOOKS)?;
    sqlx::query("DELETE FROM feeds WHERE id = $1")
        .bind(feed_id)
        .execute(&st.db)
        .await?;
    crate::audit::Entry::new(feed.guild_id, user.id, "feed_delete")
        .target("feed", feed_id, feed.title.clone())
        .details(feed.url.clone())
        .save(&st)
        .await;
    Ok(StatusCode::NO_CONTENT)
}
