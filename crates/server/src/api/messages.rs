//! Messages: history, send (with uploads), edit, delete, reactions, typing.

use std::collections::HashMap;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    Attachment, CreateMessageRequest, EditMessageRequest, Embed, EmbedField, Message,
    MessageInteraction, MessageRef, Poll, PollOption, Reaction, ThreadInfo, User,
};
use outhouse_common::permissions as p;
use serde::Deserialize;
use sqlx::types::Json as SqlJson;
use sqlx::PgPool;

use crate::auth::{self, AuthUser};
use crate::embeds;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, ChannelAccess};
use crate::state::{AppState, Audience};

const MAX_MESSAGE_CHARS: usize = 4000;
pub const MAX_FILES: usize = 10;
pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
const KEY_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

// ---- Loading -----------------------------------------------------------

#[derive(sqlx::FromRow)]
pub(crate) struct MessageRow {
    id: i64,
    channel_id: i64,
    author_id: i64,
    author_username: String,
    author_display_name: String,
    author_avatar_url: Option<String>,
    author_bot: bool,
    content: String,
    created_at: DateTime<Utc>,
    edited_at: Option<DateTime<Utc>>,
    reply_to_id: Option<i64>,
    interaction: Option<SqlJson<MessageInteraction>>,
    pinned_at: Option<DateTime<Utc>>,
    thread_id: Option<i64>,
}

pub(crate) const MESSAGE_SELECT: &str = "
    SELECT m.id, m.channel_id, m.author_id,
           u.username AS author_username,
           COALESCE(m.override_name, gm.nickname, u.display_name) AS author_display_name,
           COALESCE(m.override_avatar,
               CASE WHEN u.avatar_key IS NULL THEN NULL ELSE '/files/' || u.avatar_key || '/avatar' END)
               AS author_avatar_url,
           u.is_bot AS author_bot,
           m.content, m.created_at, m.edited_at, m.reply_to_id, m.interaction, m.pinned_at,
           m.thread_id
    FROM messages m
    JOIN users u ON u.id = m.author_id
    JOIN channels ch ON ch.id = m.channel_id
    LEFT JOIN guild_members gm ON gm.guild_id = ch.guild_id AND gm.user_id = m.author_id";

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    id: i64,
    message_id: i64,
    storage_key: String,
    filename: String,
    content_type: String,
    size: i64,
}

#[derive(sqlx::FromRow)]
struct EmbedRow {
    message_id: i64,
    url: String,
    kind: String,
    title: Option<String>,
    description: Option<String>,
    site_name: Option<String>,
    image_url: Option<String>,
    color: Option<i32>,
    author_name: Option<String>,
    author_url: Option<String>,
    author_icon_url: Option<String>,
    footer: Option<String>,
    thumbnail_url: Option<String>,
    timestamp: Option<String>,
    fields: Option<SqlJson<Vec<EmbedField>>>,
}

#[derive(sqlx::FromRow)]
struct ReactionRow {
    message_id: i64,
    emoji: String,
    user_ids: Vec<i64>,
}

#[derive(sqlx::FromRow)]
struct PollRow {
    message_id: i64,
    question: String,
    options: Vec<String>,
    multi: bool,
    ends_at: Option<DateTime<Utc>>,
    closed: bool,
}

#[derive(sqlx::FromRow)]
struct VoteRow {
    message_id: i64,
    option_idx: i32,
    user_ids: Vec<i64>,
}

pub(crate) const THREAD_SELECT: &str =
    "SELECT id, channel_id, starter_message_id, name, creator_id,
        message_count, last_message_id, created_at FROM threads";

#[derive(sqlx::FromRow)]
struct ReplyRow {
    id: i64,
    author_id: i64,
    author_display_name: String,
    content: String,
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn attachment_url(storage_key: &str, filename: &str) -> String {
    format!("/files/{storage_key}/{}", url_encode(filename))
}

/// Turns database rows into full messages (attachments, embeds, reactions,
/// reply previews), loading each kind in one query.
pub(crate) async fn hydrate(db: &PgPool, rows: Vec<MessageRow>) -> AppResult<Vec<Message>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let reply_ids: Vec<i64> = rows.iter().filter_map(|r| r.reply_to_id).collect();

    let attachments = sqlx::query_as::<_, AttachmentRow>(
        "SELECT id, message_id, storage_key, filename, content_type, size
         FROM attachments WHERE message_id = ANY($1) ORDER BY id",
    )
    .bind(&ids[..])
    .fetch_all(db)
    .await?;
    let embeds = sqlx::query_as::<_, EmbedRow>(
        "SELECT message_id, url, kind, title, description, site_name, image_url,
                color, author_name, author_url, author_icon_url, footer, thumbnail_url,
                timestamp, fields
         FROM message_embeds WHERE message_id = ANY($1) ORDER BY message_id, position",
    )
    .bind(&ids[..])
    .fetch_all(db)
    .await?;
    let reactions = sqlx::query_as::<_, ReactionRow>(
        "SELECT message_id, emoji, array_agg(user_id ORDER BY created_at) AS user_ids
         FROM reactions WHERE message_id = ANY($1)
         GROUP BY message_id, emoji
         ORDER BY min(created_at)",
    )
    .bind(&ids[..])
    .fetch_all(db)
    .await?;
    let replies = if reply_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, ReplyRow>(
            "SELECT m.id, m.author_id,
                    COALESCE(m.override_name, gm.nickname, u.display_name) AS author_display_name,
                    m.content
             FROM messages m
             JOIN users u ON u.id = m.author_id
             JOIN channels ch ON ch.id = m.channel_id
             LEFT JOIN guild_members gm ON gm.guild_id = ch.guild_id AND gm.user_id = m.author_id
             WHERE m.id = ANY($1)",
        )
        .bind(&reply_ids[..])
        .fetch_all(db)
        .await?
    };

    let mut att_map: HashMap<i64, Vec<Attachment>> = HashMap::new();
    for a in attachments {
        att_map.entry(a.message_id).or_default().push(Attachment {
            id: a.id,
            url: attachment_url(&a.storage_key, &a.filename),
            filename: a.filename,
            content_type: a.content_type,
            size: a.size,
        });
    }
    let mut embed_map: HashMap<i64, Vec<Embed>> = HashMap::new();
    for e in embeds {
        embed_map.entry(e.message_id).or_default().push(Embed {
            url: e.url,
            kind: e.kind,
            title: e.title,
            description: e.description,
            site_name: e.site_name,
            image_url: e.image_url,
            color: e.color,
            author_name: e.author_name,
            author_url: e.author_url,
            author_icon_url: e.author_icon_url,
            footer: e.footer,
            thumbnail_url: e.thumbnail_url,
            timestamp: e.timestamp,
            fields: e.fields.map(|f| f.0).unwrap_or_default(),
        });
    }
    let mut event_map = crate::api::events::events_for_messages(db, &ids).await?;
    let mut lfg_map = crate::api::lfg::groups_for_messages(db, &ids).await?;
    let thread_sql = format!("{THREAD_SELECT} WHERE starter_message_id = ANY($1)");
    let mut thread_map: HashMap<i64, ThreadInfo> = sqlx::query_as::<_, ThreadInfo>(&thread_sql)
        .bind(&ids[..])
        .fetch_all(db)
        .await?
        .into_iter()
        .filter_map(|t| t.starter_message_id.map(|m| (m, t)))
        .collect();
    let polls = sqlx::query_as::<_, PollRow>(
        "SELECT message_id, question, options, multi, ends_at, closed
         FROM polls WHERE message_id = ANY($1)",
    )
    .bind(&ids[..])
    .fetch_all(db)
    .await?;
    let mut poll_map: HashMap<i64, Poll> = HashMap::new();
    if !polls.is_empty() {
        let poll_ids: Vec<i64> = polls.iter().map(|p| p.message_id).collect();
        let votes = sqlx::query_as::<_, VoteRow>(
            "SELECT message_id, option_idx, array_agg(user_id ORDER BY voted_at, user_id) AS user_ids
             FROM poll_votes WHERE message_id = ANY($1)
             GROUP BY message_id, option_idx",
        )
        .bind(&poll_ids[..])
        .fetch_all(db)
        .await?;
        for p in polls {
            let options = p
                .options
                .into_iter()
                .enumerate()
                .map(|(i, text)| PollOption {
                    text,
                    user_ids: votes
                        .iter()
                        .find(|v| v.message_id == p.message_id && v.option_idx == i as i32)
                        .map(|v| v.user_ids.clone())
                        .unwrap_or_default(),
                })
                .collect();
            poll_map.insert(
                p.message_id,
                Poll {
                    question: p.question,
                    options,
                    multi: p.multi,
                    ends_at: p.ends_at,
                    closed: p.closed,
                },
            );
        }
    }
    let mut reaction_map: HashMap<i64, Vec<Reaction>> = HashMap::new();
    for r in reactions {
        reaction_map
            .entry(r.message_id)
            .or_default()
            .push(Reaction {
                emoji: r.emoji,
                user_ids: r.user_ids,
            });
    }
    let reply_map: HashMap<i64, MessageRef> = replies
        .into_iter()
        .map(|r| {
            (
                r.id,
                MessageRef {
                    id: r.id,
                    author_id: r.author_id,
                    author_display_name: r.author_display_name,
                    content: r.content.chars().take(120).collect(),
                },
            )
        })
        .collect();

    Ok(rows
        .into_iter()
        .map(|r| Message {
            reply_to: r.reply_to_id.and_then(|id| reply_map.get(&id).cloned()),
            attachments: att_map.remove(&r.id).unwrap_or_default(),
            embeds: embed_map.remove(&r.id).unwrap_or_default(),
            reactions: reaction_map.remove(&r.id).unwrap_or_default(),
            id: r.id,
            channel_id: r.channel_id,
            author_id: r.author_id,
            author_username: r.author_username,
            author_display_name: r.author_display_name,
            author_avatar_url: r.author_avatar_url,
            author_bot: r.author_bot,
            content: r.content,
            created_at: r.created_at,
            edited_at: r.edited_at,
            interaction: r.interaction.map(|i| i.0),
            event: event_map.remove(&r.id),
            pinned_at: r.pinned_at,
            thread_id: r.thread_id,
            thread: thread_map.remove(&r.id),
            poll: poll_map.remove(&r.id),
            lfg: lfg_map.remove(&r.id),
        })
        .collect())
}

pub async fn fetch_message(db: &PgPool, message_id: i64) -> AppResult<Message> {
    let sql = format!("{MESSAGE_SELECT} WHERE m.id = $1");
    let row = sqlx::query_as::<_, MessageRow>(&sql)
        .bind(message_id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)?;
    hydrate(db, vec![row])
        .await?
        .pop()
        .ok_or(AppError::NotFound)
}

/// Loads a thread and checks it belongs to the channel.
pub(crate) async fn thread_in_channel(
    db: &PgPool,
    channel_id: i64,
    thread_id: i64,
) -> AppResult<ThreadInfo> {
    let sql = format!("{THREAD_SELECT} WHERE id = $1");
    let thread = sqlx::query_as::<_, ThreadInfo>(&sql)
        .bind(thread_id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)?;
    if thread.channel_id != channel_id {
        return Err(AppError::NotFound);
    }
    Ok(thread)
}

/// Loads a message and checks it belongs to the channel.
async fn message_in_channel(db: &PgPool, channel_id: i64, message_id: i64) -> AppResult<Message> {
    let msg = fetch_message(db, message_id).await?;
    if msg.channel_id != channel_id {
        return Err(AppError::NotFound);
    }
    Ok(msg)
}

/// Saves embeds for a message (replacing nothing: positions continue after
/// any existing ones).
pub async fn insert_embeds(db: &PgPool, message_id: i64, embeds: &[Embed]) -> AppResult<()> {
    let start: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM message_embeds WHERE message_id = $1",
    )
    .bind(message_id)
    .fetch_one(db)
    .await?;
    for (i, e) in embeds.iter().enumerate() {
        // Bots, webhooks and news feeds can send anything: only web links
        // (never javascript: and friends) and sensible sizes are kept.
        let e = &crate::security::clean_embed(e);
        sqlx::query(
            "INSERT INTO message_embeds
                 (message_id, position, url, kind, title, description, site_name, image_url,
                  color, author_name, author_url, author_icon_url, footer, thumbnail_url,
                  timestamp, fields)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(message_id)
        .bind(start + i as i32)
        .bind(&e.url)
        .bind(&e.kind)
        .bind(&e.title)
        .bind(&e.description)
        .bind(&e.site_name)
        .bind(&e.image_url)
        .bind(e.color)
        .bind(&e.author_name)
        .bind(&e.author_url)
        .bind(&e.author_icon_url)
        .bind(&e.footer)
        .bind(&e.thumbnail_url)
        .bind(&e.timestamp)
        .bind(if e.fields.is_empty() {
            None
        } else {
            Some(SqlJson(e.fields.clone()))
        })
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Posts a message as a bot user (webhooks and news feeds).
#[allow(clippy::too_many_arguments)]
pub async fn post_bot_message(
    st: &AppState,
    guild_id: i64,
    channel_id: i64,
    author_id: i64,
    override_name: Option<String>,
    override_avatar: Option<String>,
    content: String,
    embeds: Vec<Embed>,
) -> AppResult<Message> {
    post_bot_message_full(
        st,
        guild_id,
        channel_id,
        author_id,
        override_name,
        override_avatar,
        content,
        embeds,
        None,
    )
    .await
}

/// Like [`post_bot_message`], optionally marked as a reply to a slash command.
#[allow(clippy::too_many_arguments)]
pub async fn post_bot_message_full(
    st: &AppState,
    guild_id: i64,
    channel_id: i64,
    author_id: i64,
    override_name: Option<String>,
    override_avatar: Option<String>,
    content: String,
    embeds: Vec<Embed>,
    interaction: Option<MessageInteraction>,
) -> AppResult<Message> {
    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO messages
             (id, channel_id, author_id, content, override_name, override_avatar, interaction)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(channel_id)
    .bind(author_id)
    .bind(&content)
    .bind(&override_name)
    .bind(&override_avatar)
    .bind(interaction.map(SqlJson))
    .execute(&st.db)
    .await?;
    insert_embeds(&st.db, id, &embeds).await?;
    sqlx::query("UPDATE channels SET last_message_id = $2 WHERE id = $1")
        .bind(channel_id)
        .bind(id)
        .execute(&st.db)
        .await?;
    let message = fetch_message(&st.db, id).await?;
    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id,
        },
        ServerEvent::MessageCreate(message.clone()),
    );
    crate::push::spawn_message_push(st, guild_id, &message);
    if embeds.is_empty() {
        embeds::spawn_previews(st.clone(), guild_id, channel_id, id, content);
    }
    Ok(message)
}

// ---- History -----------------------------------------------------------

#[derive(Deserialize)]
pub struct HistoryQuery {
    /// Only messages older than this ID (for scrolling back).
    before: Option<String>,
    /// Messages around this ID (half older, half newer), for jumping to an
    /// old message from search or pins.
    around: Option<String>,
    /// Messages in this thread of the channel (otherwise: the channel itself).
    thread: Option<String>,
    limit: Option<i64>,
}

fn parse_id(raw: Option<&str>, what: &str) -> AppResult<Option<i64>> {
    raw.map(|s| {
        s.trim()
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest(format!("`{what}` must be a message ID.")))
    })
    .transpose()
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Query(q): Query<HistoryQuery>,
) -> AppResult<Json<Vec<Message>>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::READ_HISTORY)?;
    let before = parse_id(q.before.as_deref(), "before")?;
    let around = parse_id(q.around.as_deref(), "around")?;
    let thread = parse_id(q.thread.as_deref(), "thread")?;
    if let Some(thread_id) = thread {
        thread_in_channel(&st.db, channel_id, thread_id).await?;
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    if let Some(around) = around {
        // The message itself and older ones, then newer ones.
        let older_sql = format!(
            "{MESSAGE_SELECT}
             WHERE m.channel_id = $1 AND m.id <= $2 AND m.thread_id IS NOT DISTINCT FROM $4::BIGINT
             ORDER BY m.id DESC LIMIT $3"
        );
        let newer_sql = format!(
            "{MESSAGE_SELECT}
             WHERE m.channel_id = $1 AND m.id > $2 AND m.thread_id IS NOT DISTINCT FROM $4::BIGINT
             ORDER BY m.id ASC LIMIT $3"
        );
        let newer_count = limit / 2;
        let mut rows = sqlx::query_as::<_, MessageRow>(&older_sql)
            .bind(channel_id)
            .bind(around)
            .bind(limit - newer_count)
            .bind(thread)
            .fetch_all(&st.db)
            .await?;
        rows.reverse();
        let newer = sqlx::query_as::<_, MessageRow>(&newer_sql)
            .bind(channel_id)
            .bind(around)
            .bind(newer_count.max(1))
            .bind(thread)
            .fetch_all(&st.db)
            .await?;
        rows.extend(newer);
        return Ok(Json(hydrate(&st.db, rows).await?));
    }
    let sql = format!(
        "{MESSAGE_SELECT}
         WHERE m.channel_id = $1 AND ($2::BIGINT IS NULL OR m.id < $2)
           AND m.thread_id IS NOT DISTINCT FROM $4::BIGINT
         ORDER BY m.id DESC LIMIT $3"
    );
    let mut rows = sqlx::query_as::<_, MessageRow>(&sql)
        .bind(channel_id)
        .bind(before)
        .bind(limit)
        .bind(thread)
        .fetch_all(&st.db)
        .await?;
    rows.reverse(); // oldest first, ready to display
    Ok(Json(hydrate(&st.db, rows).await?))
}

// ---- Sending -----------------------------------------------------------

struct PendingFile {
    storage_key: String,
    filename: String,
    content_type: String,
    data: axum::body::Bytes,
}

fn check_content(content: &str, allow_empty: bool) -> AppResult<()> {
    if content.is_empty() && !allow_empty {
        return Err(AppError::BadRequest("Messages can't be empty.".into()));
    }
    if content.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::BadRequest(format!(
            "Messages can be up to {MAX_MESSAGE_CHARS} characters."
        )));
    }
    Ok(())
}

async fn create_message(
    st: &AppState,
    access: &ChannelAccess,
    user: &User,
    content: String,
    reply_to_id: Option<i64>,
    thread_id: Option<i64>,
    files: Vec<PendingFile>,
) -> AppResult<Message> {
    let channel_id = access.channel.id;
    let guild_id = access.channel.guild_id;
    let content = content.trim().to_string();
    check_content(&content, !files.is_empty())?;
    let content = crate::api::emojis::expand(&st.db, guild_id, user.id, &content).await?;
    if let Some(thread_id) = thread_id {
        thread_in_channel(&st.db, channel_id, thread_id)
            .await
            .map_err(|_| AppError::BadRequest("That thread is gone.".into()))?;
    }

    if let Some(reply_id) = reply_to_id {
        let reply_channel: Option<i64> =
            sqlx::query_scalar("SELECT channel_id FROM messages WHERE id = $1")
                .bind(reply_id)
                .fetch_optional(&st.db)
                .await?;
        if reply_channel != Some(channel_id) {
            return Err(AppError::BadRequest(
                "The message you're replying to is gone.".into(),
            ));
        }
    }

    check_slowmode(st, access, user).await?;

    // Files go to disk first, so a failed write never leaves a broken message.
    if !files.is_empty() {
        tokio::fs::create_dir_all(&st.config.upload_dir)
            .await
            .map_err(|e| anyhow::anyhow!("creating upload folder: {e}"))?;
        for f in &files {
            tokio::fs::write(st.config.upload_dir.join(&f.storage_key), &f.data)
                .await
                .map_err(|e| anyhow::anyhow!("saving upload: {e}"))?;
        }
    }

    let id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO messages (id, channel_id, author_id, content, reply_to_id, thread_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(channel_id)
    .bind(user.id)
    .bind(&content)
    .bind(reply_to_id)
    .bind(thread_id)
    .execute(&mut *tx)
    .await?;
    for f in &files {
        sqlx::query(
            "INSERT INTO attachments (id, message_id, storage_key, filename, content_type, size)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(st.ids.next_id())
        .bind(id)
        .bind(&f.storage_key)
        .bind(&f.filename)
        .bind(&f.content_type)
        .bind(f.data.len() as i64)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(thread_id) = thread_id {
        // Thread replies don't make the channel itself unread.
        sqlx::query(
            "UPDATE threads SET message_count = message_count + 1, last_message_id = $2
             WHERE id = $1",
        )
        .bind(thread_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query("UPDATE channels SET last_message_id = $2 WHERE id = $1")
            .bind(channel_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // Your own messages count as read.
        sqlx::query(
            "INSERT INTO read_states (user_id, channel_id, last_read_id) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, channel_id) DO UPDATE SET last_read_id = EXCLUDED.last_read_id",
        )
        .bind(user.id)
        .bind(channel_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let message = fetch_message(&st.db, id).await?;
    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id,
        },
        ServerEvent::MessageCreate(message.clone()),
    );
    crate::push::spawn_message_push(st, guild_id, &message);
    if let Some(thread_id) = thread_id {
        announce_thread(st, guild_id, channel_id, thread_id).await?;
    }
    if access.perms & p::EMBED_LINKS != 0 {
        embeds::spawn_previews(st.clone(), guild_id, channel_id, id, content);
    }
    Ok(message)
}

/// Slowmode: one message per member every `slowmode_secs` (Manage
/// Messages, Manage Channels and bots skip it). Claims the slot atomically,
/// so two messages sent at once can't both get through.
async fn check_slowmode(st: &AppState, access: &ChannelAccess, user: &User) -> AppResult<()> {
    let secs = access.channel.slowmode_secs;
    if secs <= 0 || user.bot || access.perms & (p::MANAGE_MESSAGES | p::MANAGE_CHANNELS) != 0 {
        return Ok(());
    }
    let claimed: Option<i32> = sqlx::query_scalar(
        "INSERT INTO slowmode_posts (channel_id, user_id, posted_at) VALUES ($1, $2, now())
         ON CONFLICT (channel_id, user_id) DO UPDATE SET posted_at = now()
         WHERE slowmode_posts.posted_at <= now() - ($3::INT * INTERVAL '1 second')
         RETURNING 1",
    )
    .bind(access.channel.id)
    .bind(user.id)
    .bind(secs)
    .fetch_optional(&st.db)
    .await?;
    if claimed.is_some() {
        return Ok(());
    }
    let left: Option<i32> = sqlx::query_scalar(
        "SELECT GREATEST(1, CEIL(EXTRACT(EPOCH FROM
                    (posted_at + ($3::INT * INTERVAL '1 second') - now())))::INT)
         FROM slowmode_posts WHERE channel_id = $1 AND user_id = $2",
    )
    .bind(access.channel.id)
    .bind(user.id)
    .bind(secs)
    .fetch_optional(&st.db)
    .await?;
    Err(AppError::BadRequest(format!(
        "Slowmode is on. You can post again in {}s.",
        left.unwrap_or(secs)
    )))
}

pub async fn send(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateMessageRequest>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    let msg = create_message(
        &st,
        &access,
        &user,
        req.content,
        req.reply_to_id,
        req.thread_id,
        Vec::new(),
    )
    .await?;
    Ok(Json(msg))
}

/// Keeps a safe, readable version of an uploaded file's name.
fn clean_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

fn guess_content_type(filename: &str) -> &'static str {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "txt" | "log" => "text/plain",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn upload_error(e: impl std::fmt::Display) -> AppError {
    AppError::BadRequest(format!("Upload failed: {e}"))
}

/// Multipart form: `content` (text), optional `reply_to_id`, and up to
/// ten `files`.
pub async fn upload(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    mut form: Multipart,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES | p::ATTACH_FILES)?;

    let mut content = String::new();
    let mut reply_to_id = None;
    let mut thread_id = None;
    let mut files = Vec::new();
    while let Some(field) = form.next_field().await.map_err(upload_error)? {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "content" => content = field.text().await.map_err(upload_error)?,
            "reply_to_id" => {
                let text = field.text().await.map_err(upload_error)?;
                let text = text.trim();
                if !text.is_empty() {
                    reply_to_id = Some(
                        text.parse::<i64>()
                            .map_err(|_| AppError::BadRequest("Bad reply_to_id.".into()))?,
                    );
                }
            }
            "thread_id" => {
                let text = field.text().await.map_err(upload_error)?;
                let text = text.trim();
                if !text.is_empty() {
                    thread_id = Some(
                        text.parse::<i64>()
                            .map_err(|_| AppError::BadRequest("Bad thread_id.".into()))?,
                    );
                }
            }
            "files" => {
                if files.len() >= MAX_FILES {
                    return Err(AppError::BadRequest(format!(
                        "Up to {MAX_FILES} files per message."
                    )));
                }
                let filename = clean_filename(field.file_name().unwrap_or("file"));
                let content_type = guess_content_type(&filename).to_string();
                let data = field.bytes().await.map_err(upload_error)?;
                if data.len() > MAX_FILE_BYTES {
                    return Err(AppError::BadRequest(format!(
                        "{filename} is too big (25 MB max)."
                    )));
                }
                files.push(PendingFile {
                    storage_key: auth::random_code(24, KEY_ALPHABET),
                    filename,
                    content_type,
                    data,
                });
            }
            _ => {}
        }
    }
    if files.is_empty() && content.trim().is_empty() {
        return Err(AppError::BadRequest("Nothing to send.".into()));
    }
    let msg = create_message(&st, &access, &user, content, reply_to_id, thread_id, files).await?;
    Ok(Json(msg))
}

// ---- Editing and deleting ------------------------------------------------

pub async fn edit(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    Json(req): Json<EditMessageRequest>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    let original = message_in_channel(&st.db, channel_id, message_id).await?;
    if original.author_id != user.id {
        return Err(AppError::Forbidden);
    }
    let content = req.content.trim().to_string();
    check_content(&content, !original.attachments.is_empty())?;
    let content =
        crate::api::emojis::expand(&st.db, access.channel.guild_id, user.id, &content).await?;

    sqlx::query("UPDATE messages SET content = $2, edited_at = now() WHERE id = $1")
        .bind(message_id)
        .bind(&content)
        .execute(&st.db)
        .await?;
    sqlx::query("DELETE FROM message_embeds WHERE message_id = $1")
        .bind(message_id)
        .execute(&st.db)
        .await?;

    let message = fetch_message(&st.db, message_id).await?;
    let guild_id = access.channel.guild_id;
    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id,
        },
        ServerEvent::MessageUpdate(message.clone()),
    );
    if access.perms & p::EMBED_LINKS != 0 {
        embeds::spawn_previews(st.clone(), guild_id, channel_id, message_id, content);
    }
    Ok(Json(message))
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    let msg = message_in_channel(&st.db, channel_id, message_id).await?;
    if msg.author_id != user.id {
        access.require(p::MANAGE_MESSAGES)?;
    }
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT storage_key FROM attachments WHERE message_id = $1")
            .bind(message_id)
            .fetch_all(&st.db)
            .await?;
    sqlx::query("DELETE FROM messages WHERE id = $1")
        .bind(message_id)
        .execute(&st.db)
        .await?;
    for key in keys {
        let _ = tokio::fs::remove_file(st.config.upload_dir.join(key)).await;
    }
    if msg.author_id != user.id {
        crate::audit::Entry::new(access.channel.guild_id, user.id, "message_delete")
            .target("member", msg.author_id, msg.author_display_name.clone())
            .details(format!(
                "In #{}: {}",
                access.channel.name,
                snippet(&msg.content, msg.attachments.len())
            ))
            .save(&st)
            .await;
    }
    if let Some(thread_id) = msg.thread_id {
        sqlx::query(
            "UPDATE threads SET message_count = GREATEST(message_count - 1, 0),
                    last_message_id = (SELECT max(id) FROM messages WHERE thread_id = $1)
             WHERE id = $1",
        )
        .bind(thread_id)
        .execute(&st.db)
        .await?;
        announce_thread(&st, access.channel.guild_id, channel_id, thread_id).await?;
    }
    st.dispatch(
        Audience::Channel {
            guild_id: access.channel.guild_id,
            channel_id,
        },
        ServerEvent::MessageDelete {
            id: message_id,
            channel_id,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

// ---- Reactions and typing ----------------------------------------------

fn check_emoji(emoji: &str) -> AppResult<()> {
    if emoji.is_empty() || emoji.len() > 64 || emoji.chars().any(char::is_whitespace) {
        return Err(AppError::BadRequest("That's not an emoji.".into()));
    }
    Ok(())
}

/// Custom emoji reactions (`<:name:id>`) must be real ones.
async fn check_custom_reaction(db: &PgPool, emoji: &str) -> AppResult<()> {
    if !emoji.starts_with('<') {
        if emoji.len() > 32 {
            return Err(AppError::BadRequest("That's not an emoji.".into()));
        }
        return Ok(());
    }
    let (name, id) = crate::api::emojis::parse_token(emoji)
        .ok_or_else(|| AppError::BadRequest("That's not an emoji.".into()))?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM emojis WHERE id = $1 AND name = $2)")
            .bind(id)
            .bind(name)
            .fetch_one(db)
            .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::BadRequest("That emoji has been deleted.".into()))
    }
}

/// Tells everyone who can see the channel about a thread's new reply count.
pub(crate) async fn announce_thread(
    st: &AppState,
    guild_id: i64,
    channel_id: i64,
    thread_id: i64,
) -> AppResult<()> {
    if let Ok(thread) = thread_in_channel(&st.db, channel_id, thread_id).await {
        st.dispatch(
            Audience::Channel {
                guild_id,
                channel_id,
            },
            ServerEvent::ThreadUpdate(thread),
        );
    }
    Ok(())
}

pub async fn add_reaction(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id, emoji)): Path<(i64, i64, String)>,
) -> AppResult<StatusCode> {
    check_emoji(&emoji)?;
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::ADD_REACTIONS)?;
    check_custom_reaction(&st.db, &emoji).await?;
    message_in_channel(&st.db, channel_id, message_id).await?;
    let added = sqlx::query(
        "INSERT INTO reactions (message_id, user_id, emoji) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(message_id)
    .bind(user.id)
    .bind(&emoji)
    .execute(&st.db)
    .await?
    .rows_affected()
        == 1;
    if added {
        st.dispatch(
            Audience::Channel {
                guild_id: access.channel.guild_id,
                channel_id,
            },
            ServerEvent::ReactionAdd {
                channel_id,
                message_id,
                user_id: user.id,
                emoji,
            },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_reaction(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id, emoji)): Path<(i64, i64, String)>,
) -> AppResult<StatusCode> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    let removed =
        sqlx::query("DELETE FROM reactions WHERE message_id = $1 AND user_id = $2 AND emoji = $3")
            .bind(message_id)
            .bind(user.id)
            .bind(&emoji)
            .execute(&st.db)
            .await?
            .rows_affected()
            == 1;
    if removed {
        st.dispatch(
            Audience::Channel {
                guild_id: access.channel.guild_id,
                channel_id,
            },
            ServerEvent::ReactionRemove {
                channel_id,
                message_id,
                user_id: user.id,
                emoji,
            },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn typing(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<StatusCode> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    st.dispatch(
        Audience::Channel {
            guild_id: access.channel.guild_id,
            channel_id,
        },
        ServerEvent::TypingStart {
            channel_id,
            user_id: user.id,
            display_name: user.display_name,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

// ---- Pins ------------------------------------------------------------------

/// Most pins a channel can have.
pub const MAX_PINS: i64 = 50;

/// `GET /api/channels/{id}/pins`: pinned messages, most recently pinned first.
pub async fn list_pins(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<Vec<Message>>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::READ_HISTORY)?;
    let sql = format!(
        "{MESSAGE_SELECT}
         WHERE m.channel_id = $1 AND m.pinned_at IS NOT NULL
         ORDER BY m.pinned_at DESC, m.id DESC LIMIT 100"
    );
    let rows = sqlx::query_as::<_, MessageRow>(&sql)
        .bind(channel_id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(hydrate(&st.db, rows).await?))
}

async fn set_pinned(
    st: &AppState,
    user: &User,
    channel_id: i64,
    message_id: i64,
    pinned: bool,
) -> AppResult<Message> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::MANAGE_MESSAGES)?;
    let msg = message_in_channel(&st.db, channel_id, message_id).await?;
    if msg.pinned_at.is_some() == pinned {
        return Ok(msg);
    }
    if pinned {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM messages WHERE channel_id = $1 AND pinned_at IS NOT NULL",
        )
        .bind(channel_id)
        .fetch_one(&st.db)
        .await?;
        if count >= MAX_PINS {
            return Err(AppError::BadRequest(format!(
                "This channel already has {MAX_PINS} pins. Unpin one first."
            )));
        }
        sqlx::query("UPDATE messages SET pinned_at = now(), pinned_by = $2 WHERE id = $1")
            .bind(message_id)
            .bind(user.id)
            .execute(&st.db)
            .await?;
    } else {
        sqlx::query("UPDATE messages SET pinned_at = NULL, pinned_by = NULL WHERE id = $1")
            .bind(message_id)
            .execute(&st.db)
            .await?;
    }
    let message = fetch_message(&st.db, message_id).await?;
    st.dispatch(
        Audience::Channel {
            guild_id: access.channel.guild_id,
            channel_id,
        },
        ServerEvent::MessageUpdate(message.clone()),
    );
    crate::audit::Entry::new(
        access.channel.guild_id,
        user.id,
        if pinned {
            "message_pin"
        } else {
            "message_unpin"
        },
    )
    .target("message", message_id, format!("#{}", access.channel.name))
    .details(format!(
        "{}: {}",
        message.author_display_name,
        snippet(&message.content, message.attachments.len())
    ))
    .save(st)
    .await;
    Ok(message)
}

/// A short quote of a message for the audit log.
fn snippet(content: &str, attachments: usize) -> String {
    let text: String = content.chars().take(120).collect();
    let text = if content.chars().count() > 120 {
        format!("“{text}…”")
    } else if text.is_empty() {
        String::new()
    } else {
        format!("“{text}”")
    };
    match (text.is_empty(), attachments) {
        (true, 0) => "(empty)".into(),
        (true, n) => format!("{n} attachment(s)"),
        (false, 0) => text,
        (false, n) => format!("{text} + {n} attachment(s)"),
    }
}

/// `PUT /api/channels/{id}/pins/{message}` (needs Manage Messages).
pub async fn pin(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Json<Message>> {
    Ok(Json(
        set_pinned(&st, &user, channel_id, message_id, true).await?,
    ))
}

/// `DELETE /api/channels/{id}/pins/{message}` (needs Manage Messages).
pub async fn unpin(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Json<Message>> {
    Ok(Json(
        set_pinned(&st, &user, channel_id, message_id, false).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_are_cleaned() {
        assert_eq!(clean_filename("../../etc/passwd"), "passwd");
        assert_eq!(
            clean_filename("C:\\Users\\dan\\raid plan.png"),
            "raid plan.png"
        );
        assert_eq!(clean_filename("<script>.html"), "_script_.html");
        assert_eq!(clean_filename(".."), "file");
    }

    #[test]
    fn urls_are_encoded() {
        assert_eq!(
            attachment_url("abc", "raid plan.png"),
            "/files/abc/raid%20plan.png"
        );
    }
}
