//! Message components: buttons and select menus on a bot's messages, in
//! Discord's JSON shape, and what happens when someone uses one.
//!
//! 1. A bot sends a message with `components` (action rows of buttons, or a
//!    string select menu): `POST /api/channels/{id}/messages`, an
//!    interaction reply, or an UPDATE_MESSAGE callback.
//! 2. Someone clicks a button: the client calls
//!    `POST /api/channels/{id}/messages/{message}/interactions`
//!    `{"custom_id", "values"}`.
//! 3. The bot gets `INTERACTION_CREATE` (`type` 3, with `message` and
//!    `data`) and answers with `POST /api/interactions/{id}/callback`:
//!    update the message, reply (optionally only to the clicker), or defer.
//!
//! Ephemeral messages (only one person sees them) are never saved: they're
//! sent to that person over the gateway and kept here in memory for 15
//! minutes so their buttons work and the bot can update them.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    button_style, component_type, Component, ComponentEmoji, ComponentInteractionData,
    ComponentInteractionRequest, Embed, Message, MessageInteraction, SelectOption, User,
};
use serde_json::{json, Value};

use super::members::fetch_member;
use super::messages::fetch_message;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::channel_access;
use crate::state::{AppState, Audience, PendingInteraction};

pub const MAX_ROWS: usize = 5;
pub const MAX_BUTTONS_PER_ROW: usize = 5;
pub const MAX_SELECT_OPTIONS: usize = 25;
pub const MAX_CUSTOM_ID: usize = 100;
const MAX_LABEL: usize = 80;
const MAX_OPTION_TEXT: usize = 100;
const MAX_PLACEHOLDER: usize = 150;

/// Button clicks and menu picks one person can make per window.
pub const INTERACTIONS_PER_WINDOW: u32 = 10;
pub const INTERACTION_WINDOW: Duration = Duration::from_secs(10);

const EPHEMERAL_TTL: Duration = Duration::from_secs(15 * 60);

fn bad(msg: impl Into<String>) -> AppError {
    AppError::BadRequest(msg.into())
}

fn clean_text(raw: &Option<String>, max: usize, what: &str) -> AppResult<Option<String>> {
    match raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) if s.chars().count() > max => {
            Err(bad(format!("{what} can be up to {max} characters.")))
        }
        Some(s) => Ok(Some(s.to_string())),
    }
}

fn clean_emoji(e: &Option<ComponentEmoji>) -> AppResult<Option<ComponentEmoji>> {
    let Some(e) = e else { return Ok(None) };
    let id = match e.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(id) if id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit()) => {
            Some(id.to_string())
        }
        Some(_) => return Err(bad("A component emoji's id must be an emoji ID.")),
    };
    let name = match e.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(n) if n.chars().count() <= 32 && !n.chars().any(char::is_control) => {
            Some(n.to_string())
        }
        Some(_) => return Err(bad("A component emoji's name is too long.")),
    };
    if id.is_none() && name.is_none() {
        return Ok(None);
    }
    Ok(Some(ComponentEmoji {
        id,
        name,
        animated: e.animated,
    }))
}

fn clean_custom_id(raw: &Option<String>, seen: &mut HashSet<String>) -> AppResult<String> {
    let id = raw.as_deref().unwrap_or("");
    if id.is_empty() || id.chars().count() > MAX_CUSTOM_ID {
        return Err(bad(format!(
            "Every button and select menu needs a custom_id of 1–{MAX_CUSTOM_ID} characters."
        )));
    }
    if id.chars().any(char::is_control) {
        return Err(bad("custom_id can't contain control characters."));
    }
    if !seen.insert(id.to_string()) {
        return Err(bad(format!(
            "custom_id \"{id}\" is used twice on this message."
        )));
    }
    Ok(id.to_string())
}

/// A link button's address: http or https only (never javascript: and
/// friends, and not a path on this site either).
fn clean_link(raw: &Option<String>) -> AppResult<String> {
    let url = raw.as_deref().map(str::trim).unwrap_or("");
    let lower = url.to_ascii_lowercase();
    let web = lower.starts_with("https://") || lower.starts_with("http://");
    match crate::security::safe_link(url) {
        Some(u) if web && u.len() <= 512 => Ok(u),
        _ => Err(bad(
            "Link buttons need a url starting with http:// or https://.",
        )),
    }
}

fn clean_button(b: &Component, seen: &mut HashSet<String>) -> AppResult<Component> {
    let style = b.style.unwrap_or(0);
    if !(button_style::PRIMARY..=button_style::LINK).contains(&style) {
        return Err(bad(
            "Button style must be 1 (primary), 2 (secondary), 3 (success), 4 (danger) or 5 (link).",
        ));
    }
    let label = clean_text(&b.label, MAX_LABEL, "Button labels")?;
    let emoji = clean_emoji(&b.emoji)?;
    if label.is_none() && emoji.is_none() {
        return Err(bad("Buttons need a label or an emoji."));
    }
    let (custom_id, url) = if style == button_style::LINK {
        if b.custom_id.is_some() {
            return Err(bad("Link buttons have a url, not a custom_id."));
        }
        (None, Some(clean_link(&b.url)?))
    } else {
        if b.url.is_some() {
            return Err(bad("Only link buttons (style 5) can have a url."));
        }
        (Some(clean_custom_id(&b.custom_id, seen)?), None)
    };
    Ok(Component {
        kind: component_type::BUTTON,
        style: Some(style),
        label,
        emoji,
        custom_id,
        url,
        disabled: b.disabled,
        ..Component::default()
    })
}

fn clean_select(m: &Component, seen: &mut HashSet<String>) -> AppResult<Component> {
    let custom_id = clean_custom_id(&m.custom_id, seen)?;
    if m.options.is_empty() || m.options.len() > MAX_SELECT_OPTIONS {
        return Err(bad(format!(
            "Select menus need 1–{MAX_SELECT_OPTIONS} options."
        )));
    }
    let mut values = HashSet::new();
    let mut options = Vec::with_capacity(m.options.len());
    for o in &m.options {
        let label = clean_text(&Some(o.label.clone()), MAX_OPTION_TEXT, "Option labels")?
            .ok_or_else(|| bad("Every option needs a label."))?;
        let value = o.value.clone();
        if value.is_empty() || value.chars().count() > MAX_OPTION_TEXT {
            return Err(bad(format!(
                "Option values must be 1–{MAX_OPTION_TEXT} characters."
            )));
        }
        if !values.insert(value.clone()) {
            return Err(bad(format!("Option value \"{value}\" is used twice.")));
        }
        options.push(SelectOption {
            label,
            value,
            description: clean_text(&o.description, MAX_OPTION_TEXT, "Option descriptions")?,
            emoji: clean_emoji(&o.emoji)?,
            default: o.default,
        });
    }
    let min = m.min_values.unwrap_or(1);
    let max = m.max_values.unwrap_or(1);
    if min as usize > MAX_SELECT_OPTIONS || max == 0 || max as usize > options.len() || min > max {
        return Err(bad(
            "min_values and max_values must fit the options (0 ≤ min ≤ max ≤ number of options).",
        ));
    }
    if options.iter().filter(|o| o.default).count() > max as usize {
        return Err(bad(
            "More options are picked by default than max_values allows.",
        ));
    }
    Ok(Component {
        kind: component_type::STRING_SELECT,
        custom_id: Some(custom_id),
        placeholder: clean_text(&m.placeholder, MAX_PLACEHOLDER, "Placeholders")?,
        min_values: Some(min),
        max_values: Some(max),
        options,
        disabled: m.disabled,
        ..Component::default()
    })
}

/// Checks a message's components (Discord's rules: up to five action rows,
/// each with up to five buttons or one select menu; unique custom_ids;
/// link buttons only to web addresses) and returns a tidy copy.
pub fn clean(rows: &[Component]) -> AppResult<Vec<Component>> {
    if rows.len() > MAX_ROWS {
        return Err(bad(format!(
            "A message can have up to {MAX_ROWS} rows of components."
        )));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.kind != component_type::ACTION_ROW {
            return Err(bad("Top-level components must be action rows (type 1)."));
        }
        if row.components.is_empty() {
            return Err(bad("Action rows can't be empty."));
        }
        let selects = row
            .components
            .iter()
            .filter(|c| c.kind == component_type::STRING_SELECT)
            .count();
        let children = if selects > 0 {
            if row.components.len() != 1 {
                return Err(bad(
                    "A select menu needs a row to itself (no buttons next to it).",
                ));
            }
            vec![clean_select(&row.components[0], &mut seen)?]
        } else {
            if row.components.len() > MAX_BUTTONS_PER_ROW {
                return Err(bad(format!(
                    "An action row can have up to {MAX_BUTTONS_PER_ROW} buttons."
                )));
            }
            row.components
                .iter()
                .map(|c| {
                    if c.kind == component_type::BUTTON {
                        clean_button(c, &mut seen)
                    } else {
                        Err(bad(
                            "Action rows hold buttons (type 2) or one string select (type 3).",
                        ))
                    }
                })
                .collect::<AppResult<Vec<_>>>()?
        };
        out.push(Component {
            kind: component_type::ACTION_ROW,
            components: children,
            ..Component::default()
        });
    }
    Ok(out)
}

/// The button or select menu with this custom_id (never a link button).
pub fn find<'a>(rows: &'a [Component], custom_id: &str) -> Option<&'a Component> {
    rows.iter()
        .flat_map(|r| r.components.iter())
        .find(|c| c.custom_id.as_deref() == Some(custom_id))
}

// ---- Ephemeral messages ------------------------------------------------------

struct Ephemeral {
    user_id: i64,
    message: Message,
    created: Instant,
}

fn ephemerals() -> &'static Mutex<HashMap<i64, Ephemeral>> {
    static STORE: OnceLock<Mutex<HashMap<i64, Ephemeral>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// An ephemeral message shown to `user_id` (if it's still remembered).
fn ephemeral_for(message_id: i64, user_id: i64) -> Option<Message> {
    let store = ephemerals().lock().expect("ephemeral lock");
    store
        .get(&message_id)
        .filter(|e| e.user_id == user_id && e.created.elapsed() < EPHEMERAL_TTL)
        .map(|e| e.message.clone())
}

/// Everything an ephemeral message holds.
pub struct EphemeralPost {
    pub guild_id: i64,
    pub channel_id: i64,
    pub thread_id: Option<i64>,
    pub bot_id: i64,
    pub user_id: i64,
    pub content: String,
    pub embeds: Vec<Embed>,
    /// Already checked with [`clean`].
    pub components: Vec<Component>,
    pub interaction: Option<MessageInteraction>,
}

/// Sends a message only `user_id` sees (over the gateway; never saved).
pub async fn send_ephemeral(st: &AppState, post: EphemeralPost) -> AppResult<Message> {
    let bot = fetch_member(&st.db, post.guild_id, post.bot_id).await?;
    let message = Message {
        id: st.ids.next_id(),
        channel_id: post.channel_id,
        author_id: post.bot_id,
        author_username: bot.username.clone(),
        author_display_name: bot.display_name.clone(),
        author_avatar_url: bot.avatar_url.clone(),
        author_bot: true,
        content: post.content,
        created_at: Utc::now(),
        edited_at: None,
        reply_to: None,
        attachments: Vec::new(),
        embeds: post
            .embeds
            .iter()
            .map(crate::security::clean_embed)
            .collect(),
        reactions: Vec::new(),
        interaction: post.interaction,
        event: None,
        pinned_at: None,
        thread_id: post.thread_id,
        thread: None,
        poll: None,
        lfg: None,
        components: post.components,
        ephemeral: true,
        filtered: false,
    };
    {
        let mut store = ephemerals().lock().expect("ephemeral lock");
        store.retain(|_, e| e.created.elapsed() < EPHEMERAL_TTL);
        if store.len() < 20_000 {
            store.insert(
                message.id,
                Ephemeral {
                    user_id: post.user_id,
                    message: message.clone(),
                    created: Instant::now(),
                },
            );
        }
    }
    st.dispatch(
        Audience::User(post.user_id),
        ServerEvent::MessageCreate(message.clone()),
    );
    Ok(message)
}

/// Changes a remembered ephemeral message (a bot's UPDATE_MESSAGE) and
/// sends it to its one viewer again. None if it's been forgotten.
pub fn update_ephemeral(
    st: &AppState,
    message_id: i64,
    content: Option<String>,
    embeds: Option<Vec<Embed>>,
    components: Option<Vec<Component>>,
) -> Option<Message> {
    let (user_id, message) = {
        let mut store = ephemerals().lock().expect("ephemeral lock");
        let e = store.get_mut(&message_id)?;
        if let Some(c) = content {
            e.message.content = c;
        }
        if let Some(list) = embeds {
            e.message.embeds = list.iter().map(crate::security::clean_embed).collect();
        }
        if let Some(c) = components {
            e.message.components = c;
        }
        e.message.edited_at = Some(Utc::now());
        (e.user_id, e.message.clone())
    };
    st.dispatch(
        Audience::User(user_id),
        ServerEvent::MessageUpdate(message.clone()),
    );
    Some(message)
}

/// Is this ephemeral message remembered?
pub fn is_ephemeral(message_id: i64) -> bool {
    ephemerals()
        .lock()
        .expect("ephemeral lock")
        .contains_key(&message_id)
}

// ---- Someone uses a button or select menu ------------------------------------

/// `POST /api/channels/{id}/messages/{message}/interactions`
/// `{"custom_id", "values": []}`: passes the click (or the picks) to the bot
/// that posted the message. Answers `{"id"}`; the bot's answer arrives over
/// the gateway (`INTERACTION_SUCCESS` with that id, plus the message update
/// or reply).
pub async fn use_component(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    Json(req): Json<ComponentInteractionRequest>,
) -> AppResult<Json<Value>> {
    if user.bot {
        return Err(bad("Bots can't press buttons."));
    }
    if crate::security::get().rate_limits
        && !crate::security::limiter().allow(
            &format!("component:{}", user.id),
            INTERACTIONS_PER_WINDOW,
            INTERACTION_WINDOW,
        )
    {
        return Err(AppError::TooManyRequests(
            "You're clicking too fast. Give it a few seconds.".into(),
        ));
    }
    let access = channel_access(&st.db, channel_id, user.id).await?;
    let guild_id = access.channel.guild_id;
    let message = match ephemeral_for(message_id, user.id) {
        Some(m) => m,
        None => {
            if is_ephemeral(message_id) {
                return Err(AppError::NotFound);
            }
            fetch_message(&st.db, message_id).await?
        }
    };
    if message.channel_id != channel_id {
        return Err(AppError::NotFound);
    }
    let is_bot_app: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM bots WHERE user_id = $1)")
            .bind(message.author_id)
            .fetch_one(&st.db)
            .await?;
    if !is_bot_app {
        return Err(bad("That message doesn't belong to a bot."));
    }
    let component = find(&message.components, &req.custom_id)
        .ok_or_else(|| bad("That button isn't on this message any more."))?;
    if component.disabled {
        return Err(bad("That one's switched off."));
    }
    let component_kind = component.kind;
    let values = if component_kind == component_type::STRING_SELECT {
        let mut seen = HashSet::new();
        for v in &req.values {
            if !component.options.iter().any(|o| &o.value == v) || !seen.insert(v) {
                return Err(bad("Pick from the menu's options."));
            }
        }
        let n = req.values.len();
        let min = component.min_values.unwrap_or(1) as usize;
        let max = component.max_values.unwrap_or(1) as usize;
        if n < min || n > max {
            return Err(bad(if min == max {
                format!("Pick {min}.")
            } else {
                format!("Pick {min} to {max}.")
            }));
        }
        req.values.clone()
    } else {
        Vec::new()
    };
    let bot_name = message.author_display_name.clone();
    if !st.is_online(message.author_id) {
        return Err(bad(format!("{bot_name} isn't running right now.")));
    }

    let me = fetch_member(&st.db, guild_id, user.id).await?;
    let id = st.ids.next_id();
    {
        let mut pending = st.interactions.lock().expect("interactions lock");
        pending.retain(|_, i| i.created.elapsed() < super::bots::INTERACTION_TTL);
        pending.insert(
            id,
            PendingInteraction {
                bot_id: message.author_id,
                guild_id,
                channel_id,
                user_id: user.id,
                display_name: me.display_name.clone(),
                command: req.custom_id.clone(),
                created: Instant::now(),
                message_id: Some(message.id),
                thread_id: message.thread_id,
            },
        );
    }
    let mut who: User = user;
    who.display_name = me.display_name;
    let bot_id = message.author_id;
    st.dispatch(
        Audience::User(bot_id),
        ServerEvent::InteractionCreate {
            id,
            kind: 3,
            guild_id,
            channel_id,
            user: who,
            command: String::new(),
            options: HashMap::new(),
            data: Some(ComponentInteractionData {
                custom_id: req.custom_id,
                component_type: component_kind,
                values,
            }),
            message: Some(Box::new(message)),
        },
    );
    Ok(Json(json!({ "id": id.to_string() })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<Component> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn accepts_discord_shaped_components() {
        let rows = parse(
            r#"[
              {"type":1,"components":[
                {"type":2,"style":1,"label":"Roll again","emoji":{"name":"🎲"},"custom_id":"again"},
                {"type":2,"style":5,"label":"Docs","url":"https://example.com/dice"}
              ]},
              {"type":1,"components":[
                {"type":3,"custom_id":"size","placeholder":"Dice size","options":[
                  {"label":"d6","value":"6"},{"label":"d20","value":"20","default":true}
                ]}
              ]}
            ]"#,
        );
        let clean = clean(&rows).unwrap();
        assert_eq!(clean.len(), 2);
        assert_eq!(clean[1].components[0].max_values, Some(1));
        assert!(find(&clean, "size").is_some());
        assert!(find(&clean, "again").is_some());
    }

    #[test]
    fn refuses_bad_components() {
        let bad_cases = [
            // javascript: link
            r#"[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":"javascript:alert(1)"}]}]"#,
            // a path on this site isn't a link button
            r#"[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":"/api/users/@me"}]}]"#,
            // duplicate custom_id
            r#"[{"type":1,"components":[{"type":2,"style":1,"label":"a","custom_id":"x"},{"type":2,"style":2,"label":"b","custom_id":"x"}]}]"#,
            // button without custom_id
            r#"[{"type":1,"components":[{"type":2,"style":1,"label":"a"}]}]"#,
            // select next to a button
            r#"[{"type":1,"components":[{"type":3,"custom_id":"s","options":[{"label":"a","value":"a"}]},{"type":2,"style":1,"label":"b","custom_id":"b"}]}]"#,
            // not an action row
            r#"[{"type":2,"style":1,"label":"a","custom_id":"a"}]"#,
            // max_values more than the options
            r#"[{"type":1,"components":[{"type":3,"custom_id":"s","max_values":3,"options":[{"label":"a","value":"a"}]}]}]"#,
        ];
        for case in bad_cases {
            assert!(clean(&parse(case)).is_err(), "should refuse {case}");
        }
        let long_id = format!(
            r#"[{{"type":1,"components":[{{"type":2,"style":1,"label":"a","custom_id":"{}"}}]}}]"#,
            "x".repeat(101)
        );
        assert!(clean(&parse(&long_id)).is_err());
        let six_rows = format!(
            "[{}]",
            (0..6)
                .map(|i| format!(
                    r#"{{"type":1,"components":[{{"type":2,"style":1,"label":"a","custom_id":"{i}"}}]}}"#
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(clean(&parse(&six_rows)).is_err());
    }
}
