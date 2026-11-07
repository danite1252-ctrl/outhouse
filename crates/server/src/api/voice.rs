//! Voice channels.
//!
//! The audio itself goes through LiveKit. Outhouse checks permissions,
//! hands out short-lived LiveKit join tokens and keeps track of who is in
//! which channel so everyone's sidebar stays up to date.

use axum::extract::{Path, State};
use axum::http::header::HOST;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use base64::Engine;
use hmac::{Hmac, Mac};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{channel_kind, VoiceJoinResponse, VoiceState, VoiceStateRequest};
use outhouse_common::permissions as p;
use serde_json::json;
use sha2::Sha256;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, member_ctx};
use crate::state::{AppState, Audience};

const TOKEN_TTL_SECS: i64 = 6 * 60 * 60;

pub fn room_name(channel_id: i64) -> String {
    format!("voice-{channel_id}")
}

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// A LiveKit access token (an HS256 JWT signed with the API secret).
pub fn livekit_token(
    api_key: &str,
    api_secret: &str,
    identity: &str,
    name: &str,
    room: &str,
    can_publish: bool,
    now: i64,
) -> String {
    let header = b64(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = json!({
        "iss": api_key,
        "sub": identity,
        "name": name,
        "nbf": now - 10,
        "exp": now + TOKEN_TTL_SECS,
        "video": {
            "room": room,
            "roomJoin": true,
            "canPublish": can_publish,
            "canSubscribe": true,
            "canPublishData": true,
        },
    });
    let payload = b64(claims.to_string().as_bytes());
    let signing_input = format!("{header}.{payload}");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(api_secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(signing_input.as_bytes());
    let signature = b64(&mac.finalize().into_bytes());
    format!("{signing_input}.{signature}")
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
    let can_speak = access.perms & p::SPEAK != 0;
    let token = livekit_token(
        key,
        secret,
        &user.id.to_string(),
        &user.display_name,
        &room_name(channel_id),
        can_speak,
        chrono::Utc::now().timestamp(),
    );

    let guild_id = access.channel.guild_id;
    let previous = voice_state_of(&st, user.id);
    if let Some(prev) = &previous {
        if prev.guild_id != guild_id {
            clear_voice(&st, user.id, None);
        }
    }
    let state = VoiceState {
        guild_id,
        channel_id,
        user_id: user.id,
        muted: previous.as_ref().is_some_and(|p| p.muted) || !can_speak,
        deafened: previous.as_ref().is_some_and(|p| p.deafened),
    };
    st.voice
        .lock()
        .expect("voice lock")
        .insert(user.id, state.clone());
    announce(&st, guild_id, user.id, Some(&state));

    Ok(Json(VoiceJoinResponse {
        url: livekit_url(&st, &headers),
        token,
        can_speak,
    }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_a_valid_hs256_jwt() {
        let token = livekit_token(
            "devkey",
            "secret",
            "42",
            "Dan",
            "voice-7",
            true,
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
        assert_eq!(claims["exp"], 1_800_000_000 + TOKEN_TTL_SECS);

        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        mac.verify_slice(&engine.decode(parts[2]).unwrap())
            .expect("signature checks out");
    }
}
