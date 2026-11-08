//! Channels: list, create, delete, permission overwrites, read state.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    channel_kind, content_mode, AckRequest, Channel, CreateChannelRequest, PermissionOverwrite,
    SetOverwriteRequest, UpdateChannelRequest, MAX_TOPIC_CHARS,
};
use outhouse_common::permissions as p;

use crate::audit;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{self, channel_access_meta, member_ctx};
use crate::state::{AppState, Audience};

/// Channels in a server that you can see, with your unread state.
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
                       WHERE a.channel_id = c.id AND a.user_id = $2) AS nsfw_accepted
         FROM channels c
         LEFT JOIN read_states rs ON rs.channel_id = c.id AND rs.user_id = $2
         WHERE c.guild_id = $1
         ORDER BY c.position, c.id",
    )
    .bind(guild_id)
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;

    let mut visible = Vec::with_capacity(channels.len());
    for channel in channels {
        let bits = perms::channel_perms(&st.db, &ctx, channel.id).await?;
        if bits & p::VIEW_CHANNEL != 0 {
            visible.push(channel);
        }
    }
    Ok(Json(visible))
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
        _ => return Err(AppError::BadRequest("Unknown channel type.".into())),
    };
    if name.is_empty() {
        return Err(AppError::BadRequest("Give the channel a name.".into()));
    }

    let channel = sqlx::query_as::<_, Channel>(
        "INSERT INTO channels (id, guild_id, name, kind, position)
         VALUES ($1, $2, $3, $4,
                 (SELECT COALESCE(MAX(position) + 1, 0) FROM channels WHERE guild_id = $2))
         RETURNING id, guild_id, name, kind, position, last_message_id, NULL::BIGINT AS last_read_id,
                   topic, slowmode_secs, content_mode, FALSE AS nsfw_accepted",
    )
    .bind(st.ids.next_id())
    .bind(guild_id)
    .bind(&name)
    .bind(kind)
    .fetch_one(&st.db)
    .await?;

    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id: channel.id,
        },
        ServerEvent::ChannelCreate(channel.clone()),
    );
    audit::Entry::new(guild_id, user.id, "channel_create")
        .target("channel", channel.id, channel_label(&channel))
        .details(if kind == channel_kind::VOICE {
            "Voice channel"
        } else {
            "Text channel"
        })
        .save(&st)
        .await;
    Ok(Json(channel))
}

fn channel_label(c: &Channel) -> String {
    if c.kind == channel_kind::VOICE {
        format!("🔈 {}", c.name)
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

    let name = match req.name.as_deref() {
        None => old.name.clone(),
        Some(raw) => {
            let name: String = if text {
                normalise_text_channel_name(raw)
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
                   FALSE AS nsfw_accepted",
    )
    .bind(channel_id)
    .bind(&name)
    .bind(&topic)
    .bind(slowmode)
    .bind(&mode)
    .fetch_one(&st.db)
    .await?;
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
    audit::Entry::new(channel.guild_id, user.id, "channel_update")
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
    sqlx::query("DELETE FROM channels WHERE id = $1")
        .bind(channel_id)
        .execute(&st.db)
        .await?;
    audit::Entry::new(guild_id, user.id, "channel_delete")
        .target("channel", channel_id, channel_label(&access.channel))
        .save(&st)
        .await;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::ChannelDelete {
            id: channel_id,
            guild_id,
        },
    );
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
