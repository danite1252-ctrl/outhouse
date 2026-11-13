//! Messages sent over the real-time WebSocket gateway (`/api/gateway`).
//!
//! Client → server: `{"op": "IDENTIFY", "d": {"token": "..."}}`, then
//! `{"op": "HEARTBEAT"}` every 30 s or so.
//! Server → client: `{"t": "MESSAGE_CREATE", "d": {...}}` and friends.

use serde::{Deserialize, Serialize};

use crate::id::{as_string, option_as_string};
use crate::models::{
    Channel, ComponentInteractionData, DmChannel, DmUser, Guild, Member, Message, RaidEvent,
    RaidSettings, ScheduledMessage, ThreadInfo, User,
};

fn slash_command_type() -> u8 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", content = "d", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientEvent {
    Identify {
        token: String,
    },
    Heartbeat,
    /// The tab is hidden or nobody has touched it for a while (`idle: true`),
    /// or someone's back (`false`). Push notifications go to people whose
    /// connections are all idle.
    Idle {
        idle: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", content = "d", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ServerEvent {
    /// Sent once after a successful IDENTIFY.
    Ready {
        user: User,
        guilds: Vec<Guild>,
    },
    HeartbeatAck,
    /// You joined or created a server.
    GuildCreate(Guild),
    /// You left, were kicked or banned, or the server was deleted.
    GuildDelete {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    /// An outhouse's name or picture changed (sent to its members).
    GuildUpdate(Guild),
    /// The site owner shut this outhouse down (sent to its members): show
    /// it closed until `remove_at`, when members are removed.
    GuildShutDown {
        #[serde(with = "as_string")]
        guild_id: i64,
        name: String,
        remove_at: chrono::DateTime<chrono::Utc>,
    },
    /// Your rail folders or outhouse order changed on another device:
    /// re-fetch them (sent to you only).
    GuildLayoutChanged,
    /// Roles, member roles or channel permissions changed: re-fetch
    /// channels, roles, members and your permissions.
    GuildRefresh {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    ChannelCreate(Channel),
    /// A channel's name, topic, slowmode or content setting changed.
    ChannelUpdate(Channel),
    /// You said you're 18+ for an age-restricted channel, or took it back
    /// (sent to all of your own clients).
    ChannelAgeGate {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        accepted: bool,
    },
    ChannelDelete {
        #[serde(with = "as_string")]
        id: i64,
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    MessageCreate(Message),
    MessageUpdate(Message),
    MessageDelete {
        #[serde(with = "as_string")]
        id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
    },
    /// You marked a channel as read (sent to all of your own clients).
    ChannelAck {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        #[serde(with = "as_string")]
        last_read_id: i64,
    },
    ReactionAdd {
        #[serde(with = "as_string")]
        channel_id: i64,
        #[serde(with = "as_string")]
        message_id: i64,
        #[serde(with = "as_string")]
        user_id: i64,
        emoji: String,
    },
    ReactionRemove {
        #[serde(with = "as_string")]
        channel_id: i64,
        #[serde(with = "as_string")]
        message_id: i64,
        #[serde(with = "as_string")]
        user_id: i64,
        emoji: String,
    },
    TypingStart {
        #[serde(with = "as_string")]
        channel_id: i64,
        #[serde(with = "as_string")]
        user_id: i64,
        display_name: String,
    },
    MemberJoin {
        #[serde(with = "as_string")]
        guild_id: i64,
        member: Member,
    },
    MemberLeave {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        user_id: i64,
    },
    /// Someone in one of your servers came online or went offline.
    PresenceUpdate {
        #[serde(with = "as_string")]
        user_id: i64,
        online: bool,
    },
    /// Someone joined, left or moved voice, or muted/deafened.
    /// `channel_id` is null when they left.
    VoiceStateUpdate {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        user_id: i64,
        #[serde(with = "option_as_string")]
        channel_id: Option<i64>,
        muted: bool,
        deafened: bool,
        #[serde(default)]
        server_muted: bool,
        #[serde(default)]
        server_deafened: bool,
        #[serde(default)]
        streaming: bool,
    },
    /// A moderator moved you to another voice channel: switch to it
    /// (sent to you only).
    VoiceMove {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
    },
    /// A moderator disconnected you from voice (sent to you only).
    VoiceKicked {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
    },
    /// Someone joined a voice channel and has an entrance sound: people in
    /// that channel play it.
    EntranceSound {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        #[serde(with = "as_string")]
        user_id: i64,
        url: String,
    },
    /// Play a soundboard clip (clients in that voice channel play it).
    SoundboardPlay {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        #[serde(with = "as_string")]
        user_id: i64,
        /// A snowflake, or `builtin:<key>`.
        sound_id: String,
        url: String,
        volume: f32,
    },
    /// The soundboard library changed: re-fetch it.
    SoundsChanged {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    /// Slash commands in this outhouse changed: re-fetch them.
    CommandsChanged {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    /// Sent to a bot when someone uses one of its slash commands (`type` 2)
    /// or clicks a button / picks from a select menu on one of its messages
    /// (`type` 3, with `message` and `data.custom_id` / `data.values`).
    /// Answer with `POST /api/interactions/{id}/callback` (or `/reply`)
    /// within 3 seconds to avoid "This interaction failed" (it stays
    /// answerable for 15 minutes).
    InteractionCreate {
        #[serde(with = "as_string")]
        id: i64,
        /// 2 = slash command, 3 = message component.
        #[serde(rename = "type", default = "slash_command_type")]
        kind: u8,
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        user: User,
        /// The slash command's name ("" for components).
        command: String,
        options: std::collections::HashMap<String, String>,
        /// Components: the message the button or select menu is on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<Box<Message>>,
        /// Components: which one was used and what was picked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ComponentInteractionData>,
    },
    /// The bot answered your button click / select menu / command (sent to
    /// you only), so the client stops showing it as loading.
    InteractionSuccess {
        #[serde(with = "as_string")]
        id: i64,
    },
    /// A thread was started or changed (new reply, renamed).
    ThreadUpdate(ThreadInfo),
    ThreadDelete {
        #[serde(with = "as_string")]
        id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
    },
    /// You read a thread up to a reply (sent to all of your own clients).
    ThreadAck {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        #[serde(with = "as_string")]
        thread_id: i64,
        #[serde(with = "as_string")]
        last_read_id: i64,
    },
    /// You muted or unmuted a channel or DM (sent to all of your own
    /// clients). `until`: when the mute wears off (none: until unmuted).
    ChannelMuteUpdate {
        #[serde(with = "as_string")]
        channel_id: i64,
        muted: bool,
        #[serde(default)]
        until: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// A DM you're in was created or changed (renamed, people added or left).
    DmUpdate(DmChannel),
    /// You left a group DM.
    DmDelete {
        #[serde(with = "as_string")]
        id: i64,
    },
    /// How many message requests are waiting for you (DMs from people who
    /// aren't your friends and share no outhouse with you). Sent to you only.
    MessageRequests {
        count: i64,
    },
    /// Custom emoji in this outhouse changed: re-fetch them.
    EmojisChanged {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    /// A raid was planned or changed (details, sign-ups, attendance). Raid
    /// cards also get a MESSAGE_UPDATE; this one is for calendars and lists.
    EventUpdate(RaidEvent),
    /// A raid was cancelled.
    EventDelete {
        #[serde(with = "as_string")]
        id: i64,
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    /// You were moved off the bench and onto a raid's roster (sent to you only).
    RaidPromoted {
        #[serde(with = "as_string")]
        event_id: i64,
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        title: String,
        role: String,
    },
    /// Badges in this outhouse changed (made, edited, given or taken away):
    /// re-fetch them.
    BadgesChanged {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    /// The outhouse's raid settings changed (raid manager roles, default
    /// sign-up channel).
    RaidSettingsUpdate {
        #[serde(with = "as_string")]
        guild_id: i64,
        settings: RaidSettings,
    },
    /// Your relationship with someone changed (sent to you only): `kind`
    /// is "friend", "incoming", "outgoing", "blocked" or "none".
    RelationshipUpdate {
        kind: String,
        user: DmUser,
    },
    /// Stickers in this outhouse changed: re-fetch them.
    StickersChanged {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    /// One of your scheduled messages was made, changed or couldn't be
    /// sent (sent to you only).
    ScheduledUpdate(ScheduledMessage),
    /// One of your scheduled messages was sent or cancelled (sent to you only).
    ScheduledDelete {
        #[serde(with = "as_string")]
        id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
    },
    /// Safety reports changed (sent to the site owner only): how many are
    /// waiting, how many are child-safety reports, and whether this is a new
    /// urgent one (show an alert; `report_id` is the report to open).
    ReportsChanged {
        open: i64,
        urgent: i64,
        alert: bool,
        #[serde(default, with = "option_as_string")]
        report_id: Option<i64>,
    },
    /// Identify failed, the session expired or the account was suspended;
    /// sign in again.
    InvalidSession {
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_events_parse() {
        let e: ClientEvent = serde_json::from_str(r#"{"op":"HEARTBEAT"}"#).unwrap();
        assert_eq!(e, ClientEvent::Heartbeat);
        let e: ClientEvent = serde_json::from_str(r#"{"op":"IDLE","d":{"idle":true}}"#).unwrap();
        assert_eq!(e, ClientEvent::Idle { idle: true });
        let e: ClientEvent =
            serde_json::from_str(r#"{"op":"IDENTIFY","d":{"token":"abc"}}"#).unwrap();
        assert_eq!(
            e,
            ClientEvent::Identify {
                token: "abc".into()
            }
        );
    }

    #[test]
    fn ids_serialise_as_strings() {
        let e = ServerEvent::MemberLeave {
            guild_id: 42,
            user_id: 7,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""t":"MEMBER_LEAVE""#));
        assert!(json.contains(r#""guild_id":"42""#));
        assert!(json.contains(r#""user_id":"7""#));
    }
}
