//! Data shapes used by the REST API and the gateway.

use chrono::{DateTime, Utc};
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
    pub const VOICE: i16 = 2;
}

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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceJoinResponse {
    /// LiveKit server address (ws:// or wss://).
    pub url: String,
    pub token: String,
    pub can_speak: bool,
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
}

// ---- Requests and responses -------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStateRequest {
    pub muted: bool,
    pub deafened: bool,
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
