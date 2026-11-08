//! Notification settings and Web Push subscriptions.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::id::{as_string, vec_as_string};
use outhouse_common::models::DM_GUILD;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access_meta as channel_access, member_ctx};
use crate::push::{self, Level};
use crate::state::AppState;

/// Devices per person (the oldest goes when a new one is added).
const MAX_SUBSCRIPTIONS: i64 = 10;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct LevelRow {
    #[serde(with = "as_string")]
    pub guild_id: i64,
    pub level: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Device {
    #[serde(with = "as_string")]
    pub id: i64,
    /// The push service's address, so a browser can tell which row is itself.
    pub endpoint: String,
    pub user_agent: String,
    pub created_at: DateTime<Utc>,
    pub last_ok_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MutedChannel {
    #[serde(with = "as_string")]
    pub channel_id: i64,
    /// 0 for a direct message.
    #[serde(with = "as_string")]
    pub guild_id: i64,
    pub name: String,
    /// Channel kind (1 = DM, 3 = group DM).
    pub kind: i16,
    /// When the mute wears off (null: until it's turned back on).
    pub until: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct NotificationSettings {
    /// The server's VAPID public key (null = push isn't available).
    pub vapid_public_key: Option<String>,
    /// Outhouses (and "0" for DMs) with a level other than the default.
    pub levels: Vec<LevelRow>,
    #[serde(with = "vec_as_string")]
    pub muted_channels: Vec<i64>,
    /// The muted channels with their names, for the settings page.
    pub muted: Vec<MutedChannel>,
    pub devices: Vec<Device>,
}

/// `GET /api/users/@me/notifications`
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<NotificationSettings>> {
    let levels = sqlx::query_as::<_, LevelRow>(
        "SELECT guild_id, level FROM notify_levels WHERE user_id = $1",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    // Mutes that have worn off go.
    sqlx::query("DELETE FROM channel_mutes WHERE user_id = $1 AND muted_until <= now()")
        .bind(user.id)
        .execute(&st.db)
        .await?;
    let muted = sqlx::query_as::<_, MutedChannel>(
        "SELECT cm.channel_id, COALESCE(c.guild_id, 0) AS guild_id, c.name, c.kind,
                cm.muted_until AS until
         FROM channel_mutes cm JOIN channels c ON c.id = cm.channel_id
         WHERE cm.user_id = $1 ORDER BY cm.created_at",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let muted_channels = muted.iter().map(|m| m.channel_id).collect();
    let devices = sqlx::query_as::<_, Device>(
        "SELECT id, endpoint, user_agent, created_at, last_ok_at
         FROM push_subscriptions WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(NotificationSettings {
        vapid_public_key: st.vapid.as_ref().map(|v| v.public_key.clone()),
        levels,
        muted_channels,
        muted,
        devices,
    }))
}

#[derive(Deserialize)]
pub struct LevelRequest {
    level: String,
}

/// `PUT /api/users/@me/notifications/guilds/{id}` `{level}`: "all",
/// "mentions" or "nothing" (guild 0 = direct messages: "all" or "nothing").
pub async fn set_level(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<LevelRequest>,
) -> AppResult<Json<LevelRow>> {
    Level::parse(req.level.trim())
        .ok_or_else(|| AppError::BadRequest("Pick all, mentions or nothing.".into()))?;
    if guild_id != DM_GUILD {
        member_ctx(&st.db, guild_id, user.id).await?;
    }
    let level = req.level.trim().to_string();
    sqlx::query(
        "INSERT INTO notify_levels (user_id, guild_id, level) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, guild_id) DO UPDATE SET level = EXCLUDED.level",
    )
    .bind(user.id)
    .bind(guild_id)
    .bind(&level)
    .execute(&st.db)
    .await?;
    Ok(Json(LevelRow { guild_id, level }))
}

#[derive(Deserialize)]
pub struct MuteRequest {
    muted: bool,
    /// How long for (15 minutes to 30 days); left out: until unmuted.
    #[serde(default)]
    minutes: Option<i64>,
}

/// Longest timed mute: 30 days.
const MAX_MUTE_MINUTES: i64 = 30 * 24 * 60;

/// `PUT /api/users/@me/notifications/channels/{id}` `{muted, minutes?}`:
/// mutes a channel or DM (for `minutes`, or until unmuted) or unmutes it.
pub async fn set_mute(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<MuteRequest>,
) -> AppResult<Json<serde_json::Value>> {
    channel_access(&st.db, channel_id, user.id).await?;
    let mut until: Option<DateTime<Utc>> = None;
    if req.muted {
        if let Some(m) = req.minutes {
            if !(1..=MAX_MUTE_MINUTES).contains(&m) {
                return Err(AppError::BadRequest(
                    "Mute for 1 minute to 30 days, or until you turn it back on.".into(),
                ));
            }
            until = Some(Utc::now() + chrono::Duration::minutes(m));
        }
        sqlx::query(
            "INSERT INTO channel_mutes (user_id, channel_id, muted_until) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, channel_id) DO UPDATE SET muted_until = EXCLUDED.muted_until",
        )
        .bind(user.id)
        .bind(channel_id)
        .bind(until)
        .execute(&st.db)
        .await?;
    } else {
        sqlx::query("DELETE FROM channel_mutes WHERE user_id = $1 AND channel_id = $2")
            .bind(user.id)
            .bind(channel_id)
            .execute(&st.db)
            .await?;
    }
    // Your other devices grey it out (or bring it back) too.
    st.dispatch(
        crate::state::Audience::User(user.id),
        outhouse_common::gateway::ServerEvent::ChannelMuteUpdate {
            channel_id,
            muted: req.muted,
            until,
        },
    );
    Ok(Json(
        json!({ "channel_id": channel_id.to_string(), "muted": req.muted, "until": until }),
    ))
}

#[derive(Deserialize)]
pub struct SubscriptionKeys {
    p256dh: String,
    auth: String,
}

/// What `PushSubscription.toJSON()` gives in the browser.
#[derive(Deserialize)]
pub struct SubscribeRequest {
    endpoint: String,
    keys: SubscriptionKeys,
}

/// `POST /api/push/subscriptions` with the browser's subscription.
pub async fn subscribe(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(req): Json<SubscribeRequest>,
) -> AppResult<Json<Device>> {
    if st.vapid.is_none() {
        return Err(AppError::BadRequest(
            "Push notifications aren't available on this server.".into(),
        ));
    }
    let endpoint = req.endpoint.trim().to_string();
    if !push::endpoint_allowed(&st.config, &endpoint) {
        return Err(AppError::BadRequest(
            "That isn't a push service Outhouse knows.".into(),
        ));
    }
    let p256dh = push::decode_b64(&req.keys.p256dh);
    let auth = push::decode_b64(&req.keys.auth);
    match (&p256dh, &auth) {
        (Some(k), Some(a)) if k.len() == 65 && k[0] == 4 && a.len() >= 16 && a.len() <= 64 => {}
        _ => {
            return Err(AppError::BadRequest(
                "That subscription's keys don't look right.".into(),
            ))
        }
    }
    let user_agent: String = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    // The same browser signing in as someone else takes the subscription over.
    let device = sqlx::query_as::<_, Device>(
        "INSERT INTO push_subscriptions (id, user_id, endpoint, p256dh, auth, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (endpoint) DO UPDATE
             SET user_id = EXCLUDED.user_id, p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth,
                 user_agent = EXCLUDED.user_agent, failures = 0
         RETURNING id, endpoint, user_agent, created_at, last_ok_at",
    )
    .bind(st.ids.next_id())
    .bind(user.id)
    .bind(&endpoint)
    .bind(req.keys.p256dh.trim())
    .bind(req.keys.auth.trim())
    .bind(&user_agent)
    .fetch_one(&st.db)
    .await?;
    sqlx::query(
        "DELETE FROM push_subscriptions WHERE user_id = $1 AND id NOT IN (
             SELECT id FROM push_subscriptions WHERE user_id = $1
             ORDER BY created_at DESC LIMIT $2)",
    )
    .bind(user.id)
    .bind(MAX_SUBSCRIPTIONS)
    .execute(&st.db)
    .await?;
    Ok(Json(device))
}

#[derive(Deserialize)]
pub struct UnsubscribeRequest {
    endpoint: Option<String>,
    #[serde(default, with = "outhouse_common::id::option_as_string")]
    id: Option<i64>,
}

/// `DELETE /api/push/subscriptions` `{endpoint}` or `{id}`: turns push off
/// for one device.
pub async fn unsubscribe(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<UnsubscribeRequest>,
) -> AppResult<StatusCode> {
    sqlx::query(
        "DELETE FROM push_subscriptions
         WHERE user_id = $1 AND (endpoint = $2 OR id = $3)",
    )
    .bind(user.id)
    .bind(req.endpoint.as_deref().map(str::trim))
    .bind(req.id)
    .execute(&st.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/push/test`: a test notification to all your devices.
pub async fn test(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<push::SendResult>> {
    if st.vapid.is_none() {
        return Err(AppError::BadRequest(
            "Push notifications aren't available on this server.".into(),
        ));
    }
    let payload = json!({
        "title": "Outhouse",
        "body": "Test notification: the plumbing works! 🚽",
        "kind": "test",
        "tag": "test",
        "url": "/",
    });
    let result = push::send_to_user(&st, user.id, &payload)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(result))
}
