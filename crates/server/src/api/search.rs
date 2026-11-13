//! Message search (Postgres full-text) and unread/mention counts.

use axum::extract::{Path, Query, State};
use axum::Json;
use outhouse_common::models::{
    ChannelUnread, GuildRoleIds, SearchResults, ThreadUnread, UnreadState,
};
use outhouse_common::permissions as p;
use serde::Deserialize;
use sqlx::{Postgres, QueryBuilder};

use crate::api::messages::{hydrate, MessageRow, MESSAGE_SELECT};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{member_ctx, visible_text_channels};
use crate::state::AppState;

const MAX_QUERY_CHARS: usize = 200;
const MAX_WORDS: usize = 10;

// ---- Parsing ---------------------------------------------------------------

/// A search box query split into words and filters.
#[derive(Debug, Default, PartialEq)]
pub struct ParsedQuery {
    /// Lower-case words to find (each matches as a prefix).
    pub words: Vec<String>,
    /// `from:@name` (lower case, without the @), or a user ID.
    pub from: Vec<String>,
    /// `in:#channel` (lower case, without the #).
    pub channels: Vec<String>,
    /// `has:image`, `has:video`, `has:file` or `has:link`.
    pub has: Vec<&'static str>,
}

impl ParsedQuery {
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
            && self.from.is_empty()
            && self.channels.is_empty()
            && self.has.is_empty()
    }

    /// The words as a Postgres tsquery (`raid:* & tonight:*`). Words only
    /// hold letters and digits, so this can't be used to inject syntax.
    pub fn tsquery(&self) -> Option<String> {
        if self.words.is_empty() {
            None
        } else {
            Some(
                self.words
                    .iter()
                    .map(|w| format!("{w}:*"))
                    .collect::<Vec<_>>()
                    .join(" & "),
            )
        }
    }
}

/// Splits on spaces, keeping "quoted bits" together (quotes removed).
fn tokenize(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in q.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if c.is_whitespace() && !quoted {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

pub fn parse_query(q: &str) -> ParsedQuery {
    let mut parsed = ParsedQuery::default();
    for token in tokenize(q) {
        let lower = token.to_lowercase();
        if let Some(v) = lower.strip_prefix("from:") {
            let v = v.trim();
            // A mention token (<@123>) or a name, with or without the @.
            let v = v
                .strip_prefix("<@")
                .and_then(|x| x.strip_suffix('>'))
                .unwrap_or(v)
                .trim_start_matches('@')
                .trim();
            if !v.is_empty() {
                parsed.from.push(v.to_string());
            }
        } else if let Some(v) = lower.strip_prefix("in:") {
            let v = v.trim().trim_start_matches('#').trim();
            if !v.is_empty() {
                parsed.channels.push(v.to_string());
            }
        } else if let Some(v) = lower.strip_prefix("has:") {
            let kind = match v.trim() {
                "image" | "images" | "img" | "picture" | "gif" => Some("image"),
                "video" | "videos" => Some("video"),
                "file" | "files" | "attachment" | "attachments" => Some("file"),
                "link" | "links" | "url" => Some("link"),
                _ => None,
            };
            if let Some(kind) = kind {
                if !parsed.has.contains(&kind) {
                    parsed.has.push(kind);
                }
            }
        } else {
            for word in lower.split(|c: char| !c.is_alphanumeric()) {
                if !word.is_empty() && parsed.words.len() < MAX_WORDS {
                    parsed.words.push(word.chars().take(50).collect());
                }
            }
        }
    }
    parsed
}

// ---- Search ----------------------------------------------------------------

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// `GET /api/guilds/{id}/search?q=...`: messages in the channels you can
/// read in this outhouse, newest first.
pub async fn search(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<SearchResults>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let raw = query.q.unwrap_or_default();
    if raw.chars().count() > MAX_QUERY_CHARS {
        return Err(AppError::BadRequest("That search is too long.".into()));
    }
    let parsed = parse_query(&raw);
    if parsed.is_empty() {
        return Err(AppError::BadRequest("Type something to search for.".into()));
    }
    let limit = query.limit.unwrap_or(25).clamp(1, 50);
    let offset = query.offset.unwrap_or(0).clamp(0, 1000);
    let empty = |parsed: &ParsedQuery| SearchResults {
        total: 0,
        messages: Vec::new(),
        terms: parsed.words.clone(),
    };

    let mut channel_ids = visible_text_channels(&st.db, &ctx, p::READ_HISTORY).await?;
    if !parsed.channels.is_empty() {
        let named: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM channels WHERE guild_id = $1 AND kind = 0 AND lower(name) = ANY($2)",
        )
        .bind(guild_id)
        .bind(&parsed.channels)
        .fetch_all(&st.db)
        .await?;
        channel_ids.retain(|id| named.contains(id));
    }
    // Age-restricted channels you haven't said you're 18+ for stay out.
    crate::perms::drop_age_gated(&st.db, user.id, &mut channel_ids).await?;
    if channel_ids.is_empty() {
        return Ok(Json(empty(&parsed)));
    }

    let mut authors: Vec<i64> = Vec::new();
    if !parsed.from.is_empty() {
        let ids: Vec<i64> = parsed.from.iter().filter_map(|f| f.parse().ok()).collect();
        authors = sqlx::query_scalar(
            "SELECT gm.user_id FROM guild_members gm
             JOIN users u ON u.id = gm.user_id
             WHERE gm.guild_id = $1
               AND (gm.user_id = ANY($2) OR u.username = ANY($3)
                    OR lower(u.display_name) = ANY($3) OR lower(gm.nickname) = ANY($3))",
        )
        .bind(guild_id)
        .bind(&ids)
        .bind(&parsed.from)
        .fetch_all(&st.db)
        .await?;
        if authors.is_empty() {
            return Ok(Json(empty(&parsed)));
        }
    }

    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT m.id, count(*) OVER () AS total FROM messages m WHERE NOT m.safety_hidden AND m.channel_id = ANY(",
    );
    qb.push_bind(channel_ids);
    qb.push(")");
    if let Some(tsquery) = parsed.tsquery() {
        qb.push(" AND to_tsvector('simple', m.content) @@ to_tsquery('simple', ");
        qb.push_bind(tsquery);
        qb.push(")");
    }
    if !authors.is_empty() {
        qb.push(" AND m.author_id = ANY(");
        qb.push_bind(authors);
        qb.push(")");
    }
    for kind in &parsed.has {
        let condition = match *kind {
            "image" => {
                " AND (EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id AND a.content_type LIKE 'image/%')
                   OR EXISTS (SELECT 1 FROM message_embeds e WHERE e.message_id = m.id AND e.kind = 'image'))"
            }
            "video" => {
                " AND EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id AND a.content_type LIKE 'video/%')"
            }
            "file" => " AND EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id)",
            _ => " AND m.content ~* 'https?://'",
        };
        qb.push(condition);
    }
    qb.push(" ORDER BY m.id DESC LIMIT ");
    qb.push_bind(limit);
    qb.push(" OFFSET ");
    qb.push_bind(offset);
    let hits: Vec<(i64, i64)> = qb.build_query_as::<(i64, i64)>().fetch_all(&st.db).await?;

    let total = hits.first().map(|h| h.1).unwrap_or(0);
    let ids: Vec<i64> = hits.iter().map(|h| h.0).collect();
    let sql = format!("{MESSAGE_SELECT} WHERE m.id = ANY($1) ORDER BY m.id DESC");
    let rows = sqlx::query_as::<_, MessageRow>(&sql)
        .bind(&ids)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(SearchResults {
        total,
        messages: hydrate(&st.db, rows).await?,
        terms: parsed.words,
    }))
}

// ---- Unread and mentions -----------------------------------------------------

/// Counts unread messages that mention you: `<@you>`, one of your roles,
/// @everyone/@here, or a reply to one of your messages. Before you've opened
/// a channel, only messages since you joined count.
const UNREAD_SQL: &str = r#"
    SELECT c.id AS channel_id, c.guild_id, c.last_message_id, rs.last_read_id,
           (SELECT count(*) FROM (
                SELECT 1 FROM messages m
                WHERE m.channel_id = c.id
                  AND m.thread_id IS NULL
                  AND m.author_id <> $2
                  AND m.id > COALESCE(rs.last_read_id, 0)
                  AND (rs.last_read_id IS NOT NULL OR m.created_at > gm.joined_at)
                  AND (strpos(m.content, $4) > 0
                       OR m.content ~ '(^|\s)@(everyone|here)\M'
                       OR EXISTS (SELECT 1 FROM unnest($3::TEXT[]) AS r(pat)
                                  WHERE strpos(m.content, r.pat) > 0)
                       OR EXISTS (SELECT 1 FROM messages parent
                                  WHERE parent.id = m.reply_to_id AND parent.author_id = $2))
                LIMIT 100) mentions) AS mention_count,
           0::BIGINT AS direct_count
    FROM channels c
    JOIN guild_members gm ON gm.guild_id = c.guild_id AND gm.user_id = $2
    LEFT JOIN read_states rs ON rs.channel_id = c.id AND rs.user_id = $2
    WHERE c.id = ANY($1)
    ORDER BY c.position, c.id"#;

const DM_UNREAD_SQL: &str = r#"
    SELECT c.id AS channel_id, 0::BIGINT AS guild_id, c.last_message_id, rs.last_read_id,
           (SELECT count(*) FROM (
                SELECT 1 FROM messages m
                WHERE m.channel_id = c.id
                  AND m.thread_id IS NULL
                  AND m.author_id <> $1
                  AND m.id > COALESCE(rs.last_read_id, 0)
                  AND m.created_at >= p.joined_at
                LIMIT 100) unread) AS mention_count,
           (SELECT count(*) FROM (
                SELECT 1 FROM messages m
                WHERE m.channel_id = c.id
                  AND m.thread_id IS NULL
                  AND m.author_id <> $1
                  AND m.id > COALESCE(rs.last_read_id, 0)
                  AND m.created_at >= p.joined_at
                  AND (strpos(m.content, '<@' || ($1::BIGINT)::TEXT || '>') > 0
                       OR m.content ~ '(^|\s)@(everyone|here)\M'
                       OR EXISTS (SELECT 1 FROM messages parent
                                  WHERE parent.id = m.reply_to_id AND parent.author_id = $1))
                LIMIT 100) pings) AS direct_count
    FROM dm_participants p
    JOIN channels c ON c.id = p.channel_id
    LEFT JOIN read_states rs ON rs.channel_id = c.id AND rs.user_id = $1
    WHERE p.user_id = $1"#;

/// Threads with replies you haven't read in these channels, with how many
/// of those mention you. Before you've opened a thread, only replies since
/// you joined the outhouse count (and threads only show up once someone
/// else has replied).
const THREAD_UNREAD_SQL: &str = r#"
    SELECT t.id AS thread_id, t.channel_id, c.guild_id, t.last_message_id, tr.last_read_id,
           (SELECT count(*) FROM (
                SELECT 1 FROM messages m
                WHERE m.thread_id = t.id
                  AND m.author_id <> $2
                  AND m.id > COALESCE(tr.last_read_id, 0)
                  AND (tr.last_read_id IS NOT NULL OR m.created_at > gm.joined_at)
                  AND (strpos(m.content, $4) > 0
                       OR m.content ~ '(^|\s)@(everyone|here)\M'
                       OR EXISTS (SELECT 1 FROM unnest($3::TEXT[]) AS r(pat)
                                  WHERE strpos(m.content, r.pat) > 0)
                       OR EXISTS (SELECT 1 FROM messages parent
                                  WHERE parent.id = m.reply_to_id AND parent.author_id = $2))
                LIMIT 100) mentions) AS mention_count
    FROM threads t
    JOIN channels c ON c.id = t.channel_id
    JOIN guild_members gm ON gm.guild_id = c.guild_id AND gm.user_id = $2
    LEFT JOIN thread_reads tr ON tr.thread_id = t.id AND tr.user_id = $2
    WHERE t.channel_id = ANY($1)
      AND t.last_message_id IS NOT NULL
      AND t.last_message_id > COALESCE(tr.last_read_id, 0)
      AND EXISTS (SELECT 1 FROM messages m
                  WHERE m.thread_id = t.id AND m.author_id <> $2
                    AND m.id > COALESCE(tr.last_read_id, 0)
                    AND (tr.last_read_id IS NOT NULL OR m.created_at > gm.joined_at))
    ORDER BY t.last_message_id DESC
    LIMIT 500"#;

/// `GET /api/users/@me/unread`: unread state and mention counts for every
/// text channel you can see, in all your outhouses.
pub async fn unread(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<UnreadState>> {
    let guild_ids: Vec<i64> =
        sqlx::query_scalar("SELECT guild_id FROM guild_members WHERE user_id = $1")
            .bind(user.id)
            .fetch_all(&st.db)
            .await?;
    let mut channels = Vec::new();
    let mut threads = Vec::new();
    let mut roles = Vec::new();
    for guild_id in guild_ids {
        let ctx = match member_ctx(&st.db, guild_id, user.id).await {
            Ok(ctx) => ctx,
            // Left in the meantime, or shut down by the site owner.
            Err(AppError::NotFound | AppError::Gone(_)) => continue,
            Err(e) => return Err(e),
        };
        let visible = visible_text_channels(&st.db, &ctx, 0).await?;
        if !visible.is_empty() {
            let role_patterns: Vec<String> =
                ctx.role_ids.iter().map(|id| format!("<@&{id}>")).collect();
            let rows = sqlx::query_as::<_, ChannelUnread>(UNREAD_SQL)
                .bind(&visible)
                .bind(user.id)
                .bind(&role_patterns)
                .bind(format!("<@{}>", user.id))
                .fetch_all(&st.db)
                .await?;
            channels.extend(rows);
            let thread_rows = sqlx::query_as::<_, ThreadUnread>(THREAD_UNREAD_SQL)
                .bind(&visible)
                .bind(user.id)
                .bind(&role_patterns)
                .bind(format!("<@{}>", user.id))
                .fetch_all(&st.db)
                .await?;
            threads.extend(thread_rows);
        }
        roles.push(GuildRoleIds {
            guild_id,
            role_ids: ctx.role_ids.clone(),
        });
    }
    // Direct messages: every unread message from someone else counts.
    let dms = sqlx::query_as::<_, ChannelUnread>(DM_UNREAD_SQL)
        .bind(user.id)
        .fetch_all(&st.db)
        .await?;
    channels.extend(dms);
    Ok(Json(UnreadState {
        channels,
        roles,
        threads,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_words_and_filters() {
        let q = parse_query(r#"Onyxia KEY from:@Dave in:#raids has:image has:pics from:"Alice W""#);
        assert_eq!(q.words, vec!["onyxia", "key"]);
        assert_eq!(q.from, vec!["dave", "alice w"]);
        assert_eq!(q.channels, vec!["raids"]);
        assert_eq!(q.has, vec!["image"]);
        assert_eq!(q.tsquery().unwrap(), "onyxia:* & key:*");
    }

    #[test]
    fn strips_query_syntax() {
        let q = parse_query("raid's & (loot) | !boss:*");
        assert_eq!(q.words, vec!["raid", "s", "loot", "boss"]);
        assert!(parse_query("   ").is_empty());
        assert_eq!(parse_query("from:<@123>").from, vec!["123"]);
    }
}
