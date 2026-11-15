//! Channels: list, create, delete, categories and their order, permission
//! overwrites, read state.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use std::collections::{HashMap, HashSet};

use outhouse_common::models::{
    channel_kind, content_mode, AckRequest, Channel, ChannelPosition, CollapseRequest,
    CreateChannelRequest, PermissionOverwrite, SetOverwriteRequest, UpdateChannelRequest,
    MAX_CATEGORY_NAME_CHARS, MAX_TOPIC_CHARS,
};
use outhouse_common::permissions::{self as p, Overwrite};

use crate::audit;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{self, channel_access_meta, member_ctx};
use crate::state::{AppState, Audience};

/// Channels in a server that you can see, with your unread state. A
/// category is listed if you can see a channel in it, or you can see the
/// category itself and have Manage Channels there (so empty ones show to
/// the people who can fill them).
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Channel>>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let channels = sqlx::query_as::<_, Channel>(
        "SELECT c.id, c.guild_id, c.name, c.kind, c.position, c.last_message_id,
                rs.last_read_id, c.topic, c.slowmode_secs, c.content_mode,
                EXISTS(SELECT 1 FROM channel_age_accepts a
                       WHERE a.channel_id = c.id AND a.user_id = $2) AS nsfw_accepted,
                c.parent_id,
                EXISTS(SELECT 1 FROM category_collapses cc
                       WHERE cc.category_id = c.id AND cc.user_id = $2) AS collapsed
         FROM channels c
         LEFT JOIN read_states rs ON rs.channel_id = c.id AND rs.user_id = $2
         WHERE c.guild_id = $1
         ORDER BY c.position, c.id",
    )
    .bind(guild_id)
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let overwrites = perms::guild_overwrites(&st.db, guild_id).await?;
    Ok(Json(visible_channels(&ctx, channels, &overwrites)))
}

/// The channels (and categories) someone can see, with "Synced" worked out.
fn visible_channels(
    ctx: &perms::MemberCtx,
    channels: Vec<Channel>,
    overwrites: &HashMap<i64, Vec<Overwrite>>,
) -> Vec<Channel> {
    let none: Vec<Overwrite> = Vec::new();
    let ows = |id: i64| overwrites.get(&id).unwrap_or(&none);
    let bits = |id: i64| perms::channel_perms_with(ctx, ows(id));
    let filled: HashSet<i64> = channels
        .iter()
        .filter(|c| c.kind != channel_kind::CATEGORY && bits(c.id) & p::VIEW_CHANNEL != 0)
        .filter_map(|c| c.parent_id)
        .collect();
    channels
        .into_iter()
        .filter_map(|mut c| {
            let mine = bits(c.id);
            if c.kind == channel_kind::CATEGORY {
                let manage = p::VIEW_CHANNEL | p::MANAGE_CHANNELS;
                if !filled.contains(&c.id) && mine & manage != manage {
                    return None;
                }
            } else if mine & p::VIEW_CHANNEL == 0 {
                return None;
            }
            if let Some(parent) = c.parent_id {
                c.permissions_synced = same_overwrites(ows(c.id), ows(parent));
            }
            Some(c)
        })
        .collect()
}

/// Two channels have the same overwrites (empty ones don't count).
fn same_overwrites(a: &[Overwrite], b: &[Overwrite]) -> bool {
    let norm = |list: &[Overwrite]| {
        let mut v: Vec<(i64, bool, u64, u64)> = list
            .iter()
            .filter(|o| o.allow != 0 || o.deny != 0)
            .map(|o| (o.target_id, o.is_member, o.allow, o.deny))
            .collect();
        v.sort_unstable();
        v
    };
    norm(a) == norm(b)
}

/// `true` when a channel's overwrites match its category's.
async fn synced_with(st: &AppState, channel_id: i64, parent_id: Option<i64>) -> AppResult<bool> {
    let Some(parent) = parent_id else {
        return Ok(false);
    };
    Ok(same_overwrites(
        &perms::overwrites(&st.db, channel_id).await?,
        &perms::overwrites(&st.db, parent).await?,
    ))
}

/// Text channel names look like `rust-help`: lower case, dashes for spaces.
fn normalise_text_channel_name(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(50)
        .collect()
}

/// A category's name: spaces tidied, up to 100 characters (shown in capitals).
fn category_name(raw: &str) -> AppResult<String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > MAX_CATEGORY_NAME_CHARS {
        return Err(AppError::BadRequest(format!(
            "Category names can be up to {MAX_CATEGORY_NAME_CHARS} characters."
        )));
    }
    Ok(name)
}

/// Checks `id` is a category in this outhouse.
async fn require_category(st: &AppState, guild_id: i64, id: i64) -> AppResult<Channel> {
    let cat = perms::fetch_channel(&st.db, id).await.ok();
    match cat {
        Some(c) if c.guild_id == guild_id && c.kind == channel_kind::CATEGORY => Ok(c),
        _ => Err(AppError::BadRequest(
            "That category isn't in this outhouse.".into(),
        )),
    }
}

/// Replaces a channel's overwrites with copies of its category's.
async fn copy_overwrites(
    tx: &mut sqlx::PgConnection,
    from: i64,
    to: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM channel_overwrites WHERE channel_id = $1")
        .bind(to)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO channel_overwrites (channel_id, target_id, target_kind, allow_bits, deny_bits)
         SELECT $2, target_id, target_kind, allow_bits, deny_bits
         FROM channel_overwrites WHERE channel_id = $1",
    )
    .bind(from)
    .bind(to)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<CreateChannelRequest>,
) -> AppResult<Json<Channel>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_CHANNELS)?;
    let kind = req.kind.unwrap_or(channel_kind::TEXT);
    let name = match kind {
        channel_kind::TEXT => normalise_text_channel_name(&req.name),
        channel_kind::VOICE => req.name.trim().chars().take(50).collect(),
        channel_kind::CATEGORY => category_name(&req.name)?,
        _ => return Err(AppError::BadRequest("Unknown channel type.".into())),
    };
    if name.is_empty() {
        return Err(AppError::BadRequest(
            if kind == channel_kind::CATEGORY {
                "Give the category a name."
            } else {
                "Give the channel a name."
            }
            .into(),
        ));
    }
    let category = match req.parent_id {
        None => None,
        Some(_) if kind == channel_kind::CATEGORY => {
            return Err(AppError::BadRequest(
                "Categories can't go inside other categories.".into(),
            ))
        }
        Some(id) => Some(require_category(&st, guild_id, id).await?),
    };
    let parent_id = category.as_ref().map(|c| c.id);
    let id = st.ids.next_id();

    let mut tx = st.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(guild_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO channels (id, guild_id, name, kind, position, parent_id)
         VALUES ($1, $2, $3, $4,
                 (SELECT COALESCE(MAX(position) + 1, 0) FROM channels WHERE guild_id = $2), $5)",
    )
    .bind(id)
    .bind(guild_id)
    .bind(&name)
    .bind(kind)
    .bind(parent_id)
    .execute(&mut *tx)
    .await?;
    if let (Some(parent), true) = (parent_id, req.sync_permissions) {
        copy_overwrites(&mut tx, parent, id).await?;
    }
    renumber(&mut tx, guild_id, &HashMap::new()).await?;
    tx.commit().await?;

    let mut channel = perms::fetch_channel(&st.db, id).await?;
    channel.permissions_synced = synced_with(&st, id, parent_id).await?;

    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id: channel.id,
        },
        ServerEvent::ChannelCreate(channel.clone()),
    );
    if kind == channel_kind::CATEGORY {
        audit::Entry::new(guild_id, user.id, "category_create")
            .target("channel", channel.id, channel_label(&channel))
            .save(&st)
            .await;
    } else {
        let what = if kind == channel_kind::VOICE {
            "Voice channel"
        } else {
            "Text channel"
        };
        audit::Entry::new(guild_id, user.id, "channel_create")
            .target("channel", channel.id, channel_label(&channel))
            .details(match &category {
                Some(c) => format!("{what} in {}", c.name.to_uppercase()),
                None => what.to_string(),
            })
            .save(&st)
            .await;
    }
    Ok(Json(channel))
}

#[derive(sqlx::FromRow, Clone)]
struct PlaceRow {
    id: i64,
    kind: i16,
    position: i32,
    parent_id: Option<i64>,
}

/// One channel's place before and after [`renumber`].
pub struct Moved {
    pub id: i64,
    pub old_parent: Option<i64>,
    pub new_parent: Option<i64>,
    pub old_index: usize,
    pub new_index: usize,
}

/// Puts an outhouse's channels in order and numbers them 0, 1, 2, … in
/// list order: uncategorised text channels, uncategorised voice channels,
/// then each category followed by its text and then its voice channels (the
/// Discord rule: text above voice inside a category). `patch` moves some
/// first: `(position, parent)` where the position is a sort key among the
/// others (ties: the moved one goes first) and the parent `Some(None)`
/// means uncategorised. Call inside a transaction.
async fn renumber(
    tx: &mut sqlx::PgConnection,
    guild_id: i64,
    patch: &HashMap<i64, (Option<i32>, Option<Option<i64>>)>,
) -> AppResult<Vec<Moved>> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(guild_id)
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query_as::<_, PlaceRow>(
        "SELECT id, kind, position, parent_id FROM channels
         WHERE guild_id = $1 ORDER BY position, id",
    )
    .bind(guild_id)
    .fetch_all(&mut *tx)
    .await?;
    let old_index: HashMap<i64, usize> = rows.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
    let old_parent: HashMap<i64, Option<i64>> = rows.iter().map(|r| (r.id, r.parent_id)).collect();
    let categories: HashSet<i64> = rows
        .iter()
        .filter(|r| r.kind == channel_kind::CATEGORY)
        .map(|r| r.id)
        .collect();

    // (sort key, moved-first, id)
    type Key = (i32, u8, i64);
    let key = |r: &PlaceRow| -> Key {
        match patch.get(&r.id) {
            Some((Some(pos), _)) => (*pos, 0, r.id),
            Some((None, _)) => (r.position, 0, r.id),
            None => (r.position, 1, r.id),
        }
    };
    let mut cats: Vec<Key> = Vec::new();
    let mut groups: HashMap<(Option<i64>, u8), Vec<Key>> = HashMap::new();
    let mut new_parent: HashMap<i64, Option<i64>> = HashMap::new();
    for r in &rows {
        if r.kind == channel_kind::CATEGORY {
            cats.push(key(r));
            continue;
        }
        let parent = match patch.get(&r.id) {
            Some((_, Some(p))) => *p,
            _ => r.parent_id,
        }
        .filter(|p| categories.contains(p));
        new_parent.insert(r.id, parent);
        let group = if r.kind == channel_kind::VOICE { 1 } else { 0 };
        groups.entry((parent, group)).or_default().push(key(r));
    }
    cats.sort_unstable();
    for list in groups.values_mut() {
        list.sort_unstable();
    }
    let mut order: Vec<i64> = Vec::with_capacity(rows.len());
    let mut take = |parent: Option<i64>, order: &mut Vec<i64>| {
        for group in [0u8, 1] {
            if let Some(list) = groups.remove(&(parent, group)) {
                order.extend(list.into_iter().map(|k| k.2));
            }
        }
    };
    take(None, &mut order);
    for c in &cats {
        order.push(c.2);
        take(Some(c.2), &mut order);
    }

    let mut ids = Vec::new();
    let mut positions = Vec::new();
    let mut parents = Vec::new();
    let mut moved = Vec::new();
    for (i, id) in order.iter().enumerate() {
        let row = &rows[old_index[id]];
        let parent = new_parent.get(id).copied().flatten();
        if row.position != i as i32 || row.parent_id != parent {
            ids.push(*id);
            positions.push(i as i32);
            parents.push(parent.unwrap_or(0));
        }
        moved.push(Moved {
            id: *id,
            old_parent: old_parent[id],
            new_parent: parent,
            old_index: old_index[id],
            new_index: i,
        });
    }
    if !ids.is_empty() {
        sqlx::query(
            "UPDATE channels c SET position = u.pos, parent_id = NULLIF(u.parent, 0)
             FROM UNNEST($1::BIGINT[], $2::INT[], $3::BIGINT[]) AS u(id, pos, parent)
             WHERE c.id = u.id",
        )
        .bind(&ids)
        .bind(&positions)
        .bind(&parents)
        .execute(&mut *tx)
        .await?;
    }
    Ok(moved)
}

/// Most channels and categories one reorder can list.
const MAX_REORDER: usize = 500;

/// `PATCH /api/guilds/{id}/channels/positions` (Manage Channels): drag and
/// drop in the channel list. `[{id, position?, parent_id?, sync_permissions?}]`;
/// positions are sort keys (the web client sends every channel it can see),
/// the server renumbers everything in list order. Categories can't go in
/// categories. One audit log entry; everyone re-fetches their list.
pub async fn reorder(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<Vec<ChannelPosition>>,
) -> AppResult<Json<Vec<Channel>>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    ctx.require(p::MANAGE_CHANNELS)?;
    if req.is_empty() {
        return Err(AppError::BadRequest("Nothing to move.".into()));
    }
    if req.len() > MAX_REORDER {
        return Err(AppError::BadRequest(format!(
            "Move up to {MAX_REORDER} channels at once."
        )));
    }
    let rows = sqlx::query_as::<_, PlaceRow>(
        "SELECT id, kind, position, parent_id FROM channels WHERE guild_id = $1",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    let by_id: HashMap<i64, PlaceRow> = rows.iter().map(|r| (r.id, r.clone())).collect();
    let overwrites = perms::guild_overwrites(&st.db, guild_id).await?;
    let none: Vec<Overwrite> = Vec::new();
    let bits = |id: i64| perms::channel_perms_with(&ctx, overwrites.get(&id).unwrap_or(&none));

    let mut patch: HashMap<i64, (Option<i32>, Option<Option<i64>>)> = HashMap::new();
    let mut syncs: Vec<i64> = Vec::new();
    for item in &req {
        let Some(row) = by_id.get(&item.id) else {
            return Err(AppError::BadRequest(
                "Those channels aren't all in this outhouse.".into(),
            ));
        };
        let parent: Option<Option<i64>> = match &item.parent_id {
            None => None,
            Some(None) => Some(None),
            Some(Some(raw)) => {
                let id: i64 = raw
                    .parse()
                    .map_err(|_| AppError::BadRequest("Bad category ID.".into()))?;
                Some(Some(id))
            }
        };
        if let Some(Some(pid)) = parent {
            if row.kind == channel_kind::CATEGORY {
                return Err(AppError::BadRequest(
                    "Categories can't go inside other categories.".into(),
                ));
            }
            match by_id.get(&pid) {
                Some(c) if c.kind == channel_kind::CATEGORY => {}
                _ => {
                    return Err(AppError::BadRequest(
                        "That category isn't in this outhouse.".into(),
                    ))
                }
            }
            if bits(pid) & p::VIEW_CHANNEL == 0 && pid != row.parent_id.unwrap_or(0) {
                return Err(AppError::Forbidden);
            }
        }
        if patch.insert(item.id, (item.position, parent)).is_some() {
            return Err(AppError::BadRequest(
                "Each channel can only be listed once.".into(),
            ));
        }
        if item.sync_permissions {
            syncs.push(item.id);
        }
    }

    let mut tx = st.db.begin().await?;
    let moved = renumber(&mut tx, guild_id, &patch).await?;
    // Moving a channel needs Manage Channels in it (an owner or admin always
    // has it); one that only shifted because others moved doesn't.
    let mut parent_changes = Vec::new();
    let mut reordered = 0usize;
    for m in &moved {
        let listed = patch.contains_key(&m.id);
        let parent_changed = m.old_parent != m.new_parent;
        let place_changed = parent_changed || m.old_index != m.new_index;
        if listed && place_changed && bits(m.id) & p::MANAGE_CHANNELS == 0 {
            return Err(AppError::Forbidden);
        }
        if parent_changed {
            parent_changes.push(m);
        } else if listed && m.old_index != m.new_index {
            reordered += 1;
        }
    }
    for id in &syncs {
        let parent = moved
            .iter()
            .find(|m| m.id == *id)
            .and_then(|m| m.new_parent);
        let Some(parent) = parent else {
            return Err(AppError::BadRequest(
                "Only channels in a category can sync with it.".into(),
            ));
        };
        ctx.require(p::MANAGE_ROLES)?;
        let allow_deny = overwrites
            .get(&parent)
            .map(|v| v.iter().fold(0u64, |acc, o| acc | o.allow | o.deny))
            .unwrap_or(0);
        ctx.require_can_grant(allow_deny)?;
        copy_overwrites(&mut tx, parent, *id).await?;
    }
    tx.commit().await?;

    // One audit entry for the whole drag.
    let names: HashMap<i64, (String, i16)> = sqlx::query_as::<_, (i64, String, i16)>(
        "SELECT id, name, kind FROM channels WHERE guild_id = $1",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .map(|(id, name, kind)| (id, (name, kind)))
    .collect();
    let name_of = |id: Option<i64>| match id.and_then(|id| names.get(&id)) {
        None => "No category".to_string(),
        Some((name, _)) => name.to_uppercase(),
    };
    let mut details: Vec<Option<String>> = Vec::new();
    for m in parent_changes.iter().take(20) {
        let Some((name, kind)) = names.get(&m.id) else {
            continue;
        };
        let label = if *kind == channel_kind::VOICE {
            format!("🔈 {name}")
        } else {
            format!("#{name}")
        };
        details.push(Some(format!(
            "{label}: {} → {}",
            name_of(m.old_parent),
            name_of(m.new_parent)
        )));
    }
    if reordered > 0 {
        details.push(Some(format!(
            "Changed the order of {reordered} channel{}",
            if reordered == 1 { "" } else { "s" }
        )));
    }
    if !syncs.is_empty() {
        details.push(Some(format!(
            "Synced permissions for {} channel{}",
            syncs.len(),
            if syncs.len() == 1 { "" } else { "s" }
        )));
    }
    if !details.is_empty() {
        audit::Entry::new(guild_id, user.id, "channels_move")
            .details(audit::join(details))
            .save(&st)
            .await;
        if syncs.is_empty() {
            st.dispatch(
                Audience::Guild(guild_id),
                ServerEvent::ChannelsReordered { guild_id },
            );
        } else {
            st.dispatch(
                Audience::Guild(guild_id),
                ServerEvent::GuildRefresh { guild_id },
            );
        }
    }

    // Your list as it is now.
    let channels = sqlx::query_as::<_, Channel>(
        "SELECT c.id, c.guild_id, c.name, c.kind, c.position, c.last_message_id,
                rs.last_read_id, c.topic, c.slowmode_secs, c.content_mode,
                EXISTS(SELECT 1 FROM channel_age_accepts a
                       WHERE a.channel_id = c.id AND a.user_id = $2) AS nsfw_accepted,
                c.parent_id,
                EXISTS(SELECT 1 FROM category_collapses cc
                       WHERE cc.category_id = c.id AND cc.user_id = $2) AS collapsed
         FROM channels c
         LEFT JOIN read_states rs ON rs.channel_id = c.id AND rs.user_id = $2
         WHERE c.guild_id = $1
         ORDER BY c.position, c.id",
    )
    .bind(guild_id)
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let overwrites = perms::guild_overwrites(&st.db, guild_id).await?;
    Ok(Json(visible_channels(&ctx, channels, &overwrites)))
}

/// `POST /api/channels/{id}/sync-permissions` (Manage Roles): copy the
/// category's permission overwrites onto this channel. Nothing is ever
/// inherited automatically; this is a one-off copy (like Discord's "Sync
/// now").
pub async fn sync_permissions(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<Channel>> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    access.ctx.require(p::MANAGE_ROLES)?;
    let guild_id = access.channel.guild_id;
    let Some(parent) = access.channel.parent_id else {
        return Err(AppError::BadRequest(
            "This channel isn't in a category.".into(),
        ));
    };
    let category = require_category(&st, guild_id, parent).await?;
    let from = perms::overwrites(&st.db, parent).await?;
    access
        .ctx
        .require_can_grant(from.iter().fold(0u64, |acc, o| acc | o.allow | o.deny))?;
    let mut tx = st.db.begin().await?;
    copy_overwrites(&mut tx, parent, channel_id).await?;
    tx.commit().await?;
    audit::Entry::new(guild_id, user.id, "channel_permissions_sync")
        .target("channel", channel_id, channel_label(&access.channel))
        .details(format!("Copied from {}", category.name.to_uppercase()))
        .save(&st)
        .await;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    let mut channel = access.channel;
    channel.permissions_synced = true;
    Ok(Json(channel))
}

/// `PUT /api/channels/{id}/collapse` `{collapsed}`: collapse or expand a
/// category for yourself (remembered on the server for all your devices).
pub async fn collapse(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CollapseRequest>,
) -> AppResult<StatusCode> {
    let channel = perms::fetch_channel(&st.db, channel_id).await?;
    if channel.kind != channel_kind::CATEGORY
        || channel.guild_id == outhouse_common::models::DM_GUILD
    {
        return Err(AppError::NotFound);
    }
    member_ctx(&st.db, channel.guild_id, user.id).await?;
    if req.collapsed {
        sqlx::query(
            "INSERT INTO category_collapses (user_id, category_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(user.id)
        .bind(channel_id)
        .execute(&st.db)
        .await?;
    } else {
        sqlx::query("DELETE FROM category_collapses WHERE user_id = $1 AND category_id = $2")
            .bind(user.id)
            .bind(channel_id)
            .execute(&st.db)
            .await?;
    }
    st.dispatch(
        Audience::User(user.id),
        ServerEvent::CategoryCollapse {
            guild_id: channel.guild_id,
            channel_id,
            collapsed: req.collapsed,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

fn channel_label(c: &Channel) -> String {
    if c.kind == channel_kind::VOICE {
        format!("🔈 {}", c.name)
    } else if c.kind == channel_kind::CATEGORY {
        format!("📁 {}", c.name.to_uppercase())
    } else {
        format!("#{}", c.name)
    }
}

/// "Off", "10s", "5m", "2h" for slowmode settings.
fn slowmode_label(secs: i32) -> String {
    match secs {
        0 => "Off".into(),
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
    }
}

fn content_label(mode: &str) -> &'static str {
    match mode {
        content_mode::SPOILER => "Spoiler channel",
        content_mode::NSFW => "Age-restricted",
        _ => "Default",
    }
}

/// A topic shortened for the audit log.
fn topic_snippet(topic: &str) -> String {
    if topic.is_empty() {
        return "(none)".into();
    }
    let short: String = topic.chars().take(80).collect();
    if topic.chars().count() > 80 {
        format!("“{short}…”")
    } else {
        format!("“{short}”")
    }
}

/// `PATCH /api/channels/{id}`: name, topic, slowmode and content
/// visibility (Manage Channels).
pub async fn update(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<UpdateChannelRequest>,
) -> AppResult<Json<Channel>> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    let old = access.channel;
    if old.guild_id == outhouse_common::models::DM_GUILD {
        return Err(AppError::NotFound);
    }
    if access.perms & p::MANAGE_CHANNELS == 0 {
        return Err(AppError::Forbidden);
    }
    let text = old.kind == channel_kind::TEXT;
    let category = old.kind == channel_kind::CATEGORY;

    let name = match req.name.as_deref() {
        None => old.name.clone(),
        Some(raw) => {
            let name: String = if text {
                normalise_text_channel_name(raw)
            } else if category {
                category_name(raw)?
            } else {
                raw.trim().chars().take(50).collect()
            };
            if name.is_empty() {
                return Err(AppError::BadRequest("Give the channel a name.".into()));
            }
            name
        }
    };
    let topic = match req.topic.as_deref() {
        None => old.topic.clone(),
        Some(raw) => {
            let topic = raw.trim().to_string();
            if topic.chars().count() > MAX_TOPIC_CHARS {
                return Err(AppError::BadRequest(format!(
                    "Topics can be up to {MAX_TOPIC_CHARS} characters."
                )));
            }
            if !text && !topic.is_empty() {
                return Err(AppError::BadRequest(
                    "Only text channels have a topic.".into(),
                ));
            }
            topic
        }
    };
    let slowmode = match req.slowmode_secs {
        None => old.slowmode_secs,
        Some(secs) => {
            if !(0..=21600).contains(&secs) {
                return Err(AppError::BadRequest(
                    "Slowmode is between off and 6 hours.".into(),
                ));
            }
            if !text && secs != 0 {
                return Err(AppError::BadRequest(
                    "Only text channels have slowmode.".into(),
                ));
            }
            secs
        }
    };
    let mode = match req.content_mode.as_deref() {
        None => old.content_mode.clone(),
        Some(m) => {
            if !content_mode::ALL.contains(&m) {
                return Err(AppError::BadRequest(
                    "Content visibility is default, spoiler or nsfw.".into(),
                ));
            }
            if !text && m != content_mode::DEFAULT {
                return Err(AppError::BadRequest(
                    "Only text channels can be spoiler or age-restricted.".into(),
                ));
            }
            m.to_string()
        }
    };

    let mut changes: Vec<Option<String>> = Vec::new();
    if name != old.name {
        changes.push(Some(format!("Name: {} → {}", old.name, name)));
    }
    if topic != old.topic {
        changes.push(Some(format!(
            "Topic: {} → {}",
            topic_snippet(&old.topic),
            topic_snippet(&topic)
        )));
    }
    if slowmode != old.slowmode_secs {
        changes.push(Some(format!(
            "Slowmode: {} → {}",
            slowmode_label(old.slowmode_secs),
            slowmode_label(slowmode)
        )));
    }
    if mode != old.content_mode {
        changes.push(Some(format!(
            "Content: {} → {}",
            content_label(&old.content_mode),
            content_label(&mode)
        )));
    }

    let mut channel = sqlx::query_as::<_, Channel>(
        "UPDATE channels SET name = $2, topic = $3, slowmode_secs = $4, content_mode = $5
         WHERE id = $1
         RETURNING id, guild_id, name, kind, position, last_message_id,
                   NULL::BIGINT AS last_read_id, topic, slowmode_secs, content_mode,
                   FALSE AS nsfw_accepted, parent_id",
    )
    .bind(channel_id)
    .bind(&name)
    .bind(&topic)
    .bind(slowmode)
    .bind(&mode)
    .fetch_one(&st.db)
    .await?;
    channel.permissions_synced = synced_with(&st, channel_id, channel.parent_id).await?;
    if changes.is_empty() {
        channel.nsfw_accepted = perms::content_allowed(&st.db, channel_id, user.id).await;
        return Ok(Json(channel));
    }

    st.dispatch(
        Audience::Channel {
            guild_id: channel.guild_id,
            channel_id,
        },
        ServerEvent::ChannelUpdate(channel.clone()),
    );
    if category {
        // People who see a channel in it but not the category itself.
        st.dispatch(
            Audience::Guild(channel.guild_id),
            ServerEvent::ChannelsReordered {
                guild_id: channel.guild_id,
            },
        );
    }
    audit::Entry::new(
        channel.guild_id,
        user.id,
        if category {
            "category_update"
        } else {
            "channel_update"
        },
    )
    .target("channel", channel_id, channel_label(&channel))
    .details(audit::join(changes))
    .save(&st)
    .await;
    channel.nsfw_accepted = perms::content_allowed(&st.db, channel_id, user.id).await;
    Ok(Json(channel))
}

/// `PUT /api/channels/{id}/age-gate`: "I'm 18+" for an age-restricted
/// channel (remembered on the server, so it follows you to every device).
pub async fn accept_age_gate(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<StatusCode> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    if access.channel.guild_id == outhouse_common::models::DM_GUILD {
        return Err(AppError::NotFound);
    }
    // Teens can't open 18+ channels, whatever they click.
    if crate::api::safety::teen_kept_from_adult_channels(&st.db, user.id).await? {
        return Err(AppError::Refused(
            "This channel is for adults (18+) only.".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO channel_age_accepts (channel_id, user_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(channel_id)
    .bind(user.id)
    .execute(&st.db)
    .await?;
    st.dispatch(
        Audience::User(user.id),
        ServerEvent::ChannelAgeGate {
            guild_id: access.channel.guild_id,
            channel_id,
            accepted: true,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/channels/{id}/age-gate`: ask me again next time.
pub async fn reset_age_gate(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<StatusCode> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    sqlx::query("DELETE FROM channel_age_accepts WHERE channel_id = $1 AND user_id = $2")
        .bind(channel_id)
        .bind(user.id)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::User(user.id),
        ServerEvent::ChannelAgeGate {
            guild_id: access.channel.guild_id,
            channel_id,
            accepted: false,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// "@everyone", "@Raider" or a member's name, for the audit log.
async fn overwrite_target_name(st: &AppState, guild_id: i64, target_id: i64) -> String {
    if target_id == guild_id {
        return "@everyone".into();
    }
    let name: Option<String> = sqlx::query_scalar(
        "SELECT '@' || name FROM roles WHERE id = $1 AND guild_id = $2
         UNION ALL
         SELECT COALESCE(m.nickname, u.display_name) FROM guild_members m
         JOIN users u ON u.id = m.user_id WHERE m.guild_id = $2 AND m.user_id = $1
         LIMIT 1",
    )
    .bind(target_id)
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten();
    name.unwrap_or_else(|| target_id.to_string())
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<StatusCode> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    access.require(p::MANAGE_CHANNELS)?;
    let guild_id = access.channel.guild_id;
    let category = access.channel.kind == channel_kind::CATEGORY;
    let mut tx = st.db.begin().await?;
    // A category's channels are never deleted with it: they become
    // uncategorised (parent_id is ON DELETE SET NULL), keeping their order.
    let children: i64 = sqlx::query_scalar("SELECT count(*) FROM channels WHERE parent_id = $1")
        .bind(channel_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM channels WHERE id = $1")
        .bind(channel_id)
        .execute(&mut *tx)
        .await?;
    if category {
        renumber(&mut tx, guild_id, &HashMap::new()).await?;
    }
    tx.commit().await?;
    audit::Entry::new(
        guild_id,
        user.id,
        if category {
            "category_delete"
        } else {
            "channel_delete"
        },
    )
    .target("channel", channel_id, channel_label(&access.channel))
    .details(if category && children > 0 {
        format!(
            "{children} channel{} moved out of it (not deleted)",
            if children == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    })
    .save(&st)
    .await;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::ChannelDelete {
            id: channel_id,
            guild_id,
        },
    );
    if category {
        st.dispatch(
            Audience::Guild(guild_id),
            ServerEvent::ChannelsReordered { guild_id },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Marks a channel as read up to a message.
pub async fn ack(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<AckRequest>,
) -> AppResult<StatusCode> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    let last_read_id: i64 = sqlx::query_scalar(
        "INSERT INTO read_states (user_id, channel_id, last_read_id) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, channel_id)
         DO UPDATE SET last_read_id = GREATEST(read_states.last_read_id, EXCLUDED.last_read_id)
         RETURNING last_read_id",
    )
    .bind(user.id)
    .bind(channel_id)
    .bind(req.message_id)
    .fetch_one(&st.db)
    .await?;
    // Your other devices clear their unread markers and mention counts too.
    st.dispatch(
        Audience::User(user.id),
        ServerEvent::ChannelAck {
            guild_id: access.channel.guild_id,
            channel_id,
            last_read_id,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

// ---- Permission overwrites ---------------------------------------------

#[derive(sqlx::FromRow)]
struct OverwriteRow {
    target_id: i64,
    target_kind: i16,
    allow_bits: i64,
    deny_bits: i64,
}

pub async fn list_overwrites(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<Vec<PermissionOverwrite>>> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    access.ctx.require(p::MANAGE_ROLES)?;
    let rows = sqlx::query_as::<_, OverwriteRow>(
        "SELECT target_id, target_kind, allow_bits, deny_bits
         FROM channel_overwrites WHERE channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| PermissionOverwrite {
                target_id: r.target_id,
                kind: if r.target_kind == 1 { "member" } else { "role" }.into(),
                allow: r.allow_bits,
                deny: r.deny_bits,
            })
            .collect(),
    ))
}

pub async fn set_overwrite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, target_id)): Path<(i64, i64)>,
    Json(req): Json<SetOverwriteRequest>,
) -> AppResult<Json<PermissionOverwrite>> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    access.ctx.require(p::MANAGE_ROLES)?;
    let guild_id = access.channel.guild_id;
    let target_kind: i16 = match req.kind.as_str() {
        "role" => {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM roles WHERE id = $1 AND guild_id = $2)",
            )
            .bind(target_id)
            .bind(guild_id)
            .fetch_one(&st.db)
            .await?;
            if !exists {
                return Err(AppError::NotFound);
            }
            0
        }
        "member" => {
            member_ctx(&st.db, guild_id, target_id).await?;
            1
        }
        _ => return Err(AppError::BadRequest("kind must be role or member.".into())),
    };
    let allow = (req.allow as u64) & p::ALL;
    let deny = (req.deny as u64) & p::ALL & !allow;
    access.ctx.require_can_grant(allow | deny)?;
    let before: Option<(i64, i64)> = sqlx::query_as(
        "SELECT allow_bits, deny_bits FROM channel_overwrites
         WHERE channel_id = $1 AND target_id = $2",
    )
    .bind(channel_id)
    .bind(target_id)
    .fetch_optional(&st.db)
    .await?;
    let (old_allow, old_deny) = before.map(|(a, d)| (a as u64, d as u64)).unwrap_or((0, 0));

    sqlx::query(
        "INSERT INTO channel_overwrites (channel_id, target_id, target_kind, allow_bits, deny_bits)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (channel_id, target_id)
         DO UPDATE SET target_kind = EXCLUDED.target_kind,
                       allow_bits = EXCLUDED.allow_bits,
                       deny_bits = EXCLUDED.deny_bits",
    )
    .bind(channel_id)
    .bind(target_id)
    .bind(target_kind)
    .bind(allow as i64)
    .bind(deny as i64)
    .execute(&st.db)
    .await?;

    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    let details = audit::join([
        Some(format!(
            "For {}",
            overwrite_target_name(&st, guild_id, target_id).await
        )),
        audit::perm_diff("Allowed", old_allow, allow),
        audit::perm_diff("Denied", old_deny, deny),
    ]);
    audit::Entry::new(guild_id, user.id, "channel_permissions")
        .target("channel", channel_id, channel_label(&access.channel))
        .details(details)
        .save(&st)
        .await;
    Ok(Json(PermissionOverwrite {
        target_id,
        kind: req.kind,
        allow: allow as i64,
        deny: deny as i64,
    }))
}

pub async fn delete_overwrite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, target_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    access.ctx.require(p::MANAGE_ROLES)?;
    let guild_id = access.channel.guild_id;
    let removed =
        sqlx::query("DELETE FROM channel_overwrites WHERE channel_id = $1 AND target_id = $2")
            .bind(channel_id)
            .bind(target_id)
            .execute(&st.db)
            .await?
            .rows_affected();
    if removed > 0 {
        audit::Entry::new(guild_id, user.id, "channel_permissions_reset")
            .target("channel", channel_id, channel_label(&access.channel))
            .details(format!(
                "For {}",
                overwrite_target_name(&st, guild_id, target_id).await
            ))
            .save(&st)
            .await;
    }
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    Ok(StatusCode::NO_CONTENT)
}
