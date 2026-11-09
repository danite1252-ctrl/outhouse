//! Automod: per-channel, opt-in checks on messages before they're saved.
//!
//! - **Word filter**: blocked words and phrases (case-insensitive, optional
//!   whole-word matching, `*` wildcards, optional leetspeak folding), from
//!   the channel's own list and/or the outhouse-wide list. A hit either
//!   blocks the message or lets it through hidden behind a "Filtered"
//!   reveal for everyone else.
//! - **Spam limits**: messages per N seconds, the same message three times
//!   in a minute, mentions / links / attachments per message, and an
//!   optional timeout after repeated blocks.
//!
//! People with Manage Messages (unless switched off), exempt roles and bots
//! (unless switched on) skip it. Blocks go to the audit log (with a short
//! snippet only) and optionally to a mod-log channel.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::id::{option_as_string, vec_as_string};
use outhouse_common::models::{channel_kind, Embed, EmbedField, User, DM_GUILD};
use outhouse_common::permissions as p;
use serde::{Deserialize, Serialize};
use sqlx::types::Json as SqlJson;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access_meta, member_ctx, ChannelAccess};
use crate::state::{AppState, Audience};

pub const MAX_WORDS: usize = 300;
pub const MAX_WORD_CHARS: usize = 60;
/// Blocks within this long count towards an automatic timeout.
const STRIKE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// The same message this many times within [`DUPLICATE_WINDOW`] is spam.
const DUPLICATE_LIMIT: usize = 3;
const DUPLICATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WordAction {
    /// Refuse the message (the sender keeps their text).
    #[default]
    Block,
    /// Let it through, hidden behind a "Filtered" reveal for others.
    Hide,
}

/// One channel's automod settings (`GET/PUT /api/channels/{id}/automod`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChannelAutomod {
    /// Word filter on.
    pub word_filter: bool,
    /// Blocked words and phrases; `*` matches any letters (`bad*`).
    pub words: Vec<String>,
    /// Also use the outhouse-wide list (Outhouse settings → Automod).
    pub use_default_words: bool,
    /// Only whole words count ("ass" doesn't catch "class").
    pub whole_word: bool,
    /// Treat 0→o, 1→i, 3→e, 4→a, 5→s, 7→t, @→a, $→s as letters.
    pub leetspeak: bool,
    pub word_action: WordAction,
    /// Spam limits on.
    pub spam_filter: bool,
    /// At most this many messages per `per_seconds` from one person (0 = no limit).
    pub max_messages: u32,
    pub per_seconds: u32,
    /// Block the same message sent three times within a minute.
    pub block_duplicates: bool,
    /// Most @mentions in one message (0 = no limit).
    pub max_mentions: u32,
    /// Most links in one message (0 = no limit).
    pub max_links: u32,
    /// Most attachments in one message (0 = no limit).
    pub max_attachments: u32,
    /// Time someone out for this long after `timeout_after` blocks within
    /// ten minutes (0 = never).
    pub timeout_minutes: u32,
    pub timeout_after: u32,
    /// People with these roles skip automod.
    #[serde(with = "vec_as_string")]
    pub exempt_roles: Vec<i64>,
    /// People with Manage Messages skip automod.
    pub exempt_moderators: bool,
    /// Check bots' messages too.
    pub check_bots: bool,
    /// Post an alert here when something's blocked.
    #[serde(with = "option_as_string")]
    pub alert_channel_id: Option<i64>,
}

impl Default for ChannelAutomod {
    fn default() -> Self {
        Self {
            word_filter: false,
            words: Vec::new(),
            use_default_words: true,
            whole_word: true,
            leetspeak: false,
            word_action: WordAction::Block,
            spam_filter: false,
            max_messages: 5,
            per_seconds: 10,
            block_duplicates: true,
            max_mentions: 5,
            max_links: 0,
            max_attachments: 0,
            timeout_minutes: 0,
            timeout_after: 3,
            exempt_roles: Vec::new(),
            exempt_moderators: true,
            check_bots: false,
            alert_channel_id: None,
        }
    }
}

/// The outhouse-wide blocked words channels can use
/// (`GET/PUT /api/guilds/{id}/automod`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GuildAutomod {
    pub words: Vec<String>,
}

/// Blocked words: trimmed, lower case, no repeats, sensible sizes.
fn clean_words(raw: &[String]) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for w in raw {
        let w = w
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if w.is_empty() || w.chars().all(|c| c == '*') {
            continue;
        }
        if w.chars().count() > MAX_WORD_CHARS {
            return Err(AppError::BadRequest(format!(
                "Blocked words can be up to {MAX_WORD_CHARS} characters."
            )));
        }
        if !out.contains(&w) {
            out.push(w);
        }
    }
    if out.len() > MAX_WORDS {
        return Err(AppError::BadRequest(format!(
            "Up to {MAX_WORDS} blocked words."
        )));
    }
    Ok(out)
}

// ---- Matching ------------------------------------------------------------------

fn leet(c: char) -> char {
    match c {
        '0' => 'o',
        '1' => 'i',
        '3' => 'e',
        '4' | '@' => 'a',
        '5' | '$' => 's',
        '7' => 't',
        '8' => 'b',
        '9' => 'g',
        _ => c,
    }
}

/// Lower case, runs of whitespace as one space, optionally leetspeak folded.
fn normalise(text: &str, leetspeak: bool) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    let mut space = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_whitespace() {
            if !space && !out.is_empty() {
                out.push(' ');
            }
            space = true;
            continue;
        }
        space = false;
        out.push(if leetspeak { leet(c) } else { c });
    }
    if out.last() == Some(&' ') {
        out.pop();
    }
    out
}

/// Letters and numbers (after leetspeak folding, a `$` is a letter too).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every place the glob `pat` (with `*` = any run of word characters)
/// matching `text` from `start` could end.
fn glob_ends(pat: &[char], text: &[char], start: usize, out: &mut Vec<usize>) {
    // Iterative over (pattern index, text index) with a small stack; the
    // patterns and messages are short, and `*` never crosses a space.
    let mut stack = vec![(0usize, start)];
    let mut steps = 0usize;
    while let Some((pi, ti)) = stack.pop() {
        steps += 1;
        if steps > 20_000 {
            return;
        }
        if pi == pat.len() {
            if !out.contains(&ti) {
                out.push(ti);
            }
            continue;
        }
        if pat[pi] == '*' {
            // Zero characters, or one more word character.
            stack.push((pi + 1, ti));
            if ti < text.len() && is_word_char(text[ti]) {
                stack.push((pi, ti + 1));
            }
        } else if ti < text.len() && text[ti] == pat[pi] {
            stack.push((pi + 1, ti + 1));
        }
    }
}

/// The first blocked word in `text`, if any.
pub fn find_blocked<'a>(
    text: &str,
    words: &'a [String],
    whole_word: bool,
    leetspeak: bool,
) -> Option<&'a str> {
    let text = normalise(text, leetspeak);
    if text.is_empty() {
        return None;
    }
    let mut ends = Vec::new();
    for word in words {
        let pat = normalise(word, leetspeak);
        if pat.is_empty() {
            continue;
        }
        for start in 0..text.len() {
            if whole_word && start > 0 && is_word_char(text[start - 1]) {
                continue;
            }
            ends.clear();
            glob_ends(&pat, &text, start, &mut ends);
            let hit = ends.iter().any(|&end| {
                end > start && (!whole_word || end == text.len() || !is_word_char(text[end]))
            });
            if hit {
                return Some(word.as_str());
            }
        }
    }
    None
}

/// `<@id>`, `<@&id>`, @everyone and @here.
pub fn count_mentions(text: &str) -> usize {
    let tagged = text
        .match_indices("<@")
        .filter(|(i, _)| {
            let rest = &text[i + 2..];
            let rest = rest.strip_prefix('&').unwrap_or(rest);
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            digits > 0 && rest.as_bytes().get(digits) == Some(&b'>')
        })
        .count();
    tagged + text.matches("@everyone").count() + text.matches("@here").count()
}

pub fn count_links(text: &str) -> usize {
    let lower = text.to_ascii_lowercase();
    lower.matches("https://").count() + lower.matches("http://").count()
}

/// A short quote for the audit log and alerts (never the whole message).
fn snippet(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = one_line.chars().take(40).collect();
    if one_line.chars().count() > 40 {
        format!("“{short}…”")
    } else if short.is_empty() {
        "(no text)".into()
    } else {
        format!("“{short}”")
    }
}

// ---- Recent messages and strikes (in memory) -------------------------------

#[derive(Default)]
struct Recent {
    /// When each recent message was sent, with a hash of its text.
    sent: VecDeque<(Instant, u64)>,
}

fn recent() -> &'static Mutex<HashMap<(i64, i64), Recent>> {
    static MAP: OnceLock<Mutex<HashMap<(i64, i64), Recent>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn strikes() -> &'static Mutex<HashMap<(i64, i64), VecDeque<Instant>>> {
    static MAP: OnceLock<Mutex<HashMap<(i64, i64), VecDeque<Instant>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    normalise(text, false).hash(&mut h);
    h.finish()
}

/// (messages in the last `window`, same text in the last minute)
fn recent_counts(channel_id: i64, user_id: i64, window: Duration, hash: u64) -> (usize, usize) {
    let mut map = recent().lock().expect("automod lock");
    if map.len() > 20_000 {
        map.retain(|_, r| {
            r.sent
                .back()
                .is_some_and(|(t, _)| t.elapsed() < Duration::from_secs(600))
        });
    }
    let r = map.entry((channel_id, user_id)).or_default();
    let keep = window.max(DUPLICATE_WINDOW);
    while r.sent.front().is_some_and(|(t, _)| t.elapsed() > keep) {
        r.sent.pop_front();
    }
    let in_window = r.sent.iter().filter(|(t, _)| t.elapsed() <= window).count();
    let same = r
        .sent
        .iter()
        .filter(|(t, h)| *h == hash && t.elapsed() <= DUPLICATE_WINDOW)
        .count();
    (in_window, same)
}

fn remember_sent(channel_id: i64, user_id: i64, hash: u64) {
    let mut map = recent().lock().expect("automod lock");
    let r = map.entry((channel_id, user_id)).or_default();
    r.sent.push_back((Instant::now(), hash));
    while r.sent.len() > 100 {
        r.sent.pop_front();
    }
}

/// Counts a block; true when that makes `limit` within ten minutes (and
/// starts counting again).
fn add_strike(guild_id: i64, user_id: i64, limit: u32) -> bool {
    let mut map = strikes().lock().expect("automod lock");
    let list = map.entry((guild_id, user_id)).or_default();
    while list.front().is_some_and(|t| t.elapsed() > STRIKE_WINDOW) {
        list.pop_front();
    }
    list.push_back(Instant::now());
    if list.len() >= limit.max(1) as usize {
        list.clear();
        true
    } else {
        false
    }
}

// ---- Checking a message ------------------------------------------------------

/// A new message, or an edit (edits skip the rate and repeat limits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Send,
    Edit,
}

pub async fn channel_settings(st: &AppState, channel_id: i64) -> AppResult<Option<ChannelAutomod>> {
    let row: Option<SqlJson<ChannelAutomod>> =
        sqlx::query_scalar("SELECT settings FROM automod_channels WHERE channel_id = $1")
            .bind(channel_id)
            .fetch_optional(&st.db)
            .await?;
    Ok(row.map(|r| r.0))
}

async fn guild_words(st: &AppState, guild_id: i64) -> AppResult<Vec<String>> {
    let row: Option<SqlJson<GuildAutomod>> =
        sqlx::query_scalar("SELECT settings FROM automod_guilds WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_optional(&st.db)
            .await?;
    Ok(row.map(|r| r.0.words).unwrap_or_default())
}

/// Runs the channel's automod on a message from `author` (who has `access`
/// to the channel). `Ok(true)`: let it through but hide it behind a
/// "Filtered" reveal; `Ok(false)`: fine; `Err`: blocked (with the reason).
pub async fn check(
    st: &AppState,
    access: &ChannelAccess,
    author: &User,
    content: &str,
    attachments: usize,
    kind: Check,
) -> AppResult<bool> {
    let channel = &access.channel;
    if channel.guild_id == DM_GUILD {
        return Ok(false);
    }
    let Some(cfg) = channel_settings(st, channel.id).await? else {
        return Ok(false);
    };
    if !cfg.word_filter && !cfg.spam_filter {
        return Ok(false);
    }
    if author.bot && !cfg.check_bots {
        return Ok(false);
    }
    if !author.bot
        && ((cfg.exempt_moderators && access.perms & p::MANAGE_MESSAGES != 0)
            || access
                .ctx
                .role_ids
                .iter()
                .any(|r| cfg.exempt_roles.contains(r)))
    {
        return Ok(false);
    }

    let mut hide = false;
    if cfg.word_filter {
        let mut words = cfg.words.clone();
        if cfg.use_default_words {
            words.extend(guild_words(st, channel.guild_id).await?);
        }
        if let Some(word) = find_blocked(content, &words, cfg.whole_word, cfg.leetspeak) {
            match cfg.word_action {
                WordAction::Block => {
                    let rule = format!("Blocked word ({})", mask(word));
                    return Err(blocked(st, access, author, &cfg, content, &rule).await);
                }
                WordAction::Hide => hide = true,
            }
        }
    }

    if cfg.spam_filter {
        let mentions = count_mentions(content);
        if cfg.max_mentions > 0 && mentions > cfg.max_mentions as usize {
            let rule = format!(
                "Too many mentions ({mentions}, the limit is {})",
                cfg.max_mentions
            );
            return Err(blocked(st, access, author, &cfg, content, &rule).await);
        }
        let links = count_links(content);
        if cfg.max_links > 0 && links > cfg.max_links as usize {
            let rule = format!("Too many links ({links}, the limit is {})", cfg.max_links);
            return Err(blocked(st, access, author, &cfg, content, &rule).await);
        }
        if cfg.max_attachments > 0 && attachments > cfg.max_attachments as usize {
            let rule = format!(
                "Too many attachments ({attachments}, the limit is {})",
                cfg.max_attachments
            );
            return Err(blocked(st, access, author, &cfg, content, &rule).await);
        }
        if kind == Check::Send {
            let hash = text_hash(content);
            let window = Duration::from_secs(cfg.per_seconds.clamp(1, 300) as u64);
            let (in_window, same) = recent_counts(channel.id, author.id, window, hash);
            if cfg.max_messages > 0 && in_window >= cfg.max_messages as usize {
                let rule = format!(
                    "Sending too fast (over {} messages in {} seconds)",
                    cfg.max_messages, cfg.per_seconds
                );
                return Err(blocked(st, access, author, &cfg, content, &rule).await);
            }
            if cfg.block_duplicates && !content.trim().is_empty() && same + 1 >= DUPLICATE_LIMIT {
                let rule = "The same message again (3 times in a minute)".to_string();
                return Err(blocked(st, access, author, &cfg, content, &rule).await);
            }
            remember_sent(channel.id, author.id, hash);
        }
    }
    Ok(hide)
}

/// "f*ck" style: first and last letter of a blocked word, for the log.
fn mask(word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() <= 2 {
        return "*".repeat(chars.len());
    }
    let mut out = String::new();
    out.push(chars[0]);
    out.push_str(&"*".repeat(chars.len() - 2));
    out.push(chars[chars.len() - 1]);
    out
}

/// Logs a block, alerts the mod-log channel, maybe times them out, and
/// returns the error the sender sees.
async fn blocked(
    st: &AppState,
    access: &ChannelAccess,
    author: &User,
    cfg: &ChannelAutomod,
    content: &str,
    rule: &str,
) -> AppError {
    let channel = &access.channel;
    let guild_id = channel.guild_id;
    crate::audit::Entry::new(guild_id, author.id, "automod_block")
        .target("channel", channel.id, format!("#{}", channel.name))
        .details(format!("{rule} · {}", snippet(content)))
        .save(st)
        .await;
    let name = crate::audit::user_label(st, guild_id, author.id).await;
    let mut timed_out = None;
    if cfg.timeout_minutes > 0
        && !author.bot
        && access.ctx.guild_perms() & p::ADMINISTRATOR == 0
        && !access.ctx.is_owner()
        && add_strike(guild_id, author.id, cfg.timeout_after)
    {
        let minutes = cfg.timeout_minutes.min(28 * 24 * 60) as i32;
        let done = sqlx::query(
            "UPDATE guild_members SET timeout_until = now() + make_interval(mins => $3)
             WHERE guild_id = $1 AND user_id = $2",
        )
        .bind(guild_id)
        .bind(author.id)
        .bind(minutes)
        .execute(&st.db)
        .await;
        if done.is_ok() {
            st.dispatch(
                Audience::Guild(guild_id),
                ServerEvent::GuildRefresh { guild_id },
            );
            crate::audit::Entry::new(guild_id, author.id, "automod_timeout")
                .target("member", author.id, name.clone())
                .details(format!(
                    "For {minutes} minute{} after {} blocked messages",
                    if minutes == 1 { "" } else { "s" },
                    cfg.timeout_after.max(1)
                ))
                .save(st)
                .await;
            timed_out = Some(minutes);
        }
    }
    if let Some(alert) = cfg.alert_channel_id {
        let st = st.clone();
        let fields = vec![
            EmbedField {
                name: "Who".into(),
                value: name,
                inline: true,
            },
            EmbedField {
                name: "Where".into(),
                value: format!("#{}", channel.name),
                inline: true,
            },
            EmbedField {
                name: "Rule".into(),
                value: rule.to_string(),
                inline: false,
            },
            EmbedField {
                name: "Message".into(),
                value: snippet(content),
                inline: false,
            },
        ];
        let footer = timed_out.map(|m| format!("Timed out for {m} minutes after repeated blocks"));
        tokio::spawn(async move {
            if let Err(e) = post_alert(&st, guild_id, alert, fields, footer).await {
                tracing::warn!(error = %e, "couldn't post an automod alert");
            }
        });
    }
    let mut msg = format!("Your message was blocked by automod: {}.", rule);
    if let Some(m) = timed_out {
        msg.push_str(&format!(" You've been timed out for {m} minutes."));
    }
    AppError::Refused(msg)
}

/// The outhouse's "Automod" bot account (made the first time it's needed).
async fn automod_bot(st: &AppState, guild_id: i64) -> AppResult<i64> {
    let existing: Option<Option<i64>> =
        sqlx::query_scalar("SELECT bot_user_id FROM automod_guilds WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_optional(&st.db)
            .await?;
    if let Some(Some(id)) = existing {
        return Ok(id);
    }
    let id = st.ids.next_id();
    crate::auth::create_bot_user(&st.db, id, "Automod").await?;
    let stored: i64 = sqlx::query_scalar(
        "INSERT INTO automod_guilds (guild_id, settings, bot_user_id) VALUES ($1, '{}', $2)
         ON CONFLICT (guild_id) DO UPDATE
             SET bot_user_id = COALESCE(automod_guilds.bot_user_id, EXCLUDED.bot_user_id)
         RETURNING bot_user_id",
    )
    .bind(guild_id)
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    Ok(stored)
}

async fn post_alert(
    st: &AppState,
    guild_id: i64,
    channel_id: i64,
    fields: Vec<EmbedField>,
    footer: Option<String>,
) -> AppResult<()> {
    // Only to a text channel that's still in this outhouse.
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM channels WHERE id = $1 AND guild_id = $2 AND kind = $3)",
    )
    .bind(channel_id)
    .bind(guild_id)
    .bind(channel_kind::TEXT)
    .fetch_one(&st.db)
    .await?;
    if !ok {
        return Ok(());
    }
    let bot = automod_bot(st, guild_id).await?;
    let embed = Embed {
        kind: "rich".into(),
        title: Some("🚨 Automod blocked a message".into()),
        color: Some(0xE5484D),
        fields,
        footer,
        ..Embed::default()
    };
    crate::api::messages::post_bot_message(
        st,
        guild_id,
        channel_id,
        bot,
        None,
        None,
        String::new(),
        vec![embed],
    )
    .await?;
    Ok(())
}

// ---- Settings ------------------------------------------------------------------

/// `GET /api/channels/{id}/automod` (needs Manage Channels).
pub async fn get_channel(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<ChannelAutomod>> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    access.require(p::MANAGE_CHANNELS)?;
    if access.channel.guild_id == DM_GUILD {
        return Err(AppError::NotFound);
    }
    Ok(Json(
        channel_settings(&st, channel_id).await?.unwrap_or_default(),
    ))
}

/// `PUT /api/channels/{id}/automod` (needs Manage Channels).
pub async fn set_channel(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<ChannelAutomod>,
) -> AppResult<Json<ChannelAutomod>> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    access.require(p::MANAGE_CHANNELS)?;
    let channel = &access.channel;
    if channel.guild_id == DM_GUILD || channel.kind != channel_kind::TEXT {
        return Err(AppError::BadRequest(
            "Automod is for text channels in an outhouse.".into(),
        ));
    }
    let guild_id = channel.guild_id;
    let mut cfg = req;
    cfg.words = clean_words(&cfg.words)?;
    cfg.per_seconds = cfg.per_seconds.clamp(1, 300);
    cfg.max_messages = cfg.max_messages.min(50);
    cfg.max_mentions = cfg.max_mentions.min(50);
    cfg.max_links = cfg.max_links.min(50);
    cfg.max_attachments = cfg.max_attachments.min(10);
    cfg.timeout_minutes = cfg.timeout_minutes.min(7 * 24 * 60);
    cfg.timeout_after = cfg.timeout_after.clamp(1, 20);
    // Only roles and channels from this outhouse.
    let roles: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM roles WHERE guild_id = $1 AND id = ANY($2)")
            .bind(guild_id)
            .bind(&cfg.exempt_roles[..])
            .fetch_all(&st.db)
            .await?;
    cfg.exempt_roles.retain(|r| roles.contains(r));
    cfg.exempt_roles.dedup();
    if let Some(alert) = cfg.alert_channel_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM channels WHERE id = $1 AND guild_id = $2 AND kind = $3)",
        )
        .bind(alert)
        .bind(guild_id)
        .bind(channel_kind::TEXT)
        .fetch_one(&st.db)
        .await?;
        if !ok {
            return Err(AppError::BadRequest(
                "Pick a text channel in this outhouse for alerts.".into(),
            ));
        }
    }
    let before = channel_settings(&st, channel_id).await?.unwrap_or_default();
    sqlx::query(
        "INSERT INTO automod_channels (channel_id, settings) VALUES ($1, $2)
         ON CONFLICT (channel_id) DO UPDATE SET settings = EXCLUDED.settings, updated_at = now()",
    )
    .bind(channel_id)
    .bind(SqlJson(&cfg))
    .execute(&st.db)
    .await?;
    if before != cfg {
        let on_off = |b: bool| if b { "on" } else { "off" };
        crate::audit::Entry::new(guild_id, user.id, "automod_update")
            .target("channel", channel_id, format!("#{}", channel.name))
            .details(format!(
                "Word filter {} ({} words{}), spam limits {}",
                on_off(cfg.word_filter),
                cfg.words.len(),
                if cfg.use_default_words {
                    " + outhouse list"
                } else {
                    ""
                },
                on_off(cfg.spam_filter)
            ))
            .save(&st)
            .await;
    }
    Ok(Json(cfg))
}

/// `GET /api/guilds/{id}/automod`: the outhouse-wide word list (Manage
/// Channels or Manage Outhouse).
pub async fn get_guild(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<GuildAutomod>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    if ctx.guild_perms() & (p::MANAGE_CHANNELS | p::MANAGE_GUILD) == 0 {
        return Err(AppError::Forbidden);
    }
    let row: Option<SqlJson<GuildAutomod>> =
        sqlx::query_scalar("SELECT settings FROM automod_guilds WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_optional(&st.db)
            .await?;
    Ok(Json(row.map(|r| r.0).unwrap_or_default()))
}

/// `PUT /api/guilds/{id}/automod` `{"words": [...]}` (needs Manage Outhouse).
pub async fn set_guild(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<GuildAutomod>,
) -> AppResult<Json<GuildAutomod>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let cfg = GuildAutomod {
        words: clean_words(&req.words)?,
    };
    sqlx::query(
        "INSERT INTO automod_guilds (guild_id, settings) VALUES ($1, $2)
         ON CONFLICT (guild_id) DO UPDATE SET settings = EXCLUDED.settings, updated_at = now()",
    )
    .bind(guild_id)
    .bind(SqlJson(&cfg))
    .execute(&st.db)
    .await?;
    crate::audit::Entry::new(guild_id, user.id, "automod_update")
        .details(format!("Outhouse-wide blocked words: {}", cfg.words.len()))
        .save(&st)
        .await;
    Ok(Json(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn whole_words_and_wildcards() {
        let w = words(&["ass", "bad*", "dang it"]);
        assert_eq!(find_blocked("what a class act", &w, true, false), None);
        assert_eq!(
            find_blocked("what a class act", &w, false, false),
            Some("ass")
        );
        assert_eq!(find_blocked("You ASS!", &w, true, false), Some("ass"));
        assert_eq!(
            find_blocked("that's baddest", &w, true, false),
            Some("bad*")
        );
        assert_eq!(find_blocked("abad", &w, true, false), None);
        assert_eq!(
            find_blocked("oh DANG   it", &w, true, false),
            Some("dang it")
        );
        assert_eq!(find_blocked("dang itself", &w, true, false), None);
    }

    #[test]
    fn leetspeak_folding() {
        let w = words(&["noob"]);
        assert_eq!(find_blocked("you n00b", &w, true, false), None);
        assert_eq!(find_blocked("you n00b", &w, true, true), Some("noob"));
        let w = words(&["n00b"]);
        assert_eq!(find_blocked("you noob", &w, true, true), Some("n00b"));
    }

    #[test]
    fn counts_mentions_and_links() {
        assert_eq!(count_mentions("<@1> <@&2> @everyone @here <@x> <@3"), 4);
        assert_eq!(count_links("see https://a.b and HTTP://c.d"), 2);
        assert_eq!(snippet("short"), "“short”");
        assert_eq!(mask("bad"), "b*d");
    }

    #[test]
    fn settings_fill_in_defaults() {
        let cfg: ChannelAutomod = serde_json::from_str(r#"{"word_filter":true}"#).unwrap();
        assert!(cfg.word_filter && cfg.exempt_moderators && cfg.use_default_words);
        assert_eq!(cfg.max_messages, 5);
        let cfg: ChannelAutomod = serde_json::from_str(
            r#"{"word_action":"hide","exempt_roles":["5"],"alert_channel_id":null}"#,
        )
        .unwrap();
        assert_eq!(cfg.word_action, WordAction::Hide);
        assert_eq!(cfg.exempt_roles, vec![5]);
    }
}
