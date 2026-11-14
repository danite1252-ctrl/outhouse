//! "My raids": every raid you can see, in all your outhouses, for the
//! calendar on the rail (and the mini calendar widget).
//!
//! `GET /api/users/@me/raids?from=…&to=…[&guild=…]`: raids overlapping
//! `from`..`to` (up to 62 days) in text channels you can see (age-restricted
//! ones you haven't accepted stay out), soonest first, with their sign-ups
//! (yours included) so the client can colour each day and sign you up.
//! The channels you can see are worked out per outhouse from its overwrites
//! (two small queries each); the raids then come in one indexed query
//! (`events(channel_id, starts_at)`).

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::models::RaidEvent;
use serde::Deserialize;

use super::events::{build, EventRow, EVENT_COLUMNS};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{drop_age_gated, member_ctx, visible_text_channels};
use crate::state::AppState;

/// Longest range one request can ask for.
const MAX_DAYS: i64 = 62;
/// More raids than anyone has in two months (keeps the answer small).
const MAX_RAIDS: i64 = 500;

#[derive(Deserialize)]
pub struct MyRaidsQuery {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    /// Only this outhouse.
    guild: Option<String>,
}

/// Text channels you can see in every outhouse you're in (or just one).
pub(crate) async fn visible_channels(
    st: &AppState,
    user_id: i64,
    only_guild: Option<i64>,
) -> AppResult<Vec<i64>> {
    let guild_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT guild_id FROM guild_members WHERE user_id = $1 AND ($2::BIGINT IS NULL OR guild_id = $2)",
    )
    .bind(user_id)
    .bind(only_guild)
    .fetch_all(&st.db)
    .await?;
    let mut channels = Vec::new();
    for guild_id in guild_ids {
        let ctx = match member_ctx(&st.db, guild_id, user_id).await {
            Ok(ctx) => ctx,
            // Left in the meantime, or shut down by the site owner.
            Err(AppError::NotFound | AppError::Gone(_)) => continue,
            Err(e) => return Err(e),
        };
        channels.extend(visible_text_channels(&st.db, &ctx, 0).await?);
    }
    drop_age_gated(&st.db, user_id, &mut channels).await?;
    Ok(channels)
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<MyRaidsQuery>,
) -> AppResult<Json<Vec<RaidEvent>>> {
    if q.to <= q.from || q.to - q.from > chrono::Duration::days(MAX_DAYS) {
        return Err(AppError::BadRequest(format!(
            "Pick a range of up to {MAX_DAYS} days."
        )));
    }
    let only_guild = match q.guild.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
        Some(raw) => Some(
            raw.parse::<i64>()
                .map_err(|_| AppError::BadRequest("`guild` must be an outhouse ID.".into()))?,
        ),
        None => None,
    };
    let channels = visible_channels(&st, user.id, only_guild).await?;
    if channels.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let sql = format!(
        "SELECT {EVENT_COLUMNS} FROM events
         WHERE channel_id = ANY($1) AND starts_at < $3
           AND starts_at + make_interval(mins => duration_minutes) > $2
         ORDER BY starts_at, id LIMIT {MAX_RAIDS}"
    );
    let rows = sqlx::query_as::<_, EventRow>(&sql)
        .bind(&channels)
        .bind(q.from)
        .bind(q.to)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(build(&st.db, rows).await?))
}
