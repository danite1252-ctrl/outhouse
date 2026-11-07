//! Messages sent over the real-time WebSocket gateway (`/api/gateway`).
//!
//! Client → server: `{"op": "IDENTIFY", "d": {"token": "..."}}`, then
//! `{"op": "HEARTBEAT"}` every 30 s or so.
//! Server → client: `{"t": "MESSAGE_CREATE", "d": {...}}` and friends.

use serde::{Deserialize, Serialize};

use crate::id::{as_string, option_as_string};
use crate::models::{Channel, DmChannel, Guild, Member, Message, ThreadInfo, User};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", content = "d", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientEvent {
    Identify { token: String },
    Heartbeat,
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
    /// Roles, member roles or channel permissions changed: re-fetch
    /// channels, roles, members and your permissions.
    GuildRefresh {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    ChannelCreate(Channel),
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
    },
    /// Play a soundboard clip (clients in that voice channel play it).
    SoundboardPlay {
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        #[serde(with = "as_string")]
        user_id: i64,
        #[serde(with = "as_string")]
        sound_id: i64,
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
    /// Sent to a bot when someone uses one of its slash commands. Reply with
    /// `POST /api/interactions/{id}/reply` within 15 minutes.
    InteractionCreate {
        #[serde(with = "as_string")]
        id: i64,
        #[serde(with = "as_string")]
        guild_id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
        user: User,
        command: String,
        options: std::collections::HashMap<String, String>,
    },
    /// A thread was started or changed (new reply, renamed).
    ThreadUpdate(ThreadInfo),
    ThreadDelete {
        #[serde(with = "as_string")]
        id: i64,
        #[serde(with = "as_string")]
        channel_id: i64,
    },
    /// A DM you're in was created or changed (renamed, people added or left).
    DmUpdate(DmChannel),
    /// You left a group DM.
    DmDelete {
        #[serde(with = "as_string")]
        id: i64,
    },
    /// Custom emoji in this outhouse changed: re-fetch them.
    EmojisChanged {
        #[serde(with = "as_string")]
        guild_id: i64,
    },
    /// Identify failed or the session expired; sign in again.
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
