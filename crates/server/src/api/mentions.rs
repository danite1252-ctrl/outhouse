//! The Mentions inbox: messages that pinged you, across every outhouse and
//! DM, newest first.
//!
//! A row goes into `mentions` for each person a message pings when it's
//! posted (and the rows follow the message when it's edited): `<@you>`
//! ("user"), one of your roles ("role"), a reply to you ("reply"), or
//! @everyone/@here ("everyone", which, like push notifications, skips people
//! who muted the channel or set the outhouse to "Nothing"). Bots, the author,
//! people who blocked the author and anyone who can't see the channel get
//! none. Writing rows once is cheap; working mentions out from message text
//! on every read would mean scanning every channel you can see.
//!
//! Reading checks again what you can see *now*: channels you still have
//! access to (with Read Message History; age-restricted ones only if you've
//! accepted them, never for teens when that's switched off), DMs you're
//! still in, messages that aren't hidden by a safety review, authors you
//! haven't blocked since. Pictures the image detector holds stay held (the
//! message comes through the normal message loader).
//!
//! Unread: not marked read in the inbox, and not read in the channel
//! (or thread) either. Rows older than 90 days are removed; the inbox shows
//! the last 30.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::id::option_as_string;
use outhouse_common::models::{Message, DM_GUILD};
use outhouse_common::permissions as p;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::messages::{hydrate, MessageRow, MESSAGE_SELECT};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{drop_age_gated, member_ctx, visible_text_channels};
use crate::state::{AppState, Audience};

/// The inbox shows this many days.
pub const SHOW_DAYS: i32 = 30;
/// Rows are kept this long.
pub const KEEP_DAYS: i32 = 90;
const MAX_PAGE: i64 = 50;

/// Does the text say @everyone or @here (as a word, like push notifications)?
pub fn says_everyone(content: &str) -> bool {
    content.split_whitespace().any(|w| {
        let w = w.trim_end_matches(|c: char| !c.is_alphanumeric());
        w == "@everyone" || w == "@here"
    })
}

#[derive(sqlx::FromRow)]
struct Candidate {
    user_id: i64,
    kind: String,
}

/// Records who a new (or edited) message pings, in the background.
pub fn spawn_record(st: &AppState, guild_id: i64, message: &Message, edited: bool) {
    let st = st.clone();
    let message = message.clone();
    tokio::spawn(async move {
        if let Err(e) = record(&st, guild_id, &message, edited).await {
            tracing::warn!(error = %e, "recording mentions failed");
        }
    });
}

async fn record(st: &AppState, guild_id: i64, message: &Message, edited: bool) -> AppResult<()> {
    let content = &message.content;
    let users = crate::push::mention_ids(content, "<@");
    let roles = crate::push::mention_ids(content, "<@&");
    let everyone = says_everyone(content);
    let reply_to = message.reply_to.as_ref().map(|r| r.author_id);
    if users.is_empty() && roles.is_empty() && !everyone && reply_to.is_none() && !edited {
        return Ok(());
    }
    let channel_id = message.channel_id;
    let candidates: Vec<Candidate> = if guild_id == DM_GUILD {
        sqlx::query_as(
            "SELECT p.user_id,
                    CASE WHEN p.user_id = ANY($3) THEN 'user'
                         WHEN p.user_id = $4 THEN 'reply' ELSE 'everyone' END AS kind
             FROM dm_participants p JOIN users u ON u.id = p.user_id
             WHERE p.channel_id = $1 AND p.user_id <> $2 AND NOT u.is_bot
               AND (p.user_id = ANY($3) OR p.user_id = $4 OR $5)
               AND NOT EXISTS (SELECT 1 FROM user_blocks b
                               WHERE b.user_id = p.user_id AND b.blocked_id = $2)",
        )
        .bind(channel_id)
        .bind(message.author_id)
        .bind(&users)
        .bind(reply_to)
        .bind(everyone)
        .fetch_all(&st.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT gm.user_id,
                    CASE WHEN gm.user_id = ANY($3) THEN 'user'
                         WHEN EXISTS (SELECT 1 FROM member_roles mr
                                      WHERE mr.guild_id = $1 AND mr.user_id = gm.user_id
                                        AND mr.role_id = ANY($4)) THEN 'role'
                         WHEN gm.user_id = $5 THEN 'reply' ELSE 'everyone' END AS kind
             FROM guild_members gm JOIN users u ON u.id = gm.user_id
             WHERE gm.guild_id = $1 AND gm.user_id <> $2 AND NOT u.is_bot
               AND (gm.user_id = ANY($3) OR gm.user_id = $5
                    OR EXISTS (SELECT 1 FROM member_roles mr
                               WHERE mr.guild_id = $1 AND mr.user_id = gm.user_id
                                 AND mr.role_id = ANY($4))
                    OR ($6
                        AND NOT EXISTS (SELECT 1 FROM channel_mutes cm
                                        WHERE cm.user_id = gm.user_id AND cm.channel_id = $7
                                          AND (cm.muted_until IS NULL OR cm.muted_until > now()))
                        AND NOT EXISTS (SELECT 1 FROM notify_levels l
                                        WHERE l.user_id = gm.user_id AND l.guild_id = $1
                                          AND l.level = 'nothing')))
               AND NOT EXISTS (SELECT 1 FROM user_blocks b
                               WHERE b.user_id = gm.user_id AND b.blocked_id = $2)",
        )
        .bind(guild_id)
        .bind(message.author_id)
        .bind(&users)
        .bind(&roles)
        .bind(reply_to)
        .bind(everyone)
        .bind(channel_id)
        .fetch_all(&st.db)
        .await?
    };
    // Only people who can see the channel.
    let mut keep: Vec<Candidate> = Vec::with_capacity(candidates.len());
    for c in candidates {
        if guild_id == DM_GUILD
            || crate::perms::can_view(&st.db, guild_id, channel_id, c.user_id).await
        {
            keep.push(c);
        }
    }
    let ids: Vec<i64> = keep.iter().map(|c| c.user_id).collect();
    let kinds: Vec<String> = keep.iter().map(|c| c.kind.clone()).collect();
    let mut changed: HashSet<i64> = HashSet::new();
    if edited {
        // Edited so it no longer pings them.
        let gone: Vec<i64> = sqlx::query_scalar(
            "DELETE FROM mentions WHERE message_id = $1 AND user_id <> ALL($2) RETURNING user_id",
        )
        .bind(message.id)
        .bind(&ids)
        .fetch_all(&st.db)
        .await?;
        changed.extend(gone);
    }
    if !ids.is_empty() {
        let added: Vec<i64> = sqlx::query_scalar(
            "INSERT INTO mentions (user_id, message_id, channel_id, guild_id, thread_id, author_id,
                                   kind, created_at)
             SELECT u, $2, $3, $4, $5, $6, k, $7 FROM unnest($1::BIGINT[], $8::TEXT[]) AS x(u, k)
             ON CONFLICT (user_id, message_id) DO UPDATE SET kind = EXCLUDED.kind
             RETURNING user_id",
        )
        .bind(&ids)
        .bind(message.id)
        .bind(channel_id)
        .bind(guild_id)
        .bind(message.thread_id)
        .bind(message.author_id)
        .bind(message.created_at)
        .bind(&kinds)
        .fetch_all(&st.db)
        .await?;
        changed.extend(added);
    }
    for user_id in changed {
        st.dispatch(Audience::User(user_id), ServerEvent::MentionsChanged);
    }
    Ok(())
}

/// Where your mentions can be shown: text channels you can read in your
/// outhouses (age rules applied) and the DMs you're in.
async fn readable_channels(st: &AppState, user_id: i64) -> AppResult<Vec<i64>> {
    let guild_ids: Vec<i64> =
        sqlx::query_scalar("SELECT guild_id FROM guild_members WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&st.db)
            .await?;
    let mut channels = Vec::new();
    for guild_id in guild_ids {
        let ctx = match member_ctx(&st.db, guild_id, user_id).await {
            Ok(ctx) => ctx,
            Err(AppError::NotFound | AppError::Gone(_)) => continue,
            Err(e) => return Err(e),
        };
        channels.extend(visible_text_channels(&st.db, &ctx, p::READ_HISTORY).await?);
    }
    drop_age_gated(&st.db, user_id, &mut channels).await?;
    let dms: Vec<i64> =
        sqlx::query_scalar("SELECT channel_id FROM dm_participants WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&st.db)
            .await?;
    channels.extend(dms);
    Ok(channels)
}

/// Mentions you can see now (the shared part of every query here).
const VISIBLE: &str = "
    mn.user_id = $1 AND mn.channel_id = ANY($2)
    AND mn.created_at > now() - make_interval(days => $3)
    AND NOT m.safety_hidden
    AND NOT EXISTS (SELECT 1 FROM user_blocks b WHERE b.user_id = $1 AND b.blocked_id = mn.author_id)";

/// Not marked read in the inbox and not read in its channel or thread.
const UNREAD: &str = "(mn.read_at IS NULL AND mn.message_id > COALESCE(
        CASE WHEN mn.thread_id IS NULL
             THEN (SELECT rs.last_read_id FROM read_states rs
                   WHERE rs.user_id = mn.user_id AND rs.channel_id = mn.channel_id)
             ELSE (SELECT tr.last_read_id FROM thread_reads tr
                   WHERE tr.user_id = mn.user_id AND tr.thread_id = mn.thread_id) END, 0))";

async fn unread_count(st: &AppState, user_id: i64, channels: &[i64]) -> AppResult<i64> {
    let sql = format!(
        "SELECT count(*) FROM mentions mn JOIN messages m ON m.id = mn.message_id
         WHERE {VISIBLE} AND {UNREAD}"
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(channels)
        .bind(SHOW_DAYS)
        .fetch_one(&st.db)
        .await?)
}

#[derive(Deserialize)]
pub struct InboxQuery {
    /// Older than this message ID (the next page).
    before: Option<String>,
    limit: Option<i64>,
    /// "unread" for unread ones only.
    filter: Option<String>,
    /// One outhouse ("0": direct messages).
    guild: Option<String>,
}

#[derive(sqlx::FromRow)]
struct InboxRow {
    message_id: i64,
    guild_id: i64,
    channel_id: i64,
    thread_id: Option<i64>,
    kind: String,
    unread: bool,
    channel_name: String,
    thread_name: Option<String>,
}

#[derive(Serialize)]
pub struct InboxItem {
    pub message: Message,
    #[serde(with = "outhouse_common::id::as_string")]
    pub guild_id: i64,
    #[serde(with = "outhouse_common::id::as_string")]
    pub channel_id: i64,
    pub channel_name: String,
    #[serde(with = "option_as_string")]
    pub thread_id: Option<i64>,
    pub thread_name: Option<String>,
    pub kind: String,
    pub unread: bool,
}

#[derive(Serialize)]
pub struct Inbox {
    pub mentions: Vec<InboxItem>,
    /// Unread mentions you can see (all of them, whatever the filter).
    pub unread: i64,
    pub has_more: bool,
}

fn parse_id(raw: Option<&str>, what: &str) -> AppResult<Option<i64>> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<i64>()
                .map_err(|_| AppError::BadRequest(format!("`{what}` must be an ID.")))
        })
        .transpose()
}

/// `GET /api/users/@me/mentions?before=&limit=&filter=unread&guild=`
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<InboxQuery>,
) -> AppResult<Json<Inbox>> {
    let before = parse_id(q.before.as_deref(), "before")?;
    let guild = parse_id(q.guild.as_deref(), "guild")?;
    let unread_only = match q.filter.as_deref().unwrap_or("all") {
        "all" | "" => false,
        "unread" => true,
        _ => return Err(AppError::BadRequest("The filter is all or unread.".into())),
    };
    let limit = q.limit.unwrap_or(25).clamp(1, MAX_PAGE);
    let channels = readable_channels(&st, user.id).await?;
    let unread = unread_count(&st, user.id, &channels).await?;
    let sql = format!(
        "SELECT mn.message_id, mn.guild_id, mn.channel_id, mn.thread_id, mn.kind,
                {UNREAD} AS unread, c.name AS channel_name, t.name AS thread_name
         FROM mentions mn
         JOIN messages m ON m.id = mn.message_id
         JOIN channels c ON c.id = mn.channel_id
         LEFT JOIN threads t ON t.id = mn.thread_id
         WHERE {VISIBLE}
           AND ($4::BIGINT IS NULL OR mn.message_id < $4)
           AND ($5::BIGINT IS NULL OR mn.guild_id = $5)
           AND (NOT $6 OR {UNREAD})
         ORDER BY mn.message_id DESC
         LIMIT $7"
    );
    let mut rows = sqlx::query_as::<_, InboxRow>(&sql)
        .bind(user.id)
        .bind(&channels)
        .bind(SHOW_DAYS)
        .bind(before)
        .bind(guild)
        .bind(unread_only)
        .bind(limit + 1)
        .fetch_all(&st.db)
        .await?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let ids: Vec<i64> = rows.iter().map(|r| r.message_id).collect();
    let msg_sql = format!("{MESSAGE_SELECT} WHERE m.id = ANY($1) AND NOT m.safety_hidden");
    let msg_rows = sqlx::query_as::<_, MessageRow>(&msg_sql)
        .bind(&ids)
        .fetch_all(&st.db)
        .await?;
    let mut messages: HashMap<i64, Message> = hydrate(&st.db, msg_rows)
        .await?
        .into_iter()
        .map(|m| (m.id, m))
        .collect();
    let mentions = rows
        .into_iter()
        .filter_map(|r| {
            messages.remove(&r.message_id).map(|message| InboxItem {
                message,
                guild_id: r.guild_id,
                channel_id: r.channel_id,
                channel_name: r.channel_name,
                thread_id: r.thread_id,
                thread_name: r.thread_name,
                kind: r.kind,
                unread: r.unread,
            })
        })
        .collect();
    Ok(Json(Inbox {
        mentions,
        unread,
        has_more,
    }))
}

/// `GET /api/users/@me/mentions/count`: `{"unread": n}`.
pub async fn count(State(st): State<AppState>, AuthUser(user): AuthUser) -> AppResult<Json<Value>> {
    let channels = readable_channels(&st, user.id).await?;
    let unread = unread_count(&st, user.id, &channels).await?;
    Ok(Json(json!({ "unread": unread })))
}

#[derive(Deserialize)]
pub struct MarkRead {
    #[serde(default, with = "outhouse_common::id::vec_as_string")]
    message_ids: Vec<i64>,
    /// Everything (in one outhouse, with `guild_id`; "0": direct messages).
    #[serde(default)]
    all: bool,
    #[serde(default, with = "option_as_string")]
    guild_id: Option<i64>,
}

/// `POST /api/users/@me/mentions/read`: `{"message_ids": [...]}` or
/// `{"all": true[, "guild_id": "…"]}`.
pub async fn mark_read(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<MarkRead>,
) -> AppResult<Json<Value>> {
    if !req.all && req.message_ids.is_empty() {
        return Err(AppError::BadRequest(
            "Say which mentions to mark read.".into(),
        ));
    }
    if req.message_ids.len() > 200 {
        return Err(AppError::BadRequest("Up to 200 at a time.".into()));
    }
    let marked = if req.all {
        sqlx::query(
            "UPDATE mentions SET read_at = now()
             WHERE user_id = $1 AND read_at IS NULL AND ($2::BIGINT IS NULL OR guild_id = $2)",
        )
        .bind(user.id)
        .bind(req.guild_id)
        .execute(&st.db)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "UPDATE mentions SET read_at = now()
             WHERE user_id = $1 AND read_at IS NULL AND message_id = ANY($2)",
        )
        .bind(user.id)
        .bind(&req.message_ids)
        .execute(&st.db)
        .await?
        .rows_affected()
    };
    st.dispatch(Audience::User(user.id), ServerEvent::MentionsChanged);
    let channels = readable_channels(&st, user.id).await?;
    let unread = unread_count(&st, user.id, &channels).await?;
    Ok(Json(json!({ "marked": marked, "unread": unread })))
}

async fn remove_old(st: &AppState) -> AppResult<u64> {
    Ok(
        sqlx::query("DELETE FROM mentions WHERE created_at < now() - make_interval(days => $1)")
            .bind(KEEP_DAYS)
            .execute(&st.db)
            .await?
            .rows_affected(),
    )
}

/// Removes mentions older than 90 days, every few hours.
pub fn spawn_cleaner(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3 * 3600));
        loop {
            tick.tick().await;
            match remove_old(&st).await {
                Ok(n) if n > 0 => tracing::info!(removed = n, "removed old mentions"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "couldn't remove old mentions"),
            }
        }
    });
}

/// `POST /api/users/@me/mentions/cleanup`: the 90-day clean-up, now. Only
/// with `OUTHOUSE_TEST_HOOKS=1` (the smoke test backdates rows in SQL).
pub async fn cleanup_now(
    State(st): State<AppState>,
    AuthUser(_user): AuthUser,
) -> AppResult<Json<Value>> {
    if std::env::var("OUTHOUSE_TEST_HOOKS").ok().as_deref() != Some("1") {
        return Err(AppError::NotFound);
    }
    let removed = remove_old(&st).await?;
    Ok(Json(json!({ "removed": removed })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_everyone_and_here_as_words() {
        assert!(says_everyone("@everyone raid at 8"));
        assert!(says_everyone("pizza? @here!"));
        assert!(!says_everyone("email@everyone.com"));
        assert!(!says_everyone("@everyones"));
    }
}
