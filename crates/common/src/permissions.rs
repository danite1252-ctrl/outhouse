//! Discord-style permissions: bit flags on roles, plus per-channel
//! allow/deny overwrites for roles and individual members.
//!
//! How a member's permissions in a channel are worked out:
//! 1. Server owner: everything.
//! 2. Base = @everyone's permissions, then each of the member's roles from
//!    lowest to highest: its denies switch bits off, its allows switch bits
//!    on (so a higher role wins when roles disagree). Bits a role neither
//!    allows nor denies are inherited. Administrator in the result:
//!    everything.
//! 3. Channel overwrite for @everyone (deny, then allow).
//! 4. All overwrites for the member's roles combined (deny, then allow).
//! 5. Overwrite for the member themselves (deny, then allow).
//! 6. No View Channel: nothing at all.
//! 7. Timed-out members keep only View Channel and Read History.

use serde::Serialize;

pub const VIEW_CHANNEL: u64 = 1 << 0;
pub const SEND_MESSAGES: u64 = 1 << 1;
pub const READ_HISTORY: u64 = 1 << 2;
pub const EMBED_LINKS: u64 = 1 << 3;
pub const ATTACH_FILES: u64 = 1 << 4;
pub const ADD_REACTIONS: u64 = 1 << 5;
pub const MENTION_EVERYONE: u64 = 1 << 6;
pub const MANAGE_MESSAGES: u64 = 1 << 7;
pub const CREATE_INVITE: u64 = 1 << 8;
pub const CHANGE_NICKNAME: u64 = 1 << 9;

pub const CONNECT: u64 = 1 << 16;
pub const SPEAK: u64 = 1 << 17;
pub const USE_SOUNDBOARD: u64 = 1 << 18;
pub const MUTE_MEMBERS: u64 = 1 << 19;
pub const DEAFEN_MEMBERS: u64 = 1 << 20;
pub const MOVE_MEMBERS: u64 = 1 << 21;

pub const MANAGE_CHANNELS: u64 = 1 << 24;
pub const MANAGE_ROLES: u64 = 1 << 25;
pub const MANAGE_GUILD: u64 = 1 << 26;
pub const KICK_MEMBERS: u64 = 1 << 27;
pub const BAN_MEMBERS: u64 = 1 << 28;
pub const TIMEOUT_MEMBERS: u64 = 1 << 29;
pub const MANAGE_WEBHOOKS: u64 = 1 << 30;
pub const MANAGE_EVENTS: u64 = 1 << 31;
pub const MANAGE_SOUNDS: u64 = 1 << 32;
pub const VIEW_AUDIT_LOG: u64 = 1 << 33;

pub const ADMINISTRATOR: u64 = 1 << 40;

/// Every permission bit that exists.
pub const ALL: u64 = (1 << 41) - 1;

/// What @everyone can do in a brand-new outhouse.
pub const DEFAULT_EVERYONE: u64 = VIEW_CHANNEL
    | SEND_MESSAGES
    | READ_HISTORY
    | EMBED_LINKS
    | ATTACH_FILES
    | ADD_REACTIONS
    | CREATE_INVITE
    | CHANGE_NICKNAME
    | CONNECT
    | SPEAK
    | USE_SOUNDBOARD;

/// What a timed-out member keeps.
pub const TIMED_OUT_ALLOWED: u64 = VIEW_CHANNEL | READ_HISTORY;

/// Description of one permission, for settings screens.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionInfo {
    pub key: &'static str,
    #[serde(serialize_with = "bit_as_string")]
    pub bit: u64,
    pub name: &'static str,
    pub description: &'static str,
    /// Can be set per channel (otherwise server-wide only).
    pub channel: bool,
}

fn bit_as_string<S: serde::Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

macro_rules! info {
    ($key:literal, $bit:expr, $name:literal, $desc:literal, $channel:expr) => {
        PermissionInfo {
            key: $key,
            bit: $bit,
            name: $name,
            description: $desc,
            channel: $channel,
        }
    };
}

pub fn all_permissions() -> Vec<PermissionInfo> {
    vec![
        info!(
            "view_channel",
            VIEW_CHANNEL, "View channels", "See the channel and its messages.", true
        ),
        info!(
            "send_messages",
            SEND_MESSAGES, "Send messages", "Write on the wall.", true
        ),
        info!(
            "read_history",
            READ_HISTORY, "Read message history", "Scroll back through older messages.", true
        ),
        info!(
            "embed_links",
            EMBED_LINKS, "Embed links", "Links show a preview.", true
        ),
        info!(
            "attach_files",
            ATTACH_FILES, "Attach files", "Upload images and files.", true
        ),
        info!(
            "add_reactions",
            ADD_REACTIONS, "Add reactions", "React to messages with emoji.", true
        ),
        info!(
            "mention_everyone",
            MENTION_EVERYONE, "Mention @everyone", "Ping the whole outhouse.", true
        ),
        info!(
            "manage_messages",
            MANAGE_MESSAGES, "Manage messages", "Delete other people's messages.", true
        ),
        info!(
            "create_invite",
            CREATE_INVITE, "Create invites", "Invite people in.", false
        ),
        info!(
            "change_nickname",
            CHANGE_NICKNAME, "Change nickname", "Set their own nickname.", false
        ),
        info!(
            "connect",
            CONNECT, "Connect to voice", "Join voice channels.", true
        ),
        info!("speak", SPEAK, "Speak", "Talk in voice channels.", true),
        info!(
            "use_soundboard",
            USE_SOUNDBOARD, "Use soundboard", "Play soundboard clips in voice.", true
        ),
        info!(
            "mute_members",
            MUTE_MEMBERS, "Mute members", "Server-mute people in voice.", true
        ),
        info!(
            "deafen_members",
            DEAFEN_MEMBERS, "Deafen members", "Server-deafen people in voice.", true
        ),
        info!(
            "move_members",
            MOVE_MEMBERS, "Move members", "Move people between voice channels.", true
        ),
        info!(
            "manage_channels",
            MANAGE_CHANNELS, "Manage channels", "Create, edit and delete channels.", true
        ),
        info!(
            "manage_roles",
            MANAGE_ROLES,
            "Manage roles",
            "Create roles, hand them out and edit channel permissions.",
            false
        ),
        info!(
            "manage_guild",
            MANAGE_GUILD, "Manage outhouse", "Rename the outhouse and change its settings.", false
        ),
        info!(
            "kick_members",
            KICK_MEMBERS, "Kick members", "Show people the door.", false
        ),
        info!(
            "ban_members",
            BAN_MEMBERS, "Ban members", "Kick people out for good.", false
        ),
        info!(
            "timeout_members",
            TIMEOUT_MEMBERS, "Time out members", "Stop someone posting for a while.", false
        ),
        info!(
            "manage_webhooks",
            MANAGE_WEBHOOKS, "Manage webhooks", "Set up webhooks and news feeds.", true
        ),
        info!(
            "manage_events",
            MANAGE_EVENTS, "Manage events", "Create and run raid sign-ups.", true
        ),
        info!(
            "manage_sounds",
            MANAGE_SOUNDS, "Manage sounds", "Add and remove soundboard clips.", false
        ),
        info!(
            "view_audit_log",
            VIEW_AUDIT_LOG, "View audit log", "See who changed what.", false
        ),
        info!(
            "administrator",
            ADMINISTRATOR,
            "Administrator",
            "Every permission, everywhere. Hand out carefully.",
            false
        ),
    ]
}

/// A channel permission overwrite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overwrite {
    /// A role ID (the @everyone role's ID is the server's ID) or a user ID.
    pub target_id: i64,
    pub is_member: bool,
    pub allow: u64,
    pub deny: u64,
}

/// One of a member's roles (not @everyone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RolePerms {
    pub allow: u64,
    pub deny: u64,
    pub position: i32,
}

/// Server-wide permissions before channel overwrites.
pub fn base_permissions(is_owner: bool, everyone: u64, roles: &[RolePerms]) -> u64 {
    if is_owner {
        return ALL;
    }
    let mut ordered = roles.to_vec();
    ordered.sort_by_key(|r| r.position);
    let perms = ordered
        .iter()
        .fold(everyone, |acc, r| (acc & !r.deny) | r.allow);
    if perms & ADMINISTRATOR != 0 {
        ALL
    } else {
        perms
    }
}

/// Permissions in one channel.
pub fn channel_permissions(
    base: u64,
    guild_id: i64,
    user_id: i64,
    member_role_ids: &[i64],
    overwrites: &[Overwrite],
) -> u64 {
    if base & ADMINISTRATOR != 0 {
        return ALL;
    }
    let mut perms = base;

    if let Some(o) = overwrites
        .iter()
        .find(|o| !o.is_member && o.target_id == guild_id)
    {
        perms &= !o.deny;
        perms |= o.allow;
    }

    let (mut allow, mut deny) = (0u64, 0u64);
    for o in overwrites.iter().filter(|o| {
        !o.is_member && o.target_id != guild_id && member_role_ids.contains(&o.target_id)
    }) {
        allow |= o.allow;
        deny |= o.deny;
    }
    perms &= !deny;
    perms |= allow;

    if let Some(o) = overwrites
        .iter()
        .find(|o| o.is_member && o.target_id == user_id)
    {
        perms &= !o.deny;
        perms |= o.allow;
    }

    if perms & VIEW_CHANNEL == 0 {
        0
    } else {
        perms
    }
}

/// Strips everything but reading from a timed-out member (admins are immune).
pub fn apply_timeout(perms: u64, timed_out: bool) -> u64 {
    if timed_out && perms & ADMINISTRATOR == 0 {
        perms & TIMED_OUT_ALLOWED
    } else {
        perms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUILD: i64 = 100;
    const MOD_ROLE: i64 = 200;
    const USER: i64 = 300;

    #[test]
    fn owner_gets_everything() {
        assert_eq!(base_permissions(true, 0, &[]), ALL);
    }

    fn role(allow: u64, deny: u64, position: i32) -> RolePerms {
        RolePerms {
            allow,
            deny,
            position,
        }
    }

    #[test]
    fn administrator_gets_everything() {
        assert_eq!(
            base_permissions(false, VIEW_CHANNEL, &[role(ADMINISTRATOR, 0, 1)]),
            ALL
        );
    }

    #[test]
    fn roles_add_to_everyone() {
        let p = base_permissions(false, VIEW_CHANNEL, &[role(KICK_MEMBERS, 0, 1)]);
        assert_eq!(p, VIEW_CHANNEL | KICK_MEMBERS);
    }

    #[test]
    fn new_roles_inherit_everyone() {
        assert_eq!(
            base_permissions(false, DEFAULT_EVERYONE, &[role(0, 0, 1)]),
            DEFAULT_EVERYONE
        );
    }

    #[test]
    fn roles_can_deny() {
        let p = base_permissions(false, DEFAULT_EVERYONE, &[role(0, ATTACH_FILES, 1)]);
        assert_eq!(p & ATTACH_FILES, 0);
        assert!(p & SEND_MESSAGES != 0);
    }

    #[test]
    fn higher_role_wins() {
        let trial = role(0, ATTACH_FILES, 1);
        let raider = role(ATTACH_FILES, 0, 2);
        // Order given doesn't matter; position does.
        let p = base_permissions(false, DEFAULT_EVERYONE, &[raider, trial]);
        assert!(p & ATTACH_FILES != 0);
        let muted_vip = role(0, SEND_MESSAGES, 5);
        let p = base_permissions(false, DEFAULT_EVERYONE, &[raider, muted_vip, trial]);
        assert_eq!(p & SEND_MESSAGES, 0);
    }

    #[test]
    fn private_channel_for_mods() {
        let base = base_permissions(false, DEFAULT_EVERYONE, &[]);
        let overwrites = [
            Overwrite {
                target_id: GUILD,
                is_member: false,
                allow: 0,
                deny: VIEW_CHANNEL,
            },
            Overwrite {
                target_id: MOD_ROLE,
                is_member: false,
                allow: VIEW_CHANNEL,
                deny: 0,
            },
        ];
        // Regular member can't see it at all.
        assert_eq!(channel_permissions(base, GUILD, USER, &[], &overwrites), 0);
        // A mod can.
        let p = channel_permissions(base, GUILD, USER, &[MOD_ROLE], &overwrites);
        assert!(p & VIEW_CHANNEL != 0 && p & SEND_MESSAGES != 0);
    }

    #[test]
    fn member_overwrite_beats_role_overwrite() {
        let base = DEFAULT_EVERYONE;
        let overwrites = [
            Overwrite {
                target_id: MOD_ROLE,
                is_member: false,
                allow: 0,
                deny: SEND_MESSAGES,
            },
            Overwrite {
                target_id: USER,
                is_member: true,
                allow: SEND_MESSAGES,
                deny: 0,
            },
        ];
        let p = channel_permissions(base, GUILD, USER, &[MOD_ROLE], &overwrites);
        assert!(p & SEND_MESSAGES != 0);
    }

    #[test]
    fn read_only_announcements() {
        let overwrites = [Overwrite {
            target_id: GUILD,
            is_member: false,
            allow: 0,
            deny: SEND_MESSAGES,
        }];
        let p = channel_permissions(DEFAULT_EVERYONE, GUILD, USER, &[], &overwrites);
        assert!(p & VIEW_CHANNEL != 0 && p & SEND_MESSAGES == 0);
    }

    #[test]
    fn timeout_leaves_reading_only() {
        assert_eq!(
            apply_timeout(DEFAULT_EVERYONE, true),
            VIEW_CHANNEL | READ_HISTORY
        );
        assert_eq!(apply_timeout(ALL, true), ALL);
        assert_eq!(apply_timeout(DEFAULT_EVERYONE, false), DEFAULT_EVERYONE);
    }

    #[test]
    fn default_matches_migration() {
        // migrations/0002 hard-codes this number for existing servers.
        assert_eq!(DEFAULT_EVERYONE, 459_583);
    }

    #[test]
    fn permission_bits_are_unique() {
        let all = all_permissions();
        let mut seen = 0u64;
        for p in &all {
            assert_eq!(seen & p.bit, 0, "{} reuses a bit", p.key);
            seen |= p.bit;
        }
    }
}
