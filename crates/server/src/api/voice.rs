//! Voice channels.
//!
//! The audio itself goes through LiveKit. Outhouse checks permissions,
//! hands out short-lived LiveKit join tokens and keeps track of who is in
//! which channel so everyone's sidebar stays up to date.

use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::header::HOST;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    channel_kind, VoiceJoinResponse, VoiceModerateRequest, VoiceState, VoiceStateRequest,
};
use outhouse_common::permissions as p;
use serde_json::json;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::livekit;
use crate::perms::{channel_access, channel_perms, fetch_channel, member_ctx, MemberCtx};
use crate::state::{AppState, Audience};

const TOKEN_TTL_SECS: i64 = 6 * 60 * 60;
/// An entrance sound plays at most this often per person (no join/leave spam).
const ENTRANCE_COOLDOWN: Duration = Duration::from_secs(20);
/// After a move, how long the client gets to switch rooms by itself before
/// LiveKit is told to drop it from the old one.
const MOVE_GRACE: Duration = Duration::from_secs(4);

pub fn room_name(channel_id: i64) -> String {
    format!("voice-{channel_id}")
}

/// What a LiveKit join token allows.
pub struct Grants<'a> {
    /// Track sources they may publish ("microphone", "screen_share", ...).
    pub sources: &'a [&'a str],
    pub can_subscribe: bool,
}

/// A LiveKit access token (an HS256 JWT signed with the API secret).
pub fn livekit_token(
    api_key: &str,
    api_secret: &str,
    identity: &str,
    name: &str,
    room: &str,
    grants: &Grants,
    now: i64,
) -> String {
    let claims = json!({
        "iss": api_key,
        "sub": identity,
        "name": name,
        "nbf": now - 10,
        "exp": now + TOKEN_TTL_SECS,
        "video": {
            "room": room,
            "roomJoin": true,
            "canPublish": !grants.sources.is_empty(),
            "canPublishSources": grants.sources,
            "canSubscribe": grants.can_subscribe,
            "canPublishData": true,
        },
    });
    livekit::sign(api_secret, &claims)
}

/// The LiveKit address to give the browser: LIVEKIT_URL if set; behind the
/// HTTPS proxy, the same address as the page (the proxy forwards /rtc to
/// LiveKit); otherwise the page's host on port 7880.
fn livekit_url(st: &AppState, headers: &HeaderMap) -> String {
    if let Some(url) = &st.config.livekit_url {
        return url.clone();
    }
    let host = headers
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost");
    let hostname = if host.starts_with('[') {
        host.split_inclusive(']').next().unwrap_or(host)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let secure = headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|p| p.eq_ignore_ascii_case("https"));
    if secure {
        format!("wss://{host}")
    } else {
        format!("ws://{hostname}:7880")
    }
}

fn announce(st: &AppState, guild_id: i64, user_id: i64, state: Option<&VoiceState>) {
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::VoiceStateUpdate {
            guild_id,
            user_id,
            channel_id: state.map(|s| s.channel_id),
            muted: state.is_some_and(|s| s.muted),
            deafened: state.is_some_and(|s| s.deafened),
            server_muted: state.is_some_and(|s| s.server_muted),
            server_deafened: state.is_some_and(|s| s.server_deafened),
            streaming: state.is_some_and(|s| s.streaming),
        },
    );
}

/// Takes a user out of voice (if they're in it) and tells everyone.
/// With `only_guild`, only if they're in voice in that server.
pub fn clear_voice(st: &AppState, user_id: i64, only_guild: Option<i64>) {
    let removed = {
        let mut map = st.voice.lock().expect("voice lock");
        match map.get(&user_id) {
            Some(v) if only_guild.is_none_or(|g| g == v.guild_id) => map.remove(&user_id),
            _ => None,
        }
    };
    if let Some(old) = removed {
        announce(st, old.guild_id, user_id, None);
    }
}

pub fn voice_state_of(st: &AppState, user_id: i64) -> Option<VoiceState> {
    st.voice.lock().expect("voice lock").get(&user_id).cloned()
}

/// Server mute/deafen saved for someone in an outhouse: (muted, deafened).
async fn saved_moderation(st: &AppState, guild_id: i64, user_id: i64) -> AppResult<(bool, bool)> {
    let row: Option<(bool, bool)> = sqlx::query_as(
        "SELECT muted, deafened FROM voice_moderation WHERE guild_id = $1 AND user_id = $2",
    )
    .bind(guild_id)
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?;
    Ok(row.unwrap_or((false, false)))
}

pub async fn join(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<VoiceJoinResponse>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    if access.channel.kind != channel_kind::VOICE {
        return Err(AppError::BadRequest("That isn't a voice channel.".into()));
    }
    access.require(p::CONNECT)?;
    let (Some(key), Some(secret)) = (&st.config.livekit_api_key, &st.config.livekit_api_secret)
    else {
        return Err(AppError::BadRequest(
            "Voice isn't set up yet: add LIVEKIT_API_KEY and LIVEKIT_API_SECRET to .env.".into(),
        ));
    };
    let guild_id = access.channel.guild_id;
    let (server_muted, server_deafened) = saved_moderation(&st, guild_id, user.id).await?;
    let can_speak = access.perms & p::SPEAK != 0;
    let can_stream = access.perms & p::STREAM != 0;
    let sources =
        livekit::token_sources(can_speak && !server_muted && !server_deafened, can_stream);
    let token = livekit_token(
        key,
        secret,
        &user.id.to_string(),
        &user.display_name,
        &room_name(channel_id),
        &Grants {
            sources: &sources,
            can_subscribe: !server_deafened,
        },
        chrono::Utc::now().timestamp(),
    );

    let previous = voice_state_of(&st, user.id);
    if let Some(prev) = &previous {
        if prev.guild_id != guild_id {
            clear_voice(&st, user.id, None);
        }
    }
    let previous = previous.filter(|p| p.guild_id == guild_id);
    let state = VoiceState {
        guild_id,
        channel_id,
        user_id: user.id,
        muted: previous.as_ref().is_some_and(|p| p.muted) || !can_speak,
        deafened: previous.as_ref().is_some_and(|p| p.deafened),
        server_muted,
        server_deafened,
        streaming: false,
    };
    st.voice
        .lock()
        .expect("voice lock")
        .insert(user.id, state.clone());
    announce(&st, guild_id, user.id, Some(&state));
    if previous.is_none_or(|p| p.channel_id != channel_id) {
        play_entrance(&st, guild_id, channel_id, user.id).await?;
    }

    Ok(Json(VoiceJoinResponse {
        url: livekit_url(&st, &headers),
        token,
        can_speak,
        can_stream,
        server_muted,
        server_deafened,
    }))
}

/// Tells everyone in the channel to play the newcomer's entrance sound.
async fn play_entrance(
    st: &AppState,
    guild_id: i64,
    channel_id: i64,
    user_id: i64,
) -> AppResult<()> {
    let Some(url) = super::sounds::entrance_url(st, user_id).await? else {
        return Ok(());
    };
    {
        let mut cooldowns = st.entrance_cooldowns.lock().expect("cooldown lock");
        let now = Instant::now();
        if cooldowns
            .get(&user_id)
            .is_some_and(|last| now.duration_since(*last) < ENTRANCE_COOLDOWN)
        {
            return Ok(());
        }
        cooldowns.insert(user_id, now);
    }
    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id,
        },
        ServerEvent::EntranceSound {
            guild_id,
            channel_id,
            user_id,
            url,
        },
    );
    Ok(())
}

pub async fn leave(State(st): State<AppState>, AuthUser(user): AuthUser) -> StatusCode {
    clear_voice(&st, user.id, None);
    StatusCode::NO_CONTENT
}

pub async fn update_state(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<VoiceStateRequest>,
) -> AppResult<StatusCode> {
    let updated = {
        let mut map = st.voice.lock().expect("voice lock");
        match map.get_mut(&user.id) {
            Some(v) => {
                v.muted = req.muted || req.deafened;
                v.deafened = req.deafened;
                v.streaming = req.streaming;
                Some(v.clone())
            }
            None => None,
        }
    };
    match updated {
        Some(v) => {
            announce(&st, v.guild_id, user.id, Some(&v));
            Ok(StatusCode::NO_CONTENT)
        }
        None => Err(AppError::BadRequest(
            "You're not in a voice channel.".into(),
        )),
    }
}

/// Everyone currently in voice in a server.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<VoiceState>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let states = st
        .voice
        .lock()
        .expect("voice lock")
        .values()
        .filter(|v| v.guild_id == guild_id)
        .cloned()
        .collect();
    Ok(Json(states))
}

// ---------- moderation: server mute/deafen, move, disconnect ----------

/// The moderator and their target, after checking the target is a member,
/// isn't the moderator or the owner, and ranks below the moderator.
async fn moderation_pair(
    st: &AppState,
    guild_id: i64,
    actor_id: i64,
    target_id: i64,
) -> AppResult<(MemberCtx, MemberCtx)> {
    let actor = member_ctx(&st.db, guild_id, actor_id).await?;
    if target_id == actor_id {
        return Err(AppError::BadRequest(
            "You can't do that to yourself.".into(),
        ));
    }
    let target = member_ctx(&st.db, guild_id, target_id).await?;
    if target.is_owner() || !actor.outranks(target.top_position) {
        return Err(AppError::BadRequest(
            "They're ranked the same as you or higher.".into(),
        ));
    }
    Ok((actor, target))
}

/// Checks the moderator has `perm` in `channel_id` (or server-wide when the
/// target isn't in voice).
async fn require_in(
    st: &AppState,
    actor: &MemberCtx,
    channel_id: Option<i64>,
    perm: u64,
) -> AppResult<()> {
    let perms = match channel_id {
        Some(c) => channel_perms(&st.db, actor, c).await?,
        None => actor.guild_perms(),
    };
    if perms & perm == perm {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Tells LiveKit what someone in voice may do now.
async fn enforce(st: &AppState, target: &MemberCtx, v: &VoiceState) -> AppResult<()> {
    let perms = channel_perms(&st.db, target, v.channel_id).await?;
    livekit::enforce(
        st,
        &room_name(v.channel_id),
        &v.user_id.to_string(),
        perms & p::SPEAK != 0 && !v.server_muted && !v.server_deafened,
        perms & p::STREAM != 0,
        !v.server_deafened,
    )
    .await;
    Ok(())
}

/// `PATCH /api/guilds/{id}/voice/{user}`: server mute/unmute, deafen/undeafen,
/// or move someone to another voice channel.
pub async fn moderate(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target_id)): Path<(i64, i64)>,
    Json(req): Json<VoiceModerateRequest>,
) -> AppResult<StatusCode> {
    let (actor, target) = moderation_pair(&st, guild_id, user.id, target_id).await?;
    let current = voice_state_of(&st, target_id).filter(|v| v.guild_id == guild_id);
    let here = current.as_ref().map(|v| v.channel_id);

    if req.mute.is_some() || req.deafen.is_some() {
        if req.mute.is_some() {
            require_in(&st, &actor, here, p::MUTE_MEMBERS).await?;
        }
        if req.deafen.is_some() {
            require_in(&st, &actor, here, p::DEAFEN_MEMBERS).await?;
        }
        let (was_muted, was_deafened) = saved_moderation(&st, guild_id, target_id).await?;
        let muted = req.mute.unwrap_or(was_muted);
        let deafened = req.deafen.unwrap_or(was_deafened);
        if muted || deafened {
            sqlx::query(
                "INSERT INTO voice_moderation (guild_id, user_id, muted, deafened, updated_by)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (guild_id, user_id) DO UPDATE
                 SET muted = $3, deafened = $4, updated_by = $5, updated_at = now()",
            )
            .bind(guild_id)
            .bind(target_id)
            .bind(muted)
            .bind(deafened)
            .bind(user.id)
            .execute(&st.db)
            .await?;
        } else {
            sqlx::query("DELETE FROM voice_moderation WHERE guild_id = $1 AND user_id = $2")
                .bind(guild_id)
                .bind(target_id)
                .execute(&st.db)
                .await?;
        }
        let updated = {
            let mut map = st.voice.lock().expect("voice lock");
            match map.get_mut(&target_id) {
                Some(v) if v.guild_id == guild_id => {
                    v.server_muted = muted;
                    v.server_deafened = deafened;
                    Some(v.clone())
                }
                _ => None,
            }
        };
        if let Some(v) = &updated {
            announce(&st, guild_id, target_id, Some(v));
            if muted != was_muted || deafened != was_deafened {
                enforce(&st, &target, v).await?;
            }
        }
        if muted != was_muted || deafened != was_deafened {
            let yes_no = |b: bool| if b { "on" } else { "off" };
            let name = crate::audit::user_label(&st, guild_id, target_id).await;
            crate::audit::Entry::new(guild_id, user.id, "voice_moderate")
                .target("member", target_id, name)
                .details(crate::audit::join([
                    crate::audit::change("Server mute", yes_no(was_muted), yes_no(muted)),
                    crate::audit::change("Server deafen", yes_no(was_deafened), yes_no(deafened)),
                ]))
                .save(&st)
                .await;
        }
    }

    if let Some(dest) = req.channel_id {
        let Some(from) = current else {
            return Err(AppError::BadRequest("They're not in voice.".into()));
        };
        let channel = fetch_channel(&st.db, dest).await?;
        if channel.guild_id != guild_id || channel.kind != channel_kind::VOICE {
            return Err(AppError::BadRequest(
                "That isn't a voice channel in this outhouse.".into(),
            ));
        }
        require_in(&st, &actor, Some(from.channel_id), p::MOVE_MEMBERS).await?;
        require_in(&st, &actor, Some(dest), p::MOVE_MEMBERS).await?;
        let target_perms = channel_perms(&st.db, &target, dest).await?;
        if target_perms & (p::VIEW_CHANNEL | p::CONNECT) != p::VIEW_CHANNEL | p::CONNECT {
            return Err(AppError::BadRequest(
                "They aren't allowed in that channel.".into(),
            ));
        }
        if dest != from.channel_id {
            let moved = {
                let mut map = st.voice.lock().expect("voice lock");
                map.get_mut(&target_id).map(|v| {
                    v.channel_id = dest;
                    v.streaming = false;
                    v.clone()
                })
            };
            if let Some(v) = moved {
                announce(&st, guild_id, target_id, Some(&v));
            }
            let from_name: Option<String> =
                sqlx::query_scalar("SELECT name FROM channels WHERE id = $1")
                    .bind(from.channel_id)
                    .fetch_optional(&st.db)
                    .await?;
            let name = crate::audit::user_label(&st, guild_id, target_id).await;
            crate::audit::Entry::new(guild_id, user.id, "voice_move")
                .target("member", target_id, name)
                .details(format!(
                    "🔈 {} → 🔈 {}",
                    from_name.unwrap_or_default(),
                    channel.name
                ))
                .save(&st)
                .await;
            st.dispatch(
                Audience::User(target_id),
                ServerEvent::VoiceMove {
                    guild_id,
                    channel_id: dest,
                },
            );
            // Their app switches by itself; make sure the old room lets go
            // either way (unless they've gone back there meanwhile).
            let st2 = st.clone();
            tokio::spawn(async move {
                tokio::time::sleep(MOVE_GRACE).await;
                if voice_state_of(&st2, target_id).is_some_and(|v| v.channel_id == from.channel_id)
                {
                    return;
                }
                livekit::remove(&st2, &room_name(from.channel_id), &target_id.to_string()).await;
            });
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/guilds/{id}/voice/{user}`: disconnect someone from voice.
pub async fn disconnect(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let (actor, _) = moderation_pair(&st, guild_id, user.id, target_id).await?;
    let Some(v) = voice_state_of(&st, target_id).filter(|v| v.guild_id == guild_id) else {
        return Err(AppError::BadRequest("They're not in voice.".into()));
    };
    require_in(&st, &actor, Some(v.channel_id), p::MOVE_MEMBERS).await?;
    clear_voice(&st, target_id, Some(guild_id));
    st.dispatch(
        Audience::User(target_id),
        ServerEvent::VoiceKicked {
            guild_id,
            channel_id: v.channel_id,
        },
    );
    livekit::remove(&st, &room_name(v.channel_id), &target_id.to_string()).await;
    let from_name: Option<String> = sqlx::query_scalar("SELECT name FROM channels WHERE id = $1")
        .bind(v.channel_id)
        .fetch_optional(&st.db)
        .await?;
    let name = crate::audit::user_label(&st, guild_id, target_id).await;
    crate::audit::Entry::new(guild_id, user.id, "voice_disconnect")
        .target("member", target_id, name)
        .details(format!("From 🔈 {}", from_name.unwrap_or_default()))
        .save(&st)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    #[test]
    fn token_is_a_valid_hs256_jwt() {
        let sources = livekit::token_sources(true, true);
        let token = livekit_token(
            "devkey",
            "secret",
            "42",
            "Dan",
            "voice-7",
            &Grants {
                sources: &sources,
                can_subscribe: true,
            },
            1_800_000_000,
        );
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let claims: serde_json::Value =
            serde_json::from_slice(&engine.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "devkey");
        assert_eq!(claims["sub"], "42");
        assert_eq!(claims["video"]["room"], "voice-7");
        assert_eq!(claims["video"]["roomJoin"], true);
        assert_eq!(claims["video"]["canPublish"], true);
        assert_eq!(
            claims["video"]["canPublishSources"],
            json!(["microphone", "screen_share", "screen_share_audio"])
        );
        assert_eq!(claims["exp"], 1_800_000_000 + TOKEN_TTL_SECS);

        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        mac.verify_slice(&engine.decode(parts[2]).unwrap())
            .expect("signature checks out");
    }

    #[test]
    fn muted_listener_token_publishes_nothing() {
        let token = livekit_token(
            "k",
            "s",
            "1",
            "A",
            "voice-1",
            &Grants {
                sources: &[],
                can_subscribe: false,
            },
            0,
        );
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let claims: serde_json::Value =
            serde_json::from_slice(&engine.decode(token.split('.').nth(1).unwrap()).unwrap())
                .unwrap();
        assert_eq!(claims["video"]["canPublish"], false);
        assert_eq!(claims["video"]["canSubscribe"], false);
    }
}
