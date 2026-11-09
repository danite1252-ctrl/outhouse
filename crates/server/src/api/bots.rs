//! Bots: accounts with a token instead of a password, which can use the
//! whole API and gateway, plus slash commands people run from the chat box.
//!
//! Flow for a slash command:
//! 1. The bot registers commands with `PUT /api/guilds/{id}/commands`.
//! 2. Someone types `/roll sides:20`; the client calls
//!    `POST /api/channels/{id}/interactions`.
//! 3. The bot gets an `INTERACTION_CREATE` gateway event and answers with
//!    `POST /api/interactions/{id}/reply` (shown as "Alice used /roll"), or
//!    Discord-style with `POST /api/interactions/{id}/callback`.
//!
//! Buttons and select menus on a bot's messages work the same way (see
//! `components.rs`); their answers can also update the message.

use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    interaction_response, BotInfo, BotTokenResponse, CommandOption, Component, CreateBotRequest,
    Embed, InteractionCallback, InteractionReply, InteractionRequest, Message, MessageInteraction,
    SlashCommand, User, FLAG_EPHEMERAL,
};
use outhouse_common::permissions as p;
use sqlx::types::Json as SqlJson;

use super::account::clean_name;
use super::components::{self, EphemeralPost};
use super::members::fetch_member;
use super::messages::{fetch_message, insert_embeds, post_bot_message_with, BotPost};
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, channel_audience, member_ctx};
use crate::state::{AppState, Audience, PendingInteraction};

pub(crate) const INTERACTION_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_COMMANDS: usize = 50;
const MAX_OPTIONS: usize = 10;

#[derive(sqlx::FromRow)]
struct BotRow {
    user_id: i64,
    owner_id: i64,
    guild_id: i64,
    created_at: DateTime<Utc>,
}

async fn bot_row(st: &AppState, bot_id: i64) -> AppResult<BotRow> {
    sqlx::query_as::<_, BotRow>(
        "SELECT user_id, owner_id, guild_id, created_at FROM bots WHERE user_id = $1",
    )
    .bind(bot_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)
}

/// Bots are managed by people with Manage Server in the outhouse they were made in.
async fn require_manager(st: &AppState, guild_id: i64, user_id: i64) -> AppResult<()> {
    member_ctx(&st.db, guild_id, user_id)
        .await?
        .require(p::MANAGE_GUILD)
}

async fn store_token(st: &AppState, bot_id: i64) -> AppResult<String> {
    let token = auth::new_token();
    sqlx::query("UPDATE bots SET token_hash = $2 WHERE user_id = $1")
        .bind(bot_id)
        .bind(auth::token_hash(&token))
        .execute(&st.db)
        .await?;
    Ok(token)
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<BotInfo>>> {
    require_manager(&st, guild_id, user.id).await?;
    let rows = sqlx::query_as::<_, BotRow>(
        "SELECT user_id, owner_id, guild_id, created_at FROM bots
         WHERE guild_id = $1 ORDER BY created_at",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let commands: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM commands WHERE guild_id = $1 AND bot_id = $2 ORDER BY name",
        )
        .bind(guild_id)
        .bind(r.user_id)
        .fetch_all(&st.db)
        .await?;
        out.push(BotInfo {
            user: auth::fetch_user(&st.db, r.user_id).await?,
            owner_id: r.owner_id,
            created_at: r.created_at,
            online: st.is_online(r.user_id),
            commands,
        });
    }
    Ok(Json(out))
}

/// Makes a bot, adds it to the outhouse and returns its token (shown once).
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<CreateBotRequest>,
) -> AppResult<Json<BotTokenResponse>> {
    require_manager(&st, guild_id, user.id).await?;
    let name = clean_name(&req.name);
    if name.is_empty() {
        return Err(AppError::BadRequest("Give the bot a name.".into()));
    }
    let bot_id = st.ids.next_id();
    let token = auth::new_token();
    let mut tx = st.db.begin().await?;
    auth::create_bot_user(&mut *tx, bot_id, &name).await?;
    sqlx::query(
        "INSERT INTO bots (user_id, token_hash, owner_id, guild_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(bot_id)
    .bind(auth::token_hash(&token))
    .bind(user.id)
    .bind(guild_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO guild_members (guild_id, user_id) VALUES ($1, $2)")
        .bind(guild_id)
        .bind(bot_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let member = fetch_member(&st.db, guild_id, bot_id).await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::MemberJoin { guild_id, member },
    );
    crate::audit::Entry::new(guild_id, user.id, "bot_create")
        .target("bot", bot_id, name.clone())
        .save(&st)
        .await;
    Ok(Json(BotTokenResponse {
        bot: auth::fetch_user(&st.db, bot_id).await?,
        token,
    }))
}

/// Makes a new token; the old one stops working straight away.
pub async fn reset_token(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<i64>,
) -> AppResult<Json<BotTokenResponse>> {
    let row = bot_row(&st, bot_id).await?;
    require_manager(&st, row.guild_id, user.id).await?;
    let token = store_token(&st, bot_id).await?;
    let bot = auth::fetch_user(&st.db, bot_id).await?;
    crate::audit::Entry::new(row.guild_id, user.id, "bot_token_reset")
        .target("bot", bot_id, bot.display_name.clone())
        .save(&st)
        .await;
    Ok(Json(BotTokenResponse { bot, token }))
}

/// Switches a bot off for good: its token stops working, its commands go,
/// and it leaves every outhouse. Its old messages stay.
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<i64>,
) -> AppResult<StatusCode> {
    let row = bot_row(&st, bot_id).await?;
    require_manager(&st, row.guild_id, user.id).await?;
    let bot_name = auth::fetch_user(&st.db, bot_id)
        .await
        .map(|u| u.display_name)
        .unwrap_or_default();
    crate::audit::Entry::new(row.guild_id, user.id, "bot_delete")
        .target("bot", bot_id, bot_name)
        .save(&st)
        .await;
    let guilds: Vec<i64> =
        sqlx::query_scalar("SELECT guild_id FROM guild_members WHERE user_id = $1")
            .bind(bot_id)
            .fetch_all(&st.db)
            .await?;
    sqlx::query("DELETE FROM commands WHERE bot_id = $1")
        .bind(bot_id)
        .execute(&st.db)
        .await?;
    sqlx::query("DELETE FROM bots WHERE user_id = $1")
        .bind(bot_id)
        .execute(&st.db)
        .await?;
    for guild_id in guilds {
        super::guilds::remove_member(&st, guild_id, bot_id).await?;
        st.dispatch(
            Audience::Guild(guild_id),
            ServerEvent::CommandsChanged { guild_id },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- Slash commands ----------------------------------------------------

fn valid_command_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

#[derive(sqlx::FromRow)]
struct CommandRow {
    id: i64,
    bot_id: i64,
    bot_name: String,
    name: String,
    description: String,
    options: SqlJson<Vec<CommandOption>>,
}

impl From<CommandRow> for SlashCommand {
    fn from(r: CommandRow) -> Self {
        SlashCommand {
            id: r.id,
            bot_id: r.bot_id,
            bot_name: r.bot_name,
            name: r.name,
            description: r.description,
            options: r.options.0,
        }
    }
}

const COMMAND_SELECT: &str = "
    SELECT c.id, c.bot_id, COALESCE(gm.nickname, u.display_name) AS bot_name,
           c.name, c.description, c.options
    FROM commands c
    JOIN users u ON u.id = c.bot_id
    LEFT JOIN guild_members gm ON gm.guild_id = c.guild_id AND gm.user_id = c.bot_id
    WHERE c.guild_id = $1";

/// Every slash command in an outhouse (for the chat box's / menu).
pub async fn list_commands(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<SlashCommand>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let sql = format!("{COMMAND_SELECT} ORDER BY c.name");
    let rows = sqlx::query_as::<_, CommandRow>(&sql)
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.into_iter().map(SlashCommand::from).collect()))
}

/// A bot replaces its slash commands in an outhouse.
pub async fn set_commands(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(commands): Json<Vec<SlashCommand>>,
) -> AppResult<Json<Vec<SlashCommand>>> {
    if !user.bot {
        return Err(AppError::BadRequest(
            "Only bots can register commands.".into(),
        ));
    }
    member_ctx(&st.db, guild_id, user.id).await?;
    if commands.len() > MAX_COMMANDS {
        return Err(AppError::BadRequest(format!(
            "A bot can have up to {MAX_COMMANDS} commands."
        )));
    }
    for c in &commands {
        if !valid_command_name(&c.name) {
            return Err(AppError::BadRequest(format!(
                "\"{}\" isn't a valid command name: use 1–32 lower-case letters, numbers, - or _.",
                c.name
            )));
        }
        if c.options.len() > MAX_OPTIONS || c.options.iter().any(|o| !valid_command_name(&o.name)) {
            return Err(AppError::BadRequest(format!(
                "/{} has invalid options (up to {MAX_OPTIONS}, named like commands).",
                c.name
            )));
        }
    }
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM commands WHERE guild_id = $1 AND bot_id = $2")
        .bind(guild_id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    for c in &commands {
        let result = sqlx::query(
            "INSERT INTO commands (id, guild_id, bot_id, name, description, options)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(st.ids.next_id())
        .bind(guild_id)
        .bind(user.id)
        .bind(&c.name)
        .bind(c.description.chars().take(100).collect::<String>())
        .bind(SqlJson(&c.options))
        .execute(&mut *tx)
        .await;
        match result {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(AppError::Conflict(format!(
                    "Another bot in this outhouse already has /{}.",
                    c.name
                )))
            }
            Err(e) => return Err(e.into()),
        }
    }
    tx.commit().await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::CommandsChanged { guild_id },
    );

    let sql = format!("{COMMAND_SELECT} AND c.bot_id = $2 ORDER BY c.name");
    let rows = sqlx::query_as::<_, CommandRow>(&sql)
        .bind(guild_id)
        .bind(user.id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.into_iter().map(SlashCommand::from).collect()))
}

/// Someone runs a slash command: pass it to the bot.
pub async fn interact(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<InteractionRequest>,
) -> AppResult<StatusCode> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    let guild_id = access.channel.guild_id;
    let sql = format!("{COMMAND_SELECT} AND c.id = $2");
    let command: SlashCommand = sqlx::query_as::<_, CommandRow>(&sql)
        .bind(guild_id)
        .bind(req.command_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?
        .into();
    for o in &command.options {
        if o.required
            && req
                .options
                .get(&o.name)
                .map_or(true, |v| v.trim().is_empty())
        {
            return Err(AppError::BadRequest(format!(
                "/{} needs a value for {}.",
                command.name, o.name
            )));
        }
    }
    let bot_can_post = channel_access(&st.db, channel_id, command.bot_id)
        .await
        .map(|a| a.perms & p::SEND_MESSAGES != 0)
        .unwrap_or(false);
    if !bot_can_post {
        return Err(AppError::BadRequest(format!(
            "{} can't post in this channel.",
            command.bot_name
        )));
    }
    if !st.is_online(command.bot_id) {
        return Err(AppError::BadRequest(format!(
            "{} isn't running right now.",
            command.bot_name
        )));
    }

    let me = fetch_member(&st.db, guild_id, user.id).await?;
    let id = st.ids.next_id();
    {
        let mut pending = st.interactions.lock().expect("interactions lock");
        pending.retain(|_, i| i.created.elapsed() < INTERACTION_TTL);
        pending.insert(
            id,
            PendingInteraction {
                bot_id: command.bot_id,
                guild_id,
                channel_id,
                user_id: user.id,
                display_name: me.display_name.clone(),
                command: command.name.clone(),
                created: Instant::now(),
                message_id: None,
                thread_id: None,
            },
        );
    }
    let mut who: User = user;
    who.display_name = me.display_name;
    st.dispatch(
        Audience::User(command.bot_id),
        ServerEvent::InteractionCreate {
            id,
            kind: 2,
            guild_id,
            channel_id,
            user: who,
            command: command.name,
            options: req.options,
            message: None,
            data: None,
        },
    );
    Ok(StatusCode::ACCEPTED)
}

/// The interaction a bot is answering (it must be the bot's own).
fn pending_for(st: &AppState, interaction_id: i64, bot_id: i64) -> AppResult<PendingInteraction> {
    let pending = st
        .interactions
        .lock()
        .expect("interactions lock")
        .get(&interaction_id)
        .cloned()
        .filter(|i| i.created.elapsed() < INTERACTION_TTL)
        .ok_or(AppError::NotFound)?;
    if pending.bot_id != bot_id {
        return Err(AppError::Forbidden);
    }
    Ok(pending)
}

/// Tells the person who used the command or button that the bot answered.
fn answered(st: &AppState, interaction_id: i64, pending: &PendingInteraction) {
    st.dispatch(
        Audience::User(pending.user_id),
        ServerEvent::InteractionSuccess { id: interaction_id },
    );
}

fn tidy_embeds(embeds: Vec<Embed>) -> AppResult<Vec<Embed>> {
    if embeds.len() > 10 {
        return Err(AppError::BadRequest("Up to 10 embeds per message.".into()));
    }
    Ok(embeds
        .into_iter()
        .map(|mut e| {
            if e.kind.is_empty() {
                e.kind = "rich".into();
            }
            e
        })
        .collect())
}

/// A bot's new message in answer to an interaction (CHANNEL_MESSAGE).
async fn respond_with_message(
    st: &AppState,
    bot: &User,
    pending: PendingInteraction,
    content: String,
    embeds: Vec<Embed>,
    components: Vec<Component>,
    ephemeral: bool,
) -> AppResult<Message> {
    let content = content.trim().to_string();
    if content.is_empty() && embeds.is_empty() && components.is_empty() {
        return Err(AppError::BadRequest("Replies can't be empty.".into()));
    }
    if content.chars().count() > 4000 {
        return Err(AppError::BadRequest("That reply is too big.".into()));
    }
    let embeds = tidy_embeds(embeds)?;
    let components = components::clean(&components)?;
    // Slash commands show "Alice used /roll"; button answers reply to the
    // message the button is on.
    let interaction = pending.message_id.is_none().then(|| MessageInteraction {
        user_id: pending.user_id,
        display_name: pending.display_name.clone(),
        command: pending.command.clone(),
    });
    if ephemeral {
        return components::send_ephemeral(
            st,
            EphemeralPost {
                guild_id: pending.guild_id,
                channel_id: pending.channel_id,
                thread_id: pending.thread_id,
                bot_id: bot.id,
                user_id: pending.user_id,
                content,
                embeds,
                components,
                interaction,
            },
        )
        .await;
    }
    let access = channel_access(&st.db, pending.channel_id, bot.id).await?;
    access.require(p::SEND_MESSAGES)?;
    let filtered =
        crate::automod::check(st, &access, bot, &content, 0, crate::automod::Check::Send).await?;
    let reply_to_id = pending
        .message_id
        .filter(|id| !components::is_ephemeral(*id));
    post_bot_message_with(
        st,
        BotPost {
            guild_id: pending.guild_id,
            channel_id: pending.channel_id,
            author_id: bot.id,
            content,
            embeds,
            interaction,
            components,
            thread_id: pending.thread_id,
            reply_to_id,
            filtered,
            ..BotPost::default()
        },
    )
    .await
}

/// A bot changes the message its button or select menu is on (UPDATE_MESSAGE).
async fn update_component_message(
    st: &AppState,
    bot: &User,
    pending: &PendingInteraction,
    content: Option<String>,
    embeds: Option<Vec<Embed>>,
    components: Option<Vec<Component>>,
) -> AppResult<Message> {
    let message_id = pending.message_id.ok_or_else(|| {
        AppError::BadRequest(
            "Only button and select menu interactions can update a message.".into(),
        )
    })?;
    let content = content.map(|c| c.trim().to_string());
    if content.as_ref().is_some_and(|c| c.chars().count() > 4000) {
        return Err(AppError::BadRequest("That message is too big.".into()));
    }
    let embeds = embeds.map(tidy_embeds).transpose()?;
    let components = components.map(|c| components::clean(&c)).transpose()?;
    if components::is_ephemeral(message_id) {
        return components::update_ephemeral(st, message_id, content, embeds, components)
            .ok_or(AppError::NotFound);
    }
    let original = fetch_message(&st.db, message_id).await?;
    if original.author_id != bot.id {
        return Err(AppError::Forbidden);
    }
    let filtered = match &content {
        Some(text) => {
            let access = channel_access(&st.db, original.channel_id, bot.id).await?;
            crate::automod::check(st, &access, bot, text, 0, crate::automod::Check::Edit).await?
        }
        None => original.filtered,
    };
    sqlx::query(
        "UPDATE messages SET content = COALESCE($2, content), filtered = $3,
                components = CASE WHEN $4 THEN $5 ELSE components END
         WHERE id = $1",
    )
    .bind(message_id)
    .bind(&content)
    .bind(filtered)
    .bind(components.is_some())
    .bind(components.filter(|c| !c.is_empty()).map(SqlJson))
    .execute(&st.db)
    .await?;
    if let Some(embeds) = embeds {
        sqlx::query("DELETE FROM message_embeds WHERE message_id = $1")
            .bind(message_id)
            .execute(&st.db)
            .await?;
        insert_embeds(&st.db, message_id, &embeds).await?;
    }
    let message = fetch_message(&st.db, message_id).await?;
    let channel = crate::perms::fetch_channel(&st.db, message.channel_id).await?;
    st.dispatch(
        channel_audience(&channel),
        ServerEvent::MessageUpdate(message.clone()),
    );
    Ok(message)
}

/// A bot answers a slash command, button or select menu with a message:
/// `{"content", "embeds", "components", "flags": 64 | "ephemeral": true}`.
pub async fn reply(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(interaction_id): Path<i64>,
    Json(req): Json<InteractionReply>,
) -> AppResult<Json<Message>> {
    let pending = pending_for(&st, interaction_id, user.id)?;
    let ephemeral = req.ephemeral || req.flags & FLAG_EPHEMERAL != 0;
    let message = respond_with_message(
        &st,
        &user,
        pending.clone(),
        req.content,
        req.embeds,
        req.components,
        ephemeral,
    )
    .await?;
    answered(&st, interaction_id, &pending);
    Ok(Json(message))
}

/// `POST /api/interactions/{id}/callback`, Discord's interaction response:
/// `{"type": 4, "data": {...}}` reply (`flags: 64` = only the clicker sees
/// it), `5` "thinking" (reply later), `6` got it (buttons; change the
/// message later), `7` update the message the button is on. Answers with
/// the new or changed message (204 for 5 and 6). Can be called again
/// within 15 minutes (e.g. a reply after a 5).
pub async fn callback(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(interaction_id): Path<i64>,
    Json(req): Json<InteractionCallback>,
) -> AppResult<Response> {
    let pending = pending_for(&st, interaction_id, user.id)?;
    let data = req.data.unwrap_or_default();
    let out = match req.kind {
        interaction_response::CHANNEL_MESSAGE => {
            let message = respond_with_message(
                &st,
                &user,
                pending.clone(),
                data.content.unwrap_or_default(),
                data.embeds.unwrap_or_default(),
                data.components.unwrap_or_default(),
                data.flags & FLAG_EPHEMERAL != 0,
            )
            .await?;
            Json(message).into_response()
        }
        interaction_response::DEFERRED_CHANNEL_MESSAGE => StatusCode::NO_CONTENT.into_response(),
        interaction_response::DEFERRED_UPDATE_MESSAGE => {
            if pending.message_id.is_none() {
                return Err(AppError::BadRequest(
                    "Type 6 is for buttons and select menus; use 5 for slash commands.".into(),
                ));
            }
            StatusCode::NO_CONTENT.into_response()
        }
        interaction_response::UPDATE_MESSAGE => {
            let message = update_component_message(
                &st,
                &user,
                &pending,
                data.content,
                data.embeds,
                data.components,
            )
            .await?;
            Json(message).into_response()
        }
        _ => {
            return Err(AppError::BadRequest(
                "Interaction response type must be 4, 5, 6 or 7.".into(),
            ))
        }
    };
    answered(&st, interaction_id, &pending);
    Ok(out)
}
