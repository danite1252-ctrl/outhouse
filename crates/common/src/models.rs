//! Data shapes used by the REST API and the gateway.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{as_string, option_as_string, vec_as_string};

/// A user as other people see them (never includes the password hash).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct User {
    #[serde(with = "as_string")]
    pub id: i64,
    pub username: String,
    pub display_name: String,
    /// Uploaded profile picture, if any.
    #[serde(default)]
    #[cfg_attr(feature = "sqlx", sqlx(default))]
    pub avatar_url: Option<String>,
    /// A bot account (webhook, feed or bot app).
    #[serde(default)]
    #[cfg_attr(feature = "sqlx", sqlx(default))]
    pub bot: bool,
}

/// A server ("outhouse"). Called a guild in the code, like Discord's API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Guild {
    #[serde(with = "as_string")]
    pub id: i64,
    pub name: String,
    #[serde(with = "as_string")]
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
}

/// A member of a server: the user plus their server-specific details.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Member {
    #[serde(with = "as_string")]
    pub id: i64,
    pub username: String,
    /// Nickname if set, otherwise their display name.
    pub display_name: String,
    #[serde(with = "vec_as_string")]
    pub role_ids: Vec<i64>,
    pub joined_at: DateTime<Utc>,
    pub timeout_until: Option<DateTime<Utc>>,
    /// Connected right now.
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    #[cfg_attr(feature = "sqlx", sqlx(default))]
    pub avatar_url: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "sqlx", sqlx(default))]
    pub bot: bool,
}

/// A role. The @everyone role has the same ID as its server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Role {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub guild_id: i64,
    pub name: String,
    /// 0xRRGGBB, or 0 for no colour.
    pub color: i32,
    /// Permissions this role switches on (see `permissions`), as a string in JSON.
    #[serde(with = "as_string")]
    pub permissions: i64,
    /// Permissions this role switches off. Bits in neither are inherited.
    #[serde(with = "as_string", default)]
    pub deny_permissions: i64,
    /// Higher = more senior. @everyone is 0.
    pub position: i32,
    /// Shown as its own group in the member list.
    pub hoist: bool,
}

pub mod channel_kind {
    pub const TEXT: i16 = 0;
    /// A one-to-one direct message.
    pub const DM: i16 = 1;
    pub const VOICE: i16 = 2;
    /// A group direct message (up to [`super::MAX_GROUP_DM`] people).
    pub const GROUP_DM: i16 = 3;
}

/// The outhouse ID direct-message channels report (they don't belong to one).
pub const DM_GUILD: i64 = 0;

/// Most people in a group DM, including you.
pub const MAX_GROUP_DM: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Channel {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub guild_id: i64,
    pub name: String,
    /// See [`channel_kind`].
    pub kind: i16,
    pub position: i32,
    #[serde(with = "option_as_string", default)]
    pub last_message_id: Option<i64>,
    /// The newest message this user has seen (only in your own channel list).
    #[serde(with = "option_as_string", default)]
    pub last_read_id: Option<i64>,
}

/// A channel permission overwrite as shown in settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionOverwrite {
    #[serde(with = "as_string")]
    pub target_id: i64,
    /// "role" or "member".
    pub kind: String,
    #[serde(with = "as_string")]
    pub allow: i64,
    #[serde(with = "as_string")]
    pub deny: i64,
}

/// The message a reply points at (shortened).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageRef {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub author_id: i64,
    pub author_display_name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attachment {
    #[serde(with = "as_string")]
    pub id: i64,
    pub filename: String,
    pub content_type: String,
    pub size: i64,
    /// Relative URL to download or display the file.
    pub url: String,
}

/// A link preview, an inline image/GIF, or a rich card from a webhook or feed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Embed {
    pub url: String,
    /// "link", "image" or "rich".
    pub kind: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    pub image_url: Option<String>,
    /// Colour bar, 0xRRGGBB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_icon_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// ISO 8601 time shown in the footer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reaction {
    pub emoji: String,
    #[serde(with = "vec_as_string")]
    pub user_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    #[serde(with = "as_string")]
    pub author_id: i64,
    pub author_username: String,
    pub author_display_name: String,
    /// Set for webhook/feed posts that bring their own picture.
    pub author_avatar_url: Option<String>,
    /// Posted by a webhook or feed rather than a person.
    pub author_bot: bool,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub reply_to: Option<MessageRef>,
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub reactions: Vec<Reaction>,
    /// For a bot's reply to a slash command: who used which command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction: Option<MessageInteraction>,
    /// For a raid post: the event and its sign-ups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<RaidEvent>,
    /// When it was pinned to its channel (null = not pinned).
    #[serde(default)]
    pub pinned_at: Option<DateTime<Utc>>,
    /// Set for messages posted inside a thread.
    #[serde(
        with = "option_as_string",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub thread_id: Option<i64>,
    /// The thread started from this message, if any ("N replies").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadInfo>,
    /// For a poll: the question, options and votes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<Poll>,
    /// For a group finder post: the group and who's in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lfg: Option<crate::wow::LfgGroup>,
}

/// A thread: a side conversation off a channel (often off one message).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct ThreadInfo {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    #[serde(with = "option_as_string", default)]
    pub starter_message_id: Option<i64>,
    pub name: String,
    #[serde(with = "option_as_string", default)]
    pub creator_id: Option<i64>,
    pub message_count: i32,
    #[serde(with = "option_as_string", default)]
    pub last_message_id: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollOption {
    pub text: String,
    /// Who picked this option.
    #[serde(with = "vec_as_string")]
    pub user_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Poll {
    pub question: String,
    pub options: Vec<PollOption>,
    /// People can pick more than one option.
    pub multi: bool,
    pub ends_at: Option<DateTime<Utc>>,
    /// No more votes (ended or closed early).
    pub closed: bool,
}

/// Someone in a direct message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DmUser {
    #[serde(with = "as_string")]
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub bot: bool,
    pub online: bool,
}

/// A direct-message conversation (one-to-one or group).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DmChannel {
    #[serde(with = "as_string")]
    pub id: i64,
    /// 1 = one-to-one, 3 = group.
    pub kind: i16,
    /// Group name ("" = named after the people in it).
    pub name: String,
    #[serde(with = "option_as_string", default)]
    pub owner_id: Option<i64>,
    /// Everyone in it, including you.
    pub recipients: Vec<DmUser>,
    #[serde(with = "option_as_string", default)]
    pub last_message_id: Option<i64>,
    #[serde(with = "option_as_string", default)]
    pub last_read_id: Option<i64>,
}

/// A custom emoji uploaded to an outhouse. In messages it's written
/// `<:name:id>` (type `:name:` and the server swaps it in).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomEmoji {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub guild_id: i64,
    pub name: String,
    pub url: String,
    pub animated: bool,
}

/// Message search results (newest first).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResults {
    /// How many messages match in total.
    pub total: i64,
    pub messages: Vec<Message>,
    /// The words searched for, for highlighting.
    pub terms: Vec<String>,
}

/// Unread state for one text channel you can see.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct ChannelUnread {
    #[serde(with = "as_string")]
    pub channel_id: i64,
    #[serde(with = "as_string")]
    pub guild_id: i64,
    #[serde(with = "option_as_string", default)]
    pub last_message_id: Option<i64>,
    #[serde(with = "option_as_string", default)]
    pub last_read_id: Option<i64>,
    /// Unread messages mentioning you, your roles, @everyone or @here, or
    /// replying to you (counted up to 100).
    pub mention_count: i64,
}

/// Your roles in one outhouse (so clients can spot role mentions).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildRoleIds {
    #[serde(with = "as_string")]
    pub guild_id: i64,
    #[serde(with = "vec_as_string")]
    pub role_ids: Vec<i64>,
}

/// `GET /api/users/@me/unread`: unread and mention counts everywhere.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnreadState {
    pub channels: Vec<ChannelUnread>,
    pub roles: Vec<GuildRoleIds>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageInteraction {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    /// Command name without the slash.
    pub command: String,
}

/// A raid (or any event) people sign up for, shown as a card in chat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RaidEvent {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub guild_id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    /// The chat message with its sign-up card. Raids in a repeating series
    /// get their card a week before they start; until then this is null.
    #[serde(with = "option_as_string", default)]
    pub message_id: Option<i64>,
    #[serde(with = "as_string")]
    pub creator_id: i64,
    pub title: String,
    pub description: String,
    pub starts_at: DateTime<Utc>,
    pub duration_minutes: i32,
    pub max_tanks: i32,
    pub max_healers: i32,
    pub max_dps: i32,
    pub signups: Vec<EventSignup>,
    /// The repeating series this raid belongs to, if any.
    #[serde(with = "option_as_string", default)]
    pub series_id: Option<i64>,
    #[serde(default)]
    pub repeat: Option<RepeatInfo>,
    /// Sign-ups lock this many minutes before the start (null: they don't).
    #[serde(default)]
    pub signup_deadline_minutes: Option<i32>,
    /// When sign-ups lock, if they do.
    #[serde(default)]
    pub signups_close_at: Option<DateTime<Utc>>,
    /// The raid leader has marked who turned up.
    #[serde(default)]
    pub attendance_taken: bool,
}

/// How a repeating raid repeats.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepeatInfo {
    /// 0 = Monday … 6 = Sunday.
    pub weekdays: Vec<u8>,
    /// 1 = every week, 2 = every second week, …
    pub every_weeks: i32,
    /// Last day it can fall on (in `time_zone`).
    #[serde(default)]
    pub until: Option<NaiveDate>,
    /// How many raids in total.
    #[serde(default)]
    pub count: Option<i32>,
    /// IANA time zone the raid time is kept in (so daylight saving doesn't move it).
    pub time_zone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventSignup {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    /// "yes", "late", "tentative", "bench" or "no".
    pub status: String,
    /// "tank", "healer", "dps" or "".
    pub role: String,
    pub class: String,
    pub spec: String,
    pub note: String,
    pub signed_at: DateTime<Utc>,
    /// On the bench because their role was full; moved up automatically
    /// (in sign-up order) when a spot opens.
    #[serde(default)]
    pub auto_bench: bool,
}

/// Everything shown on someone's profile card.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    #[serde(with = "as_string")]
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub banner_url: Option<String>,
    pub about_me: String,
    pub profile_theme: String,
    pub accent_color: i32,
    pub bot: bool,
    pub created_at: DateTime<Utc>,
    /// Their WoW characters (main first).
    #[serde(default)]
    pub characters: Vec<crate::wow::WowCharacter>,
}

/// A bot made in an outhouse (shown to people who can manage it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BotInfo {
    pub user: User,
    #[serde(with = "as_string")]
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    /// Connected to the gateway right now.
    pub online: bool,
    /// Its slash commands in this outhouse.
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandOption {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// A slash command a bot offers in an outhouse.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlashCommand {
    #[serde(with = "as_string", default)]
    pub id: i64,
    #[serde(with = "as_string", default)]
    pub bot_id: i64,
    #[serde(default)]
    pub bot_name: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub options: Vec<CommandOption>,
}

/// Your permissions in a server, for showing and hiding buttons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MyPermissions {
    #[serde(with = "as_string")]
    pub guild: i64,
    /// Channel ID → permission bits, both as strings.
    pub channels: std::collections::HashMap<String, String>,
    /// Your most senior role's position (owner: very large).
    pub top_position: i32,
}

/// A GIF search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gif {
    pub id: String,
    pub title: String,
    /// Small animated preview for the picker.
    pub preview_url: String,
    /// The GIF to post.
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// A webhook (only shown to people who can manage webhooks: the token is
/// what lets anyone post).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Webhook {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub guild_id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    pub name: String,
    pub token: String,
    pub created_at: DateTime<Utc>,
}

/// An RSS/Atom feed being followed in a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Feed {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub guild_id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub url: String,
    pub title: String,
    pub include_keywords: String,
    pub exclude_keywords: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Who is in which voice channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceState {
    #[serde(with = "as_string")]
    pub guild_id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub muted: bool,
    pub deafened: bool,
    /// Muted by a moderator (kept when they leave and come back).
    #[serde(default)]
    pub server_muted: bool,
    /// Deafened by a moderator.
    #[serde(default)]
    pub server_deafened: bool,
    /// Sharing their screen.
    #[serde(default)]
    pub streaming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceJoinResponse {
    /// LiveKit server address (ws:// or wss://).
    pub url: String,
    pub token: String,
    pub can_speak: bool,
    /// Allowed to share their screen here.
    #[serde(default)]
    pub can_stream: bool,
    #[serde(default)]
    pub server_muted: bool,
    #[serde(default)]
    pub server_deafened: bool,
}

/// A moderator's change to someone in voice: `PATCH /api/guilds/{id}/voice/{user}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceModerateRequest {
    /// Server mute (true) or unmute (false).
    #[serde(default)]
    pub mute: Option<bool>,
    #[serde(default)]
    pub deafen: Option<bool>,
    /// Move them to this voice channel.
    #[serde(default, with = "option_as_string")]
    pub channel_id: Option<i64>,
}

/// Your own entrance sound.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntranceSound {
    pub url: String,
    pub duration_ms: i32,
}

/// A soundboard clip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sound {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub guild_id: i64,
    pub name: String,
    pub emoji: String,
    pub url: String,
    pub volume: f32,
    /// In your favourites.
    #[serde(default)]
    pub favourite: bool,
}

// ---- Requests and responses -------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEventRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub starts_at: DateTime<Utc>,
    pub duration_minutes: Option<i32>,
    pub max_tanks: Option<i32>,
    pub max_healers: Option<i32>,
    pub max_dps: Option<i32>,
    /// Lock sign-ups this many minutes before the start (0 or null: never).
    #[serde(default)]
    pub signup_deadline_minutes: Option<i32>,
    /// Make it a repeating raid (the first one is at `starts_at` or the
    /// first matching day after it).
    #[serde(default)]
    pub repeat: Option<RepeatInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateEventRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub starts_at: Option<DateTime<Utc>>,
    pub duration_minutes: Option<i32>,
    pub max_tanks: Option<i32>,
    pub max_healers: Option<i32>,
    pub max_dps: Option<i32>,
    /// 0 removes the deadline.
    #[serde(default)]
    pub signup_deadline_minutes: Option<i32>,
    /// For a repeating raid: "one" (default) or "future" (this and later ones).
    #[serde(default)]
    pub scope: Option<String>,
}

/// `PUT /api/events/{id}/attendance`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AttendanceRequest {
    /// First mark everyone signed up as present (bench: benched).
    #[serde(default)]
    pub all_signed_up: bool,
    #[serde(default)]
    pub records: Vec<AttendanceMark>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttendanceMark {
    #[serde(with = "as_string")]
    pub user_id: i64,
    /// "present", "late", "absent", "benched", or "" to clear.
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignupRequest {
    pub status: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub spec: String,
    #[serde(default)]
    pub note: String,
    /// Organisers can sign someone else up or move them (e.g. to the bench).
    #[serde(with = "option_as_string", default)]
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBotRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotTokenResponse {
    pub bot: User,
    /// Shown once. Use it as `Authorization: Bot <token>`.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionRequest {
    #[serde(with = "as_string")]
    pub command_id: i64,
    #[serde(default)]
    pub options: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionReply {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub embeds: Vec<Embed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStateRequest {
    pub muted: bool,
    pub deafened: bool,
    #[serde(default)]
    pub streaming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWebhookRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFeedRequest {
    pub url: String,
    #[serde(default)]
    pub include_keywords: String,
    #[serde(default)]
    pub exclude_keywords: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
}

/// Change your own account. Leave out anything you're not changing.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateMeRequest {
    pub display_name: Option<String>,
    pub about_me: Option<String>,
    pub profile_theme: Option<String>,
    pub accent_color: Option<i32>,
    /// Needed (with `new_password`) to change your password.
    pub current_password: Option<String>,
    pub new_password: Option<String>,
}

/// Your nickname in one outhouse. Empty or null clears it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NicknameRequest {
    pub nickname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGuildRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub kind: Option<i16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageRequest {
    pub content: String,
    #[serde(with = "option_as_string", default)]
    pub reply_to_id: Option<i64>,
    /// Post inside this thread of the channel.
    #[serde(with = "option_as_string", default)]
    pub thread_id: Option<i64>,
}

/// Start a DM (one other person) or a group DM (2–9 others).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDmRequest {
    #[serde(with = "vec_as_string")]
    pub user_ids: Vec<i64>,
    /// Name for a group DM.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDmRequest {
    /// New group name ("" = back to the people's names).
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddDmRecipientRequest {
    #[serde(with = "as_string")]
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateThreadRequest {
    pub name: String,
    /// Start it from this message (otherwise from the channel).
    #[serde(with = "option_as_string", default)]
    pub message_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePollRequest {
    pub question: String,
    pub options: Vec<String>,
    #[serde(default)]
    pub multi: bool,
    /// Close automatically after this many minutes (none = stays open).
    #[serde(default)]
    pub duration_minutes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollVoteRequest {
    /// Option numbers (from 0). Empty takes your vote back.
    pub options: Vec<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditMessageRequest {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckRequest {
    #[serde(with = "as_string")]
    pub message_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteResponse {
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoleRequest {
    pub name: String,
    pub color: Option<i32>,
    #[serde(with = "option_as_string", default)]
    pub permissions: Option<i64>,
    #[serde(with = "option_as_string", default)]
    pub deny_permissions: Option<i64>,
    pub hoist: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRoleRequest {
    pub name: Option<String>,
    pub color: Option<i32>,
    #[serde(with = "option_as_string", default)]
    pub permissions: Option<i64>,
    #[serde(with = "option_as_string", default)]
    pub deny_permissions: Option<i64>,
    pub hoist: Option<bool>,
    pub position: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetOverwriteRequest {
    /// "role" or "member".
    pub kind: String,
    #[serde(with = "as_string")]
    pub allow: i64,
    #[serde(with = "as_string")]
    pub deny: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutRequest {
    /// Minutes to time out for; 0 or missing clears the timeout.
    pub minutes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanRequest {
    pub reason: Option<String>,
}
