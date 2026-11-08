//! Loading a member's roles and working out what they're allowed to do.

use outhouse_common::models::{Channel, DM_GUILD};
use outhouse_common::permissions::{self as p, Overwrite, RolePerms};
use sqlx::PgPool;

use crate::error::{AppError, AppResult};

/// Everything needed to check one member's permissions in one server.
#[derive(Debug, Clone)]
pub struct MemberCtx {
    pub guild_id: i64,
    pub user_id: i64,
    pub owner_id: i64,
    pub role_ids: Vec<i64>,
    /// Server-wide permissions (before channel overwrites and timeouts).
    pub base: u64,
    /// Position of their most senior role (owner: `i32::MAX`).
    pub top_position: i32,
    pub timed_out: bool,
}

impl MemberCtx {
    pub fn is_owner(&self) -> bool {
        self.user_id == self.owner_id
    }

    /// Server-wide permissions right now.
    pub fn guild_perms(&self) -> u64 {
        p::apply_timeout(self.base, self.timed_out)
    }

    pub fn require(&self, perm: u64) -> AppResult<()> {
        if self.guild_perms() & perm == perm {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }

    /// Can this member act on someone/something at `position`?
    /// (You can only manage roles and members below your top role.)
    pub fn outranks(&self, position: i32) -> bool {
        self.is_owner() || self.top_position > position
    }

    /// Rejects granting permissions you don't have yourself.
    pub fn require_can_grant(&self, bits: u64) -> AppResult<()> {
        let mine = self.guild_perms();
        if mine & p::ADMINISTRATOR != 0 || bits & !mine == 0 {
            Ok(())
        } else {
            Err(AppError::BadRequest(
                "You can't hand out permissions you don't have.".into(),
            ))
        }
    }
}

#[derive(sqlx::FromRow)]
struct MembershipRow {
    owner_id: i64,
    timed_out: bool,
}

#[derive(sqlx::FromRow)]
struct RoleRow {
    id: i64,
    permissions: i64,
    deny_permissions: i64,
    position: i32,
}

/// Loads a member's context. Not a member → NotFound.
pub async fn member_ctx(db: &PgPool, guild_id: i64, user_id: i64) -> AppResult<MemberCtx> {
    let membership = sqlx::query_as::<_, MembershipRow>(
        "SELECT g.owner_id,
                (m.timeout_until IS NOT NULL AND m.timeout_until > now()) AS timed_out
         FROM guilds g
         JOIN guild_members m ON m.guild_id = g.id AND m.user_id = $2
         WHERE g.id = $1",
    )
    .bind(guild_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;

    let roles = sqlx::query_as::<_, RoleRow>(
        "SELECT id, permissions, deny_permissions, position FROM roles
         WHERE guild_id = $1
           AND (id = $1 OR id IN (SELECT role_id FROM member_roles
                                  WHERE guild_id = $1 AND user_id = $2))",
    )
    .bind(guild_id)
    .bind(user_id)
    .fetch_all(db)
    .await?;

    let everyone = roles
        .iter()
        .find(|r| r.id == guild_id)
        .map(|r| r.permissions as u64)
        .unwrap_or(p::DEFAULT_EVERYONE);
    let others: Vec<&RoleRow> = roles.iter().filter(|r| r.id != guild_id).collect();
    let role_perms: Vec<RolePerms> = others
        .iter()
        .map(|r| RolePerms {
            allow: r.permissions as u64,
            deny: r.deny_permissions as u64,
            position: r.position,
        })
        .collect();
    let is_owner = membership.owner_id == user_id;

    Ok(MemberCtx {
        guild_id,
        user_id,
        owner_id: membership.owner_id,
        role_ids: others.iter().map(|r| r.id).collect(),
        base: p::base_permissions(is_owner, everyone, &role_perms),
        top_position: if is_owner {
            i32::MAX
        } else {
            others.iter().map(|r| r.position).max().unwrap_or(0)
        },
        timed_out: membership.timed_out,
    })
}

#[derive(sqlx::FromRow)]
struct OverwriteRow {
    target_id: i64,
    target_kind: i16,
    allow_bits: i64,
    deny_bits: i64,
}

pub async fn overwrites(db: &PgPool, channel_id: i64) -> AppResult<Vec<Overwrite>> {
    let rows = sqlx::query_as::<_, OverwriteRow>(
        "SELECT target_id, target_kind, allow_bits, deny_bits
         FROM channel_overwrites WHERE channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Overwrite {
            target_id: r.target_id,
            is_member: r.target_kind == 1,
            allow: r.allow_bits as u64,
            deny: r.deny_bits as u64,
        })
        .collect())
}

/// A member's permissions in one channel, given its overwrites.
pub fn channel_perms_with(ctx: &MemberCtx, overwrites: &[Overwrite]) -> u64 {
    let perms = p::channel_permissions(
        ctx.base,
        ctx.guild_id,
        ctx.user_id,
        &ctx.role_ids,
        overwrites,
    );
    p::apply_timeout(perms, ctx.timed_out)
}

pub async fn channel_perms(db: &PgPool, ctx: &MemberCtx, channel_id: i64) -> AppResult<u64> {
    Ok(channel_perms_with(ctx, &overwrites(db, channel_id).await?))
}

pub async fn fetch_channel(db: &PgPool, channel_id: i64) -> AppResult<Channel> {
    sqlx::query_as::<_, Channel>(
        "SELECT id, COALESCE(guild_id, 0) AS guild_id, name, kind, position, last_message_id,
                NULL::BIGINT AS last_read_id, topic, slowmode_secs, content_mode,
                FALSE AS nsfw_accepted
         FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)
}

/// A channel the user can see, with their context and permissions in it.
pub struct ChannelAccess {
    pub channel: Channel,
    pub ctx: MemberCtx,
    pub perms: u64,
}

impl ChannelAccess {
    pub fn require(&self, perm: u64) -> AppResult<()> {
        if self.perms & perm == perm {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

/// Loads a channel the user is allowed to see (otherwise NotFound), for
/// reading or posting its content: age-restricted channels the user hasn't
/// said they're 18+ for are refused with [`AppError::AgeRestricted`].
pub async fn channel_access(
    db: &PgPool,
    channel_id: i64,
    user_id: i64,
) -> AppResult<ChannelAccess> {
    let access = channel_access_meta(db, channel_id, user_id).await?;
    if is_age_gated(&access.channel) && !content_allowed(db, access.channel.id, user_id).await {
        return Err(AppError::AgeRestricted);
    }
    Ok(access)
}

/// An age-restricted text channel (DMs and voice channels never are).
pub fn is_age_gated(channel: &Channel) -> bool {
    channel.guild_id != DM_GUILD
        && channel.kind == outhouse_common::models::channel_kind::TEXT
        && channel.content_mode == outhouse_common::models::content_mode::NSFW
}

/// May this user see this channel's messages? False only for an
/// age-restricted channel they haven't accepted (bots always may).
/// Used for the gateway, push notifications and the API.
pub async fn content_allowed(db: &PgPool, channel_id: i64, user_id: i64) -> bool {
    let ok: Option<bool> = sqlx::query_scalar(
        "SELECT c.content_mode <> 'nsfw' OR c.kind <> 0 OR c.guild_id IS NULL
                OR EXISTS(SELECT 1 FROM users u WHERE u.id = $2 AND u.is_bot)
                OR EXISTS(SELECT 1 FROM channel_age_accepts a
                          WHERE a.channel_id = c.id AND a.user_id = $2)
         FROM channels c WHERE c.id = $1",
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    ok.unwrap_or(false)
}

/// Drops age-restricted channels the user hasn't accepted from a list of
/// channel IDs (one query).
pub async fn drop_age_gated(
    db: &PgPool,
    user_id: i64,
    channel_ids: &mut Vec<i64>,
) -> AppResult<()> {
    if channel_ids.is_empty() {
        return Ok(());
    }
    let gated: Vec<i64> = sqlx::query_scalar(
        "SELECT c.id FROM channels c
         WHERE c.id = ANY($1) AND c.kind = 0 AND c.content_mode = 'nsfw'
           AND NOT EXISTS(SELECT 1 FROM users u WHERE u.id = $2 AND u.is_bot)
           AND NOT EXISTS(SELECT 1 FROM channel_age_accepts a
                          WHERE a.channel_id = c.id AND a.user_id = $2)",
    )
    .bind(&channel_ids[..])
    .bind(user_id)
    .fetch_all(db)
    .await?;
    channel_ids.retain(|id| !gated.contains(id));
    Ok(())
}

/// Like [`channel_access`] but without the age check: for channel settings,
/// read markers, mutes and other things that don't show its messages.
pub async fn channel_access_meta(
    db: &PgPool,
    channel_id: i64,
    user_id: i64,
) -> AppResult<ChannelAccess> {
    let channel = fetch_channel(db, channel_id).await?;
    if channel.guild_id == DM_GUILD {
        if !is_dm_participant(db, channel_id, user_id).await? {
            return Err(AppError::NotFound);
        }
        return Ok(ChannelAccess {
            channel,
            ctx: MemberCtx {
                guild_id: DM_GUILD,
                user_id,
                owner_id: 0,
                role_ids: Vec::new(),
                base: p::DM_PERMS,
                top_position: 0,
                timed_out: false,
            },
            perms: p::DM_PERMS,
        });
    }
    let ctx = member_ctx(db, channel.guild_id, user_id).await?;
    let perms = channel_perms(db, &ctx, channel_id).await?;
    if perms & p::VIEW_CHANNEL == 0 {
        return Err(AppError::NotFound);
    }
    Ok(ChannelAccess {
        channel,
        ctx,
        perms,
    })
}

/// Is this user in this direct message?
pub async fn is_dm_participant(db: &PgPool, channel_id: i64, user_id: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM dm_participants WHERE channel_id = $1 AND user_id = $2)",
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

/// Who should get events about a channel (DMs: guild 0, checked per participant).
pub fn channel_audience(channel: &Channel) -> crate::state::Audience {
    crate::state::Audience::Channel {
        guild_id: channel.guild_id,
        channel_id: channel.id,
    }
}

/// Used by the gateway to decide whether to forward a channel event.
pub async fn can_view(db: &PgPool, guild_id: i64, channel_id: i64, user_id: i64) -> bool {
    if guild_id == DM_GUILD {
        return is_dm_participant(db, channel_id, user_id)
            .await
            .unwrap_or(false);
    }
    match member_ctx(db, guild_id, user_id).await {
        Ok(ctx) => channel_perms(db, &ctx, channel_id)
            .await
            .map(|perms| perms & p::VIEW_CHANNEL != 0)
            .unwrap_or(false),
        Err(_) => false,
    }
}

#[derive(sqlx::FromRow)]
struct GuildOverwriteRow {
    channel_id: i64,
    target_id: i64,
    target_kind: i16,
    allow_bits: i64,
    deny_bits: i64,
}

/// IDs of the text channels in the member's outhouse where they have all of
/// `need` (two queries, however many channels there are).
pub async fn visible_text_channels(db: &PgPool, ctx: &MemberCtx, need: u64) -> AppResult<Vec<i64>> {
    let channel_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM channels WHERE guild_id = $1 AND kind = 0 ORDER BY position, id",
    )
    .bind(ctx.guild_id)
    .fetch_all(db)
    .await?;
    let rows = sqlx::query_as::<_, GuildOverwriteRow>(
        "SELECT o.channel_id, o.target_id, o.target_kind, o.allow_bits, o.deny_bits
         FROM channel_overwrites o
         JOIN channels c ON c.id = o.channel_id
         WHERE c.guild_id = $1",
    )
    .bind(ctx.guild_id)
    .fetch_all(db)
    .await?;
    let mut by_channel: std::collections::HashMap<i64, Vec<Overwrite>> =
        std::collections::HashMap::new();
    for r in rows {
        by_channel.entry(r.channel_id).or_default().push(Overwrite {
            target_id: r.target_id,
            is_member: r.target_kind == 1,
            allow: r.allow_bits as u64,
            deny: r.deny_bits as u64,
        });
    }
    let need = need | p::VIEW_CHANNEL;
    Ok(channel_ids
        .into_iter()
        .filter(|id| {
            let ovs = by_channel.get(id).map(Vec::as_slice).unwrap_or(&[]);
            channel_perms_with(ctx, ovs) & need == need
        })
        .collect())
}
