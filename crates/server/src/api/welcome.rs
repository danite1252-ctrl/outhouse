//! Join and leave messages: "Knock knock — {name} just took the last free
//! stall." posted in the outhouse's welcome channel when someone joins, and
//! (if switched on) a line when someone leaves.
//!
//! - A template is picked at random when the line is posted and its number is
//!   stored on the message (`messages.system`), so it reads the same on reload.
//! - The message's author is the person who joined or left; the client draws
//!   it as a one-liner with their name as a clickable mention. Nobody is
//!   pinged (no `<@id>` in the text, no push, no mention badge).
//! - Not posted: within 5 minutes of the last one for the same person (leave
//!   and rejoin spam), in a shut-down outhouse, or when the channel is gone or
//!   @everyone can't see it.
//! - Bots get their own line ("🤖 {name} was installed in the outhouse.").
//! - Leave lines only for people who leave themselves (not kicks or bans).
//!
//! Settings (Outhouse settings → Overview → Welcome): `GET`/`PUT
//! /api/guilds/{id}/welcome`, Manage Outhouse.

use axum::extract::{Path, State};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::SystemMessage;
use outhouse_common::permissions as p;
use serde::{Deserialize, Serialize};
use sqlx::types::Json as SqlJson;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{member_ctx, role_set_ctx, visible_text_channels};
use crate::state::{AppState, Audience};

/// `{name}` is the member (drawn as a mention by the client).
pub const JOIN_TEMPLATES: [&str; 12] = [
    "{name} has joined the outhouse. Mind the gap.",
    "Knock knock — {name} just took the last free stall.",
    "{name} has entered the long drop. Welcome aboard!",
    "Everyone hold your breath: {name} just walked in.",
    "{name} found the outhouse. Fresh roll of toilet paper on the house.",
    "Occupied! {name} has joined.",
    "{name} squeezed in just in time.",
    "A wild {name} appeared behind the outhouse door.",
    "{name} has flushed into the outhouse. Welcome!",
    "Please wash your hands and welcome {name}.",
    "{name} brought their own TP. A true professional.",
    "The seat's still warm — say hi to {name}!",
];

pub const LEAVE_TEMPLATES: [&str; 4] = [
    "{name} has left the outhouse. Don't forget to flush.",
    "{name} took the express pipe out.",
    "{name} has left the building. The seat is up.",
    "{name} wandered off into the bushes.",
];

pub const BOT_TEMPLATES: [&str; 1] = ["🤖 {name} was installed in the outhouse."];

/// No second line for the same person within this long.
const REPEAT_GAP_MINUTES: i32 = 5;

/// What's stored in `messages.system`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSystem {
    pub kind: String,
    pub template: i16,
    #[serde(default)]
    pub wave: bool,
}

fn templates(kind: &str) -> &'static [&'static str] {
    match kind {
        "member_leave" => &LEAVE_TEMPLATES,
        "bot_join" => &BOT_TEMPLATES,
        _ => &JOIN_TEMPLATES,
    }
}

impl StoredSystem {
    pub fn to_message(&self) -> SystemMessage {
        let list = templates(&self.kind);
        let text = usize::try_from(self.template)
            .ok()
            .and_then(|i| list.get(i))
            .unwrap_or(&list[0]);
        SystemMessage {
            kind: self.kind.clone(),
            template: self.template,
            text: (*text).to_string(),
            wave: self.wave && self.kind == "member_join",
        }
    }
}

fn random_index(len: usize) -> usize {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut b = [0u8; 4];
    if SystemRandom::new().fill(&mut b).is_err() {
        return 0;
    }
    (u32::from_le_bytes(b) as usize) % len.max(1)
}

#[derive(sqlx::FromRow)]
struct GuildWelcome {
    shut_down: bool,
    join_messages: bool,
    join_channel_id: Option<i64>,
    leave_messages: bool,
    join_wave: bool,
}

async fn load(st: &AppState, guild_id: i64) -> AppResult<Option<GuildWelcome>> {
    Ok(sqlx::query_as::<_, GuildWelcome>(
        "SELECT shut_down_at IS NOT NULL AS shut_down, join_messages, join_channel_id,
                leave_messages, join_wave
         FROM guilds WHERE id = $1",
    )
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await?)
}

/// Text channels @everyone can see, in channel-list order.
async fn everyone_channels(st: &AppState, guild_id: i64) -> AppResult<Vec<i64>> {
    let ctx = role_set_ctx(&st.db, guild_id, &[]).await?;
    visible_text_channels(&st.db, &ctx, 0).await
}

/// Where lines go: the chosen channel if @everyone can still see it (None if
/// it's gone or hidden: nothing is posted), else the first channel they can.
async fn target_channel(
    st: &AppState,
    guild_id: i64,
    chosen: Option<i64>,
) -> AppResult<Option<i64>> {
    let visible = everyone_channels(st, guild_id).await?;
    Ok(match chosen {
        Some(id) => visible.contains(&id).then_some(id),
        None => default_channel(st, &visible).await?,
    })
}

/// The first channel in `visible` that isn't age-restricted.
async fn default_channel(st: &AppState, visible: &[i64]) -> AppResult<Option<i64>> {
    if visible.is_empty() {
        return Ok(None);
    }
    let safe: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM channels WHERE id = ANY($1) AND content_mode <> 'nsfw'")
            .bind(visible)
            .fetch_all(&st.db)
            .await?;
    Ok(visible.iter().copied().find(|id| safe.contains(id)))
}

/// Someone joined (`bot`: a bot was added). Never fails the join: problems
/// are logged.
pub async fn member_joined(st: &AppState, guild_id: i64, user_id: i64, bot: bool) {
    let kind = if bot { "bot_join" } else { "member_join" };
    if let Err(e) = post(st, guild_id, user_id, kind).await {
        tracing::warn!("join message in {guild_id} for {user_id}: {e:?}");
    }
}

/// Someone left by themselves (only posted when leave messages are on).
pub async fn member_left(st: &AppState, guild_id: i64, user_id: i64) {
    if let Err(e) = post(st, guild_id, user_id, "member_leave").await {
        tracing::warn!("leave message in {guild_id} for {user_id}: {e:?}");
    }
}

async fn post(st: &AppState, guild_id: i64, user_id: i64, kind: &str) -> AppResult<()> {
    let Some(g) = load(st, guild_id).await? else {
        return Ok(());
    };
    let leave = kind == "member_leave";
    if g.shut_down || (leave && !g.leave_messages) || (!leave && !g.join_messages) {
        return Ok(());
    }
    let Some(channel_id) = target_channel(st, guild_id, g.join_channel_id).await? else {
        return Ok(());
    };
    // One line per person per 5 minutes (claimed atomically, so two joins at
    // once can't both post).
    let claimed: Option<i32> = sqlx::query_scalar(
        "INSERT INTO welcome_posts (guild_id, user_id, kind) VALUES ($1, $2, $3)
         ON CONFLICT (guild_id, user_id, kind) DO UPDATE SET posted_at = now()
             WHERE welcome_posts.posted_at < now() - make_interval(mins => $4)
         RETURNING 1",
    )
    .bind(guild_id)
    .bind(user_id)
    .bind(if leave { "leave" } else { "join" })
    .bind(REPEAT_GAP_MINUTES)
    .fetch_optional(&st.db)
    .await?;
    if claimed.is_none() {
        return Ok(());
    }
    let list = templates(kind);
    let stored = StoredSystem {
        kind: kind.to_string(),
        template: random_index(list.len()) as i16,
        wave: g.join_wave && kind == "member_join",
    };
    // The text kept in `content` (search, reply previews, forwards): the
    // display name only, never a mention, so nobody is pinged.
    let name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    let content = list[stored.template as usize].replace("{name}", &name);
    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO messages (id, channel_id, author_id, content, system)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(channel_id)
    .bind(user_id)
    .bind(&content)
    .bind(SqlJson(&stored))
    .execute(&st.db)
    .await?;
    sqlx::query("UPDATE channels SET last_message_id = $2 WHERE id = $1")
        .bind(channel_id)
        .bind(id)
        .execute(&st.db)
        .await?;
    let message = super::messages::fetch_message(&st.db, id).await?;
    // Just the message: no push notification and no mention record.
    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id,
        },
        ServerEvent::MessageCreate(message),
    );
    Ok(())
}

// ---- Settings ------------------------------------------------------------

#[derive(Serialize)]
pub struct WelcomeSettings {
    pub join_messages: bool,
    pub leave_messages: bool,
    pub wave: bool,
    /// The chosen channel (null: the default).
    #[serde(with = "outhouse_common::id::option_as_string")]
    pub channel_id: Option<i64>,
    /// Where lines go right now (null: nowhere, e.g. the chosen channel was
    /// deleted or @everyone can't see it).
    #[serde(with = "outhouse_common::id::option_as_string")]
    pub effective_channel_id: Option<i64>,
    /// Text channels @everyone can see (the picker's choices).
    #[serde(with = "outhouse_common::id::vec_as_string")]
    pub channel_choices: Vec<i64>,
}

async fn require_manager(st: &AppState, guild_id: i64, user_id: i64) -> AppResult<()> {
    member_ctx(&st.db, guild_id, user_id)
        .await?
        .require(p::MANAGE_GUILD)
}

async fn settings(st: &AppState, guild_id: i64) -> AppResult<WelcomeSettings> {
    let g = load(st, guild_id).await?.ok_or(AppError::NotFound)?;
    let visible = everyone_channels(st, guild_id).await?;
    let effective = match g.join_channel_id {
        Some(id) => visible.contains(&id).then_some(id),
        None => default_channel(st, &visible).await?,
    };
    Ok(WelcomeSettings {
        join_messages: g.join_messages,
        leave_messages: g.leave_messages,
        wave: g.join_wave,
        channel_id: g.join_channel_id,
        effective_channel_id: effective,
        channel_choices: visible,
    })
}

/// `GET /api/guilds/{id}/welcome`
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<WelcomeSettings>> {
    require_manager(&st, guild_id, user.id).await?;
    Ok(Json(settings(&st, guild_id).await?))
}

/// Missing fields stay as they are.
#[derive(Deserialize)]
pub struct WelcomeUpdate {
    pub join_messages: Option<bool>,
    pub leave_messages: Option<bool>,
    pub wave: Option<bool>,
    /// A channel ID, or "" / null for the default.
    #[serde(default, deserialize_with = "channel_choice")]
    pub channel_id: Option<Option<i64>>,
}

/// Absent → None (unchanged); null or "" → Some(None) (default); "123" → Some(Some(123)).
fn channel_choice<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<i64>>, D::Error> {
    let v: Option<String> = Option::deserialize(d)?;
    match v.as_deref().map(str::trim) {
        None | Some("") => Ok(Some(None)),
        Some(s) => s
            .parse::<i64>()
            .map(|id| Some(Some(id)))
            .map_err(|_| serde::de::Error::custom("channel_id must be a channel ID")),
    }
}

/// `PUT /api/guilds/{id}/welcome`
pub async fn set(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<WelcomeUpdate>,
) -> AppResult<Json<WelcomeSettings>> {
    require_manager(&st, guild_id, user.id).await?;
    if let Some(Some(id)) = req.channel_id {
        if !everyone_channels(&st, guild_id).await?.contains(&id) {
            return Err(AppError::BadRequest(
                "Pick a text channel in this outhouse that everyone can see.".into(),
            ));
        }
    }
    let before = settings(&st, guild_id).await?;
    sqlx::query(
        "UPDATE guilds SET
             join_messages = COALESCE($2, join_messages),
             leave_messages = COALESCE($3, leave_messages),
             join_wave = COALESCE($4, join_wave),
             join_channel_id = CASE WHEN $5 THEN $6 ELSE join_channel_id END
         WHERE id = $1",
    )
    .bind(guild_id)
    .bind(req.join_messages)
    .bind(req.leave_messages)
    .bind(req.wave)
    .bind(req.channel_id.is_some())
    .bind(req.channel_id.flatten())
    .execute(&st.db)
    .await?;
    let after = settings(&st, guild_id).await?;
    let mut changes = Vec::new();
    let onoff = |b: bool| if b { "on" } else { "off" };
    if before.join_messages != after.join_messages {
        changes.push(format!("Join messages {}", onoff(after.join_messages)));
    }
    if before.leave_messages != after.leave_messages {
        changes.push(format!("Leave messages {}", onoff(after.leave_messages)));
    }
    if before.wave != after.wave {
        changes.push(format!("Wave button {}", onoff(after.wave)));
    }
    if before.channel_id != after.channel_id {
        let name: Option<String> = match after.channel_id {
            Some(id) => {
                sqlx::query_scalar("SELECT name FROM channels WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&st.db)
                    .await?
            }
            None => None,
        };
        changes.push(match name {
            Some(n) => format!("Posted in #{n}"),
            None => "Posted in the first channel everyone can see".into(),
        });
    }
    if !changes.is_empty() {
        crate::audit::Entry::new(guild_id, user.id, "welcome_update")
            .details(changes.join(", "))
            .save(&st)
            .await;
    }
    Ok(Json(after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_have_one_name_each() {
        for t in JOIN_TEMPLATES
            .iter()
            .chain(LEAVE_TEMPLATES.iter())
            .chain(BOT_TEMPLATES.iter())
        {
            assert_eq!(t.matches("{name}").count(), 1, "{t}");
        }
    }

    #[test]
    fn stored_template_reads_back() {
        let s = StoredSystem {
            kind: "member_join".into(),
            template: 5,
            wave: true,
        };
        let m = s.to_message();
        assert_eq!(m.text, "Occupied! {name} has joined.");
        assert!(m.wave);
        // Out of range falls back to the first; leave lines never wave.
        let s = StoredSystem {
            kind: "member_leave".into(),
            template: 99,
            wave: true,
        };
        let m = s.to_message();
        assert_eq!(m.text, LEAVE_TEMPLATES[0]);
        assert!(!m.wave);
    }

    #[test]
    fn picks_stay_in_range() {
        for _ in 0..200 {
            assert!(random_index(12) < 12);
        }
    }
}
