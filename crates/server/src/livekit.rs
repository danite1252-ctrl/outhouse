//! Talking to LiveKit: access tokens (HS256 JWTs signed with the API secret)
//! and the RoomService API (Twirp: JSON POSTs to
//! `{LIVEKIT_API_URL}/twirp/livekit.RoomService/{Method}`), used to enforce
//! server mute/deafen and to disconnect people.

use std::time::Duration;

use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::state::AppState;

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Signs JWT claims with the LiveKit API secret (HS256).
pub fn sign(api_secret: &str, claims: &Value) -> String {
    let header = b64(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = b64(claims.to_string().as_bytes());
    let signing_input = format!("{header}.{payload}");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(api_secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(signing_input.as_bytes());
    let signature = b64(&mac.finalize().into_bytes());
    format!("{signing_input}.{signature}")
}

/// A short-lived token that lets Outhouse manage one room.
pub fn admin_token(api_key: &str, api_secret: &str, room: &str, now: i64) -> String {
    sign(
        api_secret,
        &json!({
            "iss": api_key,
            "sub": "outhouse-server",
            "nbf": now - 10,
            "exp": now + 10 * 60,
            "video": { "room": room, "roomAdmin": true },
        }),
    )
}

/// What someone may publish, as LiveKit track sources (token spelling).
pub fn token_sources(can_speak: bool, can_stream: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if can_speak {
        out.push("microphone");
    }
    if can_stream {
        out.extend(["screen_share", "screen_share_audio"]);
    }
    out
}

/// The same, in the RoomService API's spelling (protobuf enum names).
fn api_sources(can_speak: bool, can_stream: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if can_speak {
        out.push("MICROPHONE");
    }
    if can_stream {
        out.extend(["SCREEN_SHARE", "SCREEN_SHARE_AUDIO"]);
    }
    out
}

/// Calls one RoomService method. Errors if LiveKit isn't set up, can't be
/// reached, or says no.
async fn call(st: &AppState, room: &str, method: &str, body: Value) -> anyhow::Result<Value> {
    let (Some(key), Some(secret)) = (&st.config.livekit_api_key, &st.config.livekit_api_secret)
    else {
        anyhow::bail!("LiveKit keys aren't set");
    };
    let token = admin_token(key, secret, room, chrono::Utc::now().timestamp());
    let url = format!(
        "{}/twirp/livekit.RoomService/{method}",
        st.config.livekit_api_url
    );
    let res = st
        .http
        .post(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(5))
        .json(&body)
        .send()
        .await?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "{method} -> {status}: {}",
            text.chars().take(300).collect::<String>()
        );
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// Logs a failed call (LiveKit being down mustn't break moderation: the
/// clients are told over the gateway as well).
fn log(result: anyhow::Result<Value>, what: &str) {
    if let Err(e) = result {
        // "not_found" just means they aren't connected to LiveKit right now.
        if e.to_string().contains("not_found") || e.to_string().contains("404") {
            tracing::debug!(error = %e, "LiveKit {what}: not in the room");
        } else {
            tracing::warn!(error = %e, "LiveKit {what} failed");
        }
    }
}

/// Is a track (from GetParticipant) someone's microphone?
fn is_mic(track: &Value) -> bool {
    matches!(&track["source"], Value::String(s) if s == "MICROPHONE")
        || track["source"].as_i64() == Some(2)
}

/// Applies server mute/deafen to someone connected to a room: mutes their
/// microphone track, then limits what they may publish and hear.
pub async fn enforce(
    st: &AppState,
    room: &str,
    identity: &str,
    can_speak: bool,
    can_stream: bool,
    can_listen: bool,
) {
    if !can_speak {
        let participant = call(
            st,
            room,
            "GetParticipant",
            json!({ "room": room, "identity": identity }),
        )
        .await;
        match participant {
            Ok(p) => {
                let tracks = p["tracks"].as_array().cloned().unwrap_or_default();
                for track in tracks.iter().filter(|t| is_mic(t)) {
                    let Some(sid) = track["sid"].as_str() else {
                        continue;
                    };
                    log(
                        call(
                            st,
                            room,
                            "MutePublishedTrack",
                            json!({ "room": room, "identity": identity, "track_sid": sid, "muted": true }),
                        )
                        .await,
                        "mute track",
                    );
                }
            }
            Err(e) => log(Err(e), "get participant"),
        }
    }
    let permission = json!({
        "can_subscribe": can_listen,
        "can_publish": can_speak || can_stream,
        "can_publish_data": true,
        "can_publish_sources": api_sources(can_speak, can_stream),
    });
    log(
        call(
            st,
            room,
            "UpdateParticipant",
            json!({ "room": room, "identity": identity, "permission": permission }),
        )
        .await,
        "update permissions",
    );
}

/// Disconnects someone from a room.
pub async fn remove(st: &AppState, room: &str, identity: &str) {
    log(
        call(
            st,
            room,
            "RemoveParticipant",
            json!({ "room": room, "identity": identity }),
        )
        .await,
        "remove participant",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_token_grants_room_admin() {
        let token = admin_token("devkey", "secret", "voice-7", 1_800_000_000);
        let parts: Vec<&str> = token.split('.').collect();
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let claims: Value = serde_json::from_slice(&engine.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "devkey");
        assert_eq!(claims["video"]["room"], "voice-7");
        assert_eq!(claims["video"]["roomAdmin"], true);
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        mac.verify_slice(&engine.decode(parts[2]).unwrap()).unwrap();
    }

    #[test]
    fn sources() {
        assert_eq!(token_sources(true, false), vec!["microphone"]);
        assert_eq!(
            api_sources(false, true),
            vec!["SCREEN_SHARE", "SCREEN_SHARE_AUDIO"]
        );
        assert!(is_mic(&json!({"source": "MICROPHONE"})));
        assert!(is_mic(&json!({"source": 2})));
        assert!(!is_mic(&json!({"source": "SCREEN_SHARE"})));
    }
}
