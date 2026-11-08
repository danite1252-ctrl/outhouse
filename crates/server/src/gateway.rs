//! The real-time WebSocket gateway at `/api/gateway`.
//!
//! 1. Client connects and sends IDENTIFY with its login token (within 10 s).
//! 2. Server replies READY with the user and their servers.
//! 3. Server pushes events for those servers; client sends HEARTBEAT every
//!    ~30 s. No heartbeat for 90 s and the connection is closed.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use outhouse_common::gateway::{ClientEvent, ServerEvent};
use outhouse_common::models::DM_GUILD;
use tokio::sync::broadcast::error::RecvError;

use crate::api::guilds::guilds_for_user;
use crate::auth::user_for_token;
use crate::perms;
use crate::state::{AppState, Audience};

const IDENTIFY_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(90);
/// Clients only send small JSON (identify, heartbeats, idle): anything
/// bigger is refused rather than buffered.
const MAX_CLIENT_MESSAGE: usize = 16 * 1024;
/// Open connections one account may have at once (tabs, phone, desktop app).
const MAX_CONNECTIONS_PER_USER: usize = 20;
/// How often an open connection re-checks its sign-in is still good, so
/// signing out (or changing your password) closes other connections too.
const SESSION_RECHECK: Duration = Duration::from_secs(60);

pub async fn gateway(ws: WebSocketUpgrade, State(st): State<AppState>) -> Response {
    ws.max_message_size(MAX_CLIENT_MESSAGE)
        .max_frame_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| async move {
            if let Err(e) = run(socket, st).await {
                tracing::debug!(error = %e, "gateway connection ended");
            }
        })
}

type Sink = SplitSink<WebSocket, WsMessage>;

async fn send(sink: &mut Sink, event: &ServerEvent) -> anyhow::Result<()> {
    let text = serde_json::to_string(event)?;
    sink.send(WsMessage::Text(text.into())).await?;
    Ok(())
}

/// Waits for the next client event, skipping anything that isn't valid JSON.
async fn next_event(stream: &mut SplitStream<WebSocket>) -> Option<ClientEvent> {
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            WsMessage::Text(text) => {
                if let Ok(event) = serde_json::from_str(text.as_str()) {
                    return Some(event);
                }
            }
            WsMessage::Close(_) => return None,
            _ => {}
        }
    }
    None
}

async fn reject(sink: &mut Sink, reason: &str) -> anyhow::Result<()> {
    send(
        sink,
        &ServerEvent::InvalidSession {
            reason: reason.into(),
        },
    )
    .await?;
    sink.send(WsMessage::Close(None)).await?;
    Ok(())
}

async fn run(socket: WebSocket, st: AppState) -> anyhow::Result<()> {
    let (mut sink, mut stream) = socket.split();

    let token = match tokio::time::timeout(IDENTIFY_TIMEOUT, next_event(&mut stream)).await {
        Ok(Some(ClientEvent::Identify { token })) => token,
        _ => return reject(&mut sink, "Send IDENTIFY first.").await,
    };
    let Some(user) = user_for_token(&st.db, &token).await? else {
        return reject(&mut sink, "Your session has expired. Sign in again.").await;
    };
    let open_now = st
        .presence
        .lock()
        .expect("presence lock")
        .get(&user.id)
        .copied()
        .unwrap_or(0);
    if open_now >= MAX_CONNECTIONS_PER_USER {
        // Just close (not INVALID_SESSION, which signs the client out).
        tracing::warn!(user = %user.username, "too many gateway connections");
        sink.send(WsMessage::Close(None)).await?;
        return Ok(());
    }

    // Subscribe before loading servers so nothing slips through the gap.
    let mut events = st.events.subscribe();
    let guilds = guilds_for_user(&st.db, user.id).await?;
    let mut guild_ids: HashSet<i64> = guilds.iter().map(|g| g.id).collect();
    send(
        &mut sink,
        &ServerEvent::Ready {
            user: user.clone(),
            guilds,
        },
    )
    .await?;
    tracing::debug!(user = %user.username, "gateway client ready");

    if st.presence_connect(user.id) {
        announce_presence(&st, &guild_ids, user.id, true);
    }

    // Whether this connection said it's idle (an atomic so the loop below
    // can update it and we can still read it however the loop ends).
    let idle = std::sync::atomic::AtomicBool::new(false);
    let outcome: anyhow::Result<()> = async {
        let mut last_heartbeat = Instant::now();
        let mut last_session_check = Instant::now();
        let mut watchdog = tokio::time::interval(Duration::from_secs(15));

        loop {
            tokio::select! {
                incoming = stream.next() => match incoming {
                    Some(Ok(WsMessage::Text(text))) => {
                        match serde_json::from_str::<ClientEvent>(text.as_str()) {
                            Ok(ClientEvent::Heartbeat) => {
                                last_heartbeat = Instant::now();
                                send(&mut sink, &ServerEvent::HeartbeatAck).await?;
                            }
                            Ok(ClientEvent::Idle { idle: now_idle })
                                if now_idle != idle.load(Ordering::Relaxed) =>
                            {
                                idle.store(now_idle, Ordering::Relaxed);
                                st.set_idle(user.id, now_idle);
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
                dispatch = events.recv() => match dispatch {
                    Ok(d) => {
                        let wanted = match d.audience {
                            Audience::Guild(id) => guild_ids.contains(&id),
                            Audience::User(id) => id == user.id,
                            Audience::Channel { guild_id, channel_id } => {
                                (guild_id == DM_GUILD || guild_ids.contains(&guild_id))
                                    && perms::can_view(&st.db, guild_id, channel_id, user.id).await
                                    // Nothing from an age-restricted channel until
                                    // they've said they're 18+.
                                    && (guild_id == DM_GUILD
                                        || !carries_content(&d.event)
                                        || perms::content_allowed(&st.db, channel_id, user.id).await)
                            }
                        };
                        if wanted {
                            match &d.event {
                                ServerEvent::GuildCreate(g) => {
                                    guild_ids.insert(g.id);
                                }
                                ServerEvent::GuildDelete { guild_id } => {
                                    guild_ids.remove(guild_id);
                                }
                                _ => {}
                            }
                            send(&mut sink, &d.event).await?;
                        }
                    }
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, user = %user.username, "gateway client fell behind");
                    }
                    Err(RecvError::Closed) => break,
                },
                _ = watchdog.tick() => {
                    if last_heartbeat.elapsed() > HEARTBEAT_TIMEOUT {
                        tracing::debug!(user = %user.username, "heartbeat timed out");
                        break;
                    }
                    if last_session_check.elapsed() >= SESSION_RECHECK {
                        last_session_check = Instant::now();
                        // A database hiccup isn't a sign-out: only a definite "no".
                        if let Ok(current) = user_for_token(&st.db, &token).await {
                            if current.map(|u| u.id) != Some(user.id) {
                                tracing::debug!(user = %user.username, "session ended; closing gateway");
                                let _ = send(&mut sink, &ServerEvent::InvalidSession {
                                    reason: "You've been signed out. Sign in again.".into(),
                                }).await;
                                let _ = sink.send(WsMessage::Close(None)).await;
                                break;
                            }
                        }
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    // This connection no longer counts towards "every connection is idle".
    if idle.load(Ordering::Relaxed) {
        st.set_idle(user.id, false);
    }

    if st.presence_disconnect(user.id) {
        crate::api::voice::clear_voice(&st, user.id, None);
        announce_presence(&st, &guild_ids, user.id, false);
    }
    outcome
}

/// Events that show what's in a channel (messages, reactions, threads,
/// raids), held back from age-restricted channels until accepted.
fn carries_content(event: &ServerEvent) -> bool {
    matches!(
        event,
        ServerEvent::MessageCreate(_)
            | ServerEvent::MessageUpdate(_)
            | ServerEvent::ReactionAdd { .. }
            | ServerEvent::ReactionRemove { .. }
            | ServerEvent::ThreadUpdate(_)
            | ServerEvent::EventUpdate(_)
            | ServerEvent::TypingStart { .. }
    )
}

fn announce_presence(st: &AppState, guild_ids: &HashSet<i64>, user_id: i64, online: bool) {
    for &guild_id in guild_ids {
        st.dispatch(
            Audience::Guild(guild_id),
            ServerEvent::PresenceUpdate { user_id, online },
        );
    }
}
