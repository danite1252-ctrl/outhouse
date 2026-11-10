//! DM message requests.
//!
//! A one-to-one DM from someone who is neither your friend nor in an
//! outhouse with you is held as a message request. While it's open the
//! recipient isn't one of the DM's `dm_participants`, so nothing reaches
//! them: no gateway events, pushes, unread counts, or access to the files.
//! They see a quiet count on "Message requests" and, there, who it's from
//! and a text preview. Accepting adds them to the DM (everything is
//! delivered then); ignoring hides it without telling the sender; blocking
//! blocks the sender.
//!
//! - Senders may send `MESSAGE_REQUEST_MAX_MESSAGES` (3) messages to an open
//!   request and start `MESSAGE_REQUESTS_PER_DAY` (10) new ones a day.
//! - Becoming friends or sharing an outhouse accepts open requests
//!   between two people automatically.
//! - Replying to someone who has messaged you in a DM is never a request
//!   (this grandfathers conversations from before requests existed).
//! - Bots and the server's own helpers are never held. Teen protections
//!   (api::safety) are stricter and still apply: adults who aren't a
//!   teen's friend can't message them at all, request or not.
//! - Adults can refuse requests entirely (Privacy: `message_requests`).

use std::sync::OnceLock;

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{DmChannel, User};
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

fn env_number(name: &str, default: i64, min: i64, max: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

/// Most messages someone can send to a request that hasn't been accepted.
pub fn max_messages() -> i64 {
    static V: OnceLock<i64> = OnceLock::new();
    *V.get_or_init(|| env_number("MESSAGE_REQUEST_MAX_MESSAGES", 3, 1, 50))
}

/// Most new message requests someone can send in a day.
pub fn per_day() -> i64 {
    static V: OnceLock<i64> = OnceLock::new();
    *V.get_or_init(|| env_number("MESSAGE_REQUESTS_PER_DAY", 10, 1, 1000))
}

// ---- Checks ---------------------------------------------------------------------

/// Friends, or in an outhouse together.
pub async fn connected(db: &PgPool, a: i64, b: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM guild_members x
                       JOIN guild_members y ON x.guild_id = y.guild_id
                       WHERE x.user_id = $1 AND y.user_id = $2)
             OR EXISTS(SELECT 1 FROM friendships WHERE user_a = LEAST($1, $2)::BIGINT
                                                  AND user_b = GREATEST($1, $2)::BIGINT)",
    )
    .bind(a)
    .bind(b)
    .fetch_one(db)
    .await?)
}

async fn is_bot(db: &PgPool, id: i64) -> AppResult<bool> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT is_bot FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await?
            .unwrap_or(false),
    )
}

/// The recipient refuses requests entirely?
async fn setting_refusal(db: &PgPool, other: i64) -> AppResult<Option<String>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT message_requests, display_name FROM users WHERE id = $1")
            .bind(other)
            .fetch_optional(db)
            .await?;
    Ok(match row {
        Some((setting, name)) if setting == "block" => Some(format!(
            "{name} only takes messages from friends and people in their outhouses."
        )),
        _ => None,
    })
}

/// Has `me` used up today's new requests?
async fn daily_refusal(db: &PgPool, me: i64) -> AppResult<Option<String>> {
    let sent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dm_requests
         WHERE from_id = $1 AND sent_at > now() - interval '1 day'",
    )
    .bind(me)
    .fetch_one(db)
    .await?;
    let limit = per_day();
    Ok((sent >= limit).then(|| {
        format!(
            "You've sent {limit} message requests today. Try again tomorrow, or add them as a friend."
        )
    }))
}

fn too_many_messages() -> String {
    let n = max_messages();
    format!(
        "You've sent {n} message{} as a request. Wait for them to accept it.",
        if n == 1 { "" } else { "s" }
    )
}

#[derive(sqlx::FromRow, Clone)]
pub struct OpenRequest {
    pub id: i64,
    pub channel_id: i64,
    pub from_id: i64,
    pub to_id: i64,
    pub messages: i32,
}

/// The DM's open (pending or ignored) request, if any.
pub async fn open_request(db: &PgPool, channel_id: i64) -> AppResult<Option<OpenRequest>> {
    Ok(sqlx::query_as::<_, OpenRequest>(
        "SELECT id, channel_id, from_id, to_id, messages FROM dm_requests
         WHERE channel_id = $1 AND status IN ('pending', 'ignored')",
    )
    .bind(channel_id)
    .fetch_optional(db)
    .await?)
}

/// The other person in a one-to-one DM: the other participant, or whoever
/// an open request is waiting on.
pub async fn other_person(db: &PgPool, channel_id: i64, me: i64) -> AppResult<Option<i64>> {
    let other: Option<i64> = sqlx::query_scalar(
        "SELECT user_id FROM dm_participants WHERE channel_id = $1 AND user_id <> $2 LIMIT 1",
    )
    .bind(channel_id)
    .bind(me)
    .fetch_optional(db)
    .await?;
    if other.is_some() {
        return Ok(other);
    }
    // Not in it (yet): whoever a request here was between, open or not
    // (a blocked request leaves the sender alone in the DM).
    let pair: Option<(i64, i64)> = sqlx::query_as(
        "SELECT from_id, to_id FROM dm_requests WHERE channel_id = $1
         ORDER BY (status IN ('pending', 'ignored')) DESC, id DESC LIMIT 1",
    )
    .bind(channel_id)
    .fetch_optional(db)
    .await?;
    Ok(pair.map(|(from, to)| if from == me { to } else { from }))
}

/// Would `me`'s next message in this one-to-one DM (both in it) be held?
/// Not between friends or outhouse-mates, not with bots, not when `other`
/// has messaged here before (a reply), and not once `other` has accepted a
/// request here.
async fn hold_applies(db: &PgPool, channel_id: i64, me: i64, other: i64) -> AppResult<bool> {
    if is_bot(db, me).await? || is_bot(db, other).await? || connected(db, me, other).await? {
        return Ok(false);
    }
    let known: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE channel_id = $1 AND author_id = $2)
             OR EXISTS(SELECT 1 FROM dm_requests
                       WHERE channel_id = $1 AND to_id = $2 AND status = 'accepted')",
    )
    .bind(channel_id)
    .bind(other)
    .fetch_one(db)
    .await?;
    Ok(!known)
}

/// The one-to-one DM between two people, if there is one.
pub async fn dm_between(db: &PgPool, a: i64, b: i64) -> AppResult<Option<i64>> {
    let key = format!("{}:{}", a.min(b), a.max(b));
    Ok(
        sqlx::query_scalar("SELECT id FROM channels WHERE dm_key = $1")
            .bind(&key)
            .fetch_optional(db)
            .await?,
    )
}

async fn is_participant(db: &PgPool, channel_id: i64, user_id: i64) -> AppResult<bool> {
    crate::perms::is_dm_participant(db, channel_id, user_id).await
}

/// What would happen if `me` messaged `other` now: (why not, as a request).
/// Covers blocks, suspensions, teen rules, privacy and request limits.
pub async fn contact_status(
    st: &AppState,
    me: i64,
    other: i64,
) -> AppResult<(Option<String>, bool)> {
    let db = &st.db;
    if me == other {
        return Ok((Some("That's you.".into()), false));
    }
    let row: Option<(bool, bool)> = sqlx::query_as(
        "SELECT is_bot, COALESCE(suspended_until > now(), false) FROM users WHERE id = $1",
    )
    .bind(other)
    .fetch_optional(db)
    .await?;
    let Some((bot, suspended)) = row else {
        return Err(AppError::NotFound);
    };
    if suspended && !super::audit::is_site_owner(db, me).await? {
        return Ok((Some("This account is suspended.".into()), false));
    }
    if super::friends::blocked_either(db, me, other).await? {
        return Ok((
            Some("You can't message this person right now.".into()),
            false,
        ));
    }
    if let Some(reason) = super::safety::teen_dm_refusal(db, me, other).await? {
        return Ok((Some(reason), false));
    }
    if let Some(ch) = dm_between(db, me, other).await? {
        if let Some(r) = open_request(db, ch).await? {
            if r.to_id == me {
                // Messaging them back accepts their request.
                return Ok((None, false));
            }
            if connected(db, me, other).await? || bot {
                return Ok((None, false));
            }
            if let Some(reason) = setting_refusal(db, other).await? {
                return Ok((Some(reason), true));
            }
            if r.messages as i64 >= max_messages() {
                return Ok((Some(too_many_messages()), true));
            }
            if r.messages == 0 {
                if let Some(reason) = daily_refusal(db, me).await? {
                    return Ok((Some(reason), true));
                }
            }
            return Ok((None, true));
        }
        if is_participant(db, ch, me).await? && is_participant(db, ch, other).await? {
            if !hold_applies(db, ch, me, other).await? {
                return Ok((None, false));
            }
            if let Some(reason) = setting_refusal(db, other).await? {
                return Ok((Some(reason), true));
            }
            if let Some(reason) = daily_refusal(db, me).await? {
                return Ok((Some(reason), true));
            }
            return Ok((None, true));
        }
    }
    // A new conversation.
    if let Err(AppError::Refused(reason)) = super::friends::check_can_dm(db, me, other).await {
        return Ok((Some(reason), false));
    }
    if bot {
        return Ok((Some("Bots can't be messaged.".into()), false));
    }
    if connected(db, me, other).await? {
        return Ok((None, false));
    }
    if is_bot(db, me).await? {
        return Ok((Some("Bots can't send message requests.".into()), false));
    }
    if let Some(reason) = setting_refusal(db, other).await? {
        return Ok((Some(reason), true));
    }
    if let Some(reason) = daily_refusal(db, me).await? {
        return Ok((Some(reason), true));
    }
    Ok((None, true))
}

/// Before starting a new one-to-one DM with someone you're not connected
/// to (it'll be a request): refused if they don't take requests or you've
/// sent too many today.
pub async fn check_new_request(db: &PgPool, me: i64, other: i64) -> AppResult<()> {
    if is_bot(db, me).await? {
        return Err(AppError::Forbidden);
    }
    if let Some(reason) = setting_refusal(db, other).await? {
        return Err(AppError::Refused(reason));
    }
    if let Some(reason) = daily_refusal(db, me).await? {
        return Err(AppError::TooManyRequests(reason));
    }
    Ok(())
}

/// Opens a request for a brand-new DM (no messages yet).
pub async fn open_for_new_dm(st: &AppState, channel_id: i64, from: i64, to: i64) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO dm_requests (channel_id, from_id, to_id) VALUES ($1, $2, $3)
         ON CONFLICT (channel_id) WHERE status IN ('pending', 'ignored') DO NOTHING",
    )
    .bind(channel_id)
    .bind(from)
    .bind(to)
    .execute(&st.db)
    .await?;
    Ok(())
}

/// Called before a message is stored in a one-to-one DM. Returns the
/// request it goes to when it's held (and counts it), None when it's
/// delivered normally.
pub async fn before_send(st: &AppState, channel_id: i64, me: &User) -> AppResult<Option<i64>> {
    if me.bot {
        return Ok(None);
    }
    let db = &st.db;
    for _ in 0..2 {
        if let Some(r) = open_request(db, channel_id).await? {
            if r.from_id != me.id {
                return Ok(None);
            }
            if connected(db, me.id, r.to_id).await? || is_bot(db, r.to_id).await? {
                accept(st, &r, true).await?;
                return Ok(None);
            }
            if let Some(reason) = setting_refusal(db, r.to_id).await? {
                return Err(AppError::Refused(reason));
            }
            if r.messages == 0 {
                if let Some(reason) = daily_refusal(db, me.id).await? {
                    return Err(AppError::TooManyRequests(reason));
                }
            }
            let claimed: Option<i64> = sqlx::query_scalar(
                "UPDATE dm_requests SET messages = messages + 1, sent_at = COALESCE(sent_at, now())
                 WHERE id = $1 AND messages < $2 AND status IN ('pending', 'ignored')
                 RETURNING id",
            )
            .bind(r.id)
            .bind(max_messages() as i32)
            .fetch_optional(db)
            .await?;
            return match claimed {
                Some(id) => Ok(Some(id)),
                None => Err(AppError::TooManyRequests(too_many_messages())),
            };
        }
        // No request open. Both in it: does this message turn it into one?
        // Only you in it (an earlier request of yours was blocked, then
        // they unblocked you): a new request, or straight back in if you're
        // connected now.
        let in_dm: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM dm_participants WHERE channel_id = $1 AND user_id <> $2 LIMIT 1",
        )
        .bind(channel_id)
        .bind(me.id)
        .fetch_optional(db)
        .await?;
        let other = match in_dm {
            Some(o) => o,
            None => match other_person(db, channel_id, me.id).await? {
                Some(o) => o,
                None => return Ok(None),
            },
        };
        if in_dm.is_some() {
            if !hold_applies(db, channel_id, me.id, other).await? {
                return Ok(None);
            }
        } else if connected(db, me.id, other).await? || is_bot(db, other).await? {
            sqlx::query(
                "INSERT INTO dm_participants (channel_id, user_id) VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
            )
            .bind(channel_id)
            .bind(other)
            .execute(db)
            .await?;
            super::dms::announce(st, channel_id).await?;
            return Ok(None);
        }
        if let Some(reason) = setting_refusal(db, other).await? {
            return Err(AppError::Refused(reason));
        }
        if let Some(reason) = daily_refusal(db, me.id).await? {
            return Err(AppError::TooManyRequests(reason));
        }
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO dm_requests (channel_id, from_id, to_id, messages, sent_at)
             VALUES ($1, $2, $3, 1, now())
             ON CONFLICT (channel_id) WHERE status IN ('pending', 'ignored') DO NOTHING
             RETURNING id",
        )
        .bind(channel_id)
        .bind(me.id)
        .bind(other)
        .fetch_optional(db)
        .await?;
        let Some(id) = id else {
            continue; // someone else opened one a moment ago: count against it
        };
        if in_dm.is_some() {
            // They leave the conversation until they accept.
            sqlx::query("DELETE FROM dm_participants WHERE channel_id = $1 AND user_id = $2")
                .bind(channel_id)
                .bind(other)
                .execute(db)
                .await?;
            st.dispatch(
                Audience::User(other),
                ServerEvent::DmDelete { id: channel_id },
            );
        }
        super::dms::announce(st, channel_id).await?;
        return Ok(Some(id));
    }
    Err(AppError::Conflict("Try sending that again.".into()))
}

/// After a held message is stored: the recipient's count goes up.
pub async fn after_send(st: &AppState, request_id: i64) {
    let to: Option<i64> = sqlx::query_scalar("SELECT to_id FROM dm_requests WHERE id = $1")
        .bind(request_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    if let Some(to) = to {
        notify_count(st, to).await;
    }
}

async fn pending_count(db: &PgPool, user_id: i64) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM dm_requests r JOIN users u ON u.id = r.from_id
         WHERE r.to_id = $1 AND r.status = 'pending' AND r.messages > 0
           AND NOT COALESCE(u.suspended_until > now(), false)",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

/// Tells `user_id`'s clients how many requests are waiting.
pub async fn notify_count(st: &AppState, user_id: i64) {
    if let Ok(count) = pending_count(&st.db, user_id).await {
        st.dispatch(
            Audience::User(user_id),
            ServerEvent::MessageRequests { count },
        );
    }
}

/// Accepts a request: the recipient joins the DM and gets everything.
pub async fn accept(st: &AppState, r: &OpenRequest, auto: bool) -> AppResult<()> {
    let done = sqlx::query(
        "UPDATE dm_requests SET status = 'accepted', decided_at = now(), auto = $2
         WHERE id = $1 AND status IN ('pending', 'ignored')",
    )
    .bind(r.id)
    .bind(auto)
    .execute(&st.db)
    .await?
    .rows_affected();
    if done == 0 {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO dm_participants (channel_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(r.channel_id)
    .bind(r.to_id)
    .execute(&st.db)
    .await?;
    super::dms::announce(st, r.channel_id).await?;
    notify_count(st, r.to_id).await;
    Ok(())
}

/// Open requests between two people are accepted (they became friends).
pub async fn accept_between(st: &AppState, a: i64, b: i64) -> AppResult<()> {
    let open = sqlx::query_as::<_, OpenRequest>(
        "SELECT id, channel_id, from_id, to_id, messages FROM dm_requests
         WHERE status IN ('pending', 'ignored')
           AND ((from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1))",
    )
    .bind(a)
    .bind(b)
    .fetch_all(&st.db)
    .await?;
    for r in open {
        accept(st, &r, true).await?;
    }
    Ok(())
}

/// Someone joined an outhouse: open requests between them and its members
/// are accepted.
pub async fn accept_for_new_member(st: &AppState, user_id: i64, guild_id: i64) -> AppResult<()> {
    let open = sqlx::query_as::<_, OpenRequest>(
        "SELECT r.id, r.channel_id, r.from_id, r.to_id, r.messages FROM dm_requests r
         JOIN guild_members m ON m.guild_id = $2
              AND m.user_id = CASE WHEN r.from_id = $1 THEN r.to_id ELSE r.from_id END
         WHERE r.status IN ('pending', 'ignored') AND (r.from_id = $1 OR r.to_id = $1)",
    )
    .bind(user_id)
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    for r in open {
        accept(st, &r, true).await?;
    }
    Ok(())
}

/// `me` blocked `other`: their open requests to `me` end as "blocked".
pub async fn mark_blocked(st: &AppState, me: i64, other: i64) -> AppResult<()> {
    let n = sqlx::query(
        "UPDATE dm_requests SET status = 'blocked', decided_at = now()
         WHERE to_id = $1 AND from_id = $2 AND status IN ('pending', 'ignored')",
    )
    .bind(me)
    .bind(other)
    .execute(&st.db)
    .await?
    .rows_affected();
    if n > 0 {
        notify_count(st, me).await;
    }
    Ok(())
}

/// A message's text for the request list: mentions and custom emoji made
/// readable, at most 300 characters.
fn preview_text(content: &str) -> String {
    let mut out = String::new();
    let mut rest = content;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let inner = &tail[1..end];
        let replaced = if let Some(id) = inner.strip_prefix("@&") {
            id.chars()
                .all(|c| c.is_ascii_digit())
                .then(|| "@role".to_string())
        } else if let Some(id) = inner.strip_prefix('@') {
            id.chars()
                .all(|c| c.is_ascii_digit())
                .then(|| "@someone".to_string())
        } else if let Some(e) = inner.strip_prefix(':').or_else(|| inner.strip_prefix("a:")) {
            e.split_once(':').map(|(name, _)| format!(":{name}:"))
        } else {
            None
        };
        match replaced {
            Some(r) => out.push_str(&r),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    let mut short: String = out.chars().take(300).collect();
    if out.chars().count() > 300 {
        short.push('…');
    }
    short
}

// ---- Endpoints ------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct RequestRow {
    id: i64,
    channel_id: i64,
    created_at: DateTime<Utc>,
    sent_at: Option<DateTime<Utc>>,
    from_id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PreviewRow {
    channel_id: i64,
    id: i64,
    content: String,
    created_at: DateTime<Utc>,
    sticker: bool,
    pictures: i64,
    videos: i64,
    voice: i64,
    files: i64,
}

/// `GET /api/dms/requests`: requests waiting for you, newest first, with a
/// text-only preview (pictures and files are only counted).
pub async fn list(State(st): State<AppState>, AuthUser(user): AuthUser) -> AppResult<Json<Value>> {
    // Any that should have gone through by now (friends since, or an
    // outhouse in common) are accepted first.
    let stale = sqlx::query_as::<_, OpenRequest>(
        "SELECT id, channel_id, from_id, to_id, messages FROM dm_requests
         WHERE to_id = $1 AND status = 'pending'",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    for r in stale {
        if connected(&st.db, r.from_id, r.to_id).await? {
            accept(&st, &r, true).await?;
        }
    }
    let sql = format!(
        "SELECT r.id, r.channel_id, r.created_at, r.sent_at, r.from_id,
                u.username, u.display_name,
                {} AS avatar_url
         FROM dm_requests r JOIN users u ON u.id = r.from_id
         WHERE r.to_id = $1 AND r.status = 'pending' AND r.messages > 0
           AND NOT COALESCE(u.suspended_until > now(), false)
         ORDER BY COALESCE(r.sent_at, r.created_at) DESC
         LIMIT 100",
        crate::auth::AVATAR_URL_SQL
    );
    let rows = sqlx::query_as::<_, RequestRow>(&sql)
        .bind(user.id)
        .fetch_all(&st.db)
        .await?;
    let channels: Vec<i64> = rows.iter().map(|r| r.channel_id).collect();
    let previews = sqlx::query_as::<_, PreviewRow>(
        "SELECT m.channel_id, m.id, m.content, m.created_at, m.sticker IS NOT NULL AS sticker,
                count(a.id) FILTER (WHERE a.content_type LIKE 'image/%') AS pictures,
                count(a.id) FILTER (WHERE a.content_type LIKE 'video/%') AS videos,
                count(a.id) FILTER (WHERE a.voice) AS voice,
                count(a.id) FILTER (WHERE NOT a.voice AND a.content_type NOT LIKE 'image/%'
                                     AND a.content_type NOT LIKE 'video/%') AS files
         FROM messages m
         JOIN dm_requests r ON r.channel_id = m.channel_id AND r.to_id = $2
              AND r.status = 'pending' AND m.author_id = r.from_id
         LEFT JOIN attachments a ON a.message_id = m.id
         WHERE m.channel_id = ANY($1) AND NOT m.safety_hidden AND m.thread_id IS NULL
         GROUP BY m.id
         ORDER BY m.id DESC
         LIMIT 500",
    )
    .bind(&channels)
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut msgs: Vec<Value> = previews
                .iter()
                .filter(|p| p.channel_id == r.channel_id)
                .take(5)
                .map(|p| {
                    json!({
                        "id": p.id.to_string(),
                        "content": preview_text(&p.content),
                        "created_at": p.created_at,
                        "sticker": p.sticker,
                        "pictures": p.pictures,
                        "videos": p.videos,
                        "voice": p.voice,
                        "files": p.files,
                    })
                })
                .collect();
            msgs.reverse();
            json!({
                "id": r.id.to_string(),
                "channel_id": r.channel_id.to_string(),
                "created_at": r.created_at,
                "sent_at": r.sent_at,
                "user": {
                    "id": r.from_id.to_string(),
                    "username": r.username,
                    "display_name": r.display_name,
                    "avatar_url": r.avatar_url,
                },
                "messages": msgs,
            })
        })
        .collect();
    Ok(Json(json!({ "count": out.len(), "requests": out })))
}

async fn my_request(st: &AppState, channel_id: i64, me: i64) -> AppResult<OpenRequest> {
    sqlx::query_as::<_, OpenRequest>(
        "SELECT id, channel_id, from_id, to_id, messages FROM dm_requests
         WHERE channel_id = $1 AND to_id = $2 AND status = 'pending'",
    )
    .bind(channel_id)
    .bind(me)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)
}

/// `POST /api/dms/requests/{channel_id}/accept`: join the conversation.
pub async fn accept_route(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<DmChannel>> {
    let r = my_request(&st, channel_id, user.id).await?;
    if super::friends::blocked_either(&st.db, r.from_id, user.id).await? {
        return Err(AppError::Refused(
            "Unblock them first to accept their message.".into(),
        ));
    }
    accept(&st, &r, false).await?;
    Ok(Json(super::dms::dm_for(&st, channel_id, user.id).await?))
}

/// `POST /api/dms/requests/{channel_id}/ignore`: hide it (they aren't told).
pub async fn ignore_route(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<axum::http::StatusCode> {
    let r = my_request(&st, channel_id, user.id).await?;
    sqlx::query("UPDATE dm_requests SET status = 'ignored', decided_at = now() WHERE id = $1")
        .bind(r.id)
        .execute(&st.db)
        .await?;
    notify_count(&st, user.id).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// The site owner's console: requests someone has sent, and how they went.
pub async fn stats_for(db: &PgPool, user_id: i64) -> AppResult<Value> {
    let (sent, pending, accepted, auto, ignored, blocked, last_day): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT count(*),
                count(*) FILTER (WHERE status = 'pending'),
                count(*) FILTER (WHERE status = 'accepted' AND NOT auto),
                count(*) FILTER (WHERE status = 'accepted' AND auto),
                count(*) FILTER (WHERE status = 'ignored'),
                count(*) FILTER (WHERE status = 'blocked'),
                count(*) FILTER (WHERE sent_at > now() - interval '1 day')
         FROM dm_requests WHERE from_id = $1 AND sent_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    let received: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dm_requests WHERE to_id = $1 AND sent_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(json!({
        "sent": sent,
        "pending": pending,
        "accepted": accepted,
        "auto_accepted": auto,
        "ignored": ignored,
        "blocked": blocked,
        "last_day": last_day,
        "received": received,
    }))
}

#[cfg(test)]
mod tests {
    use super::preview_text;

    #[test]
    fn previews_are_readable_and_short() {
        assert_eq!(
            preview_text("hi <@123> and <@&45>, look <:pog:678> <a:spin:9>"),
            "hi @someone and @role, look :pog: :spin:"
        );
        assert_eq!(preview_text("a < b > c"), "a < b > c");
        assert_eq!(preview_text("trailing <"), "trailing <");
        let long = "x".repeat(400);
        let short = preview_text(&long);
        assert_eq!(short.chars().count(), 301);
        assert!(short.ends_with('…'));
    }
}
