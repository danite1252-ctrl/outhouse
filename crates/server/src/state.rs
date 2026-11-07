use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use outhouse_common::models::VoiceState;

use outhouse_common::gateway::ServerEvent;
use outhouse_common::id::SnowflakeGen;
use sqlx::PgPool;
use tokio::sync::broadcast;

/// Who should receive a gateway event.
#[derive(Debug, Clone, Copy)]
pub enum Audience {
    /// Everyone who is a member of this server.
    Guild(i64),
    /// Members of the server who can see this channel.
    Channel { guild_id: i64, channel_id: i64 },
    /// One user (all of their connected clients).
    User(i64),
}

#[derive(Debug)]
pub struct Dispatch {
    pub audience: Audience,
    pub event: ServerEvent,
}

/// Settings read from the environment at start-up.
#[derive(Debug, Clone)]
pub struct Config {
    pub upload_dir: PathBuf,
    pub giphy_api_key: Option<String>,
    pub livekit_api_key: Option<String>,
    pub livekit_api_secret: Option<String>,
    /// Public LiveKit address; None = work it out from the request.
    pub livekit_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            upload_dir: std::env::var("UPLOAD_DIR")
                .unwrap_or_else(|_| "data/uploads".into())
                .into(),
            giphy_api_key: env_opt("GIPHY_API_KEY"),
            livekit_api_key: env_opt("LIVEKIT_API_KEY"),
            livekit_api_secret: env_opt("LIVEKIT_API_SECRET"),
            livekit_url: env_opt("LIVEKIT_URL"),
        }
    }
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub ids: Arc<SnowflakeGen>,
    /// In-process fan-out to connected gateway clients. Swap for Redis
    /// pub/sub if Outhouse ever runs on more than one server.
    pub events: broadcast::Sender<Arc<Dispatch>>,
    pub config: Arc<Config>,
    /// For calling trusted APIs (GIF search). Link previews use their own,
    /// locked-down client.
    pub http: reqwest::Client,
    /// User ID → number of open gateway connections.
    pub presence: Arc<Mutex<HashMap<i64, usize>>>,
    /// User ID → their voice channel (one at a time).
    pub voice: Arc<Mutex<HashMap<i64, VoiceState>>>,
    /// User ID → when they last played a soundboard clip.
    pub sound_cooldowns: Arc<Mutex<HashMap<i64, Instant>>>,
    /// Slash commands waiting for their bot to reply (kept 15 minutes).
    pub interactions: Arc<Mutex<HashMap<i64, PendingInteraction>>>,
}

/// Someone used a bot's slash command; the bot replies to this.
#[derive(Debug, Clone)]
pub struct PendingInteraction {
    pub bot_id: i64,
    pub guild_id: i64,
    pub channel_id: i64,
    pub user_id: i64,
    pub display_name: String,
    pub command: String,
    pub created: Instant,
}

impl AppState {
    pub fn new(db: PgPool, config: Config) -> Self {
        let (events, _) = broadcast::channel(1024);
        let http = reqwest::Client::builder()
            .user_agent("Outhouse/0.2")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("building HTTP client");
        Self {
            db,
            ids: Arc::new(SnowflakeGen::new(0)),
            events,
            config: Arc::new(config),
            http,
            presence: Arc::new(Mutex::new(HashMap::new())),
            voice: Arc::new(Mutex::new(HashMap::new())),
            sound_cooldowns: Arc::new(Mutex::new(HashMap::new())),
            interactions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Records a new connection. True if the user just came online.
    pub fn presence_connect(&self, user_id: i64) -> bool {
        let mut map = self.presence.lock().expect("presence lock");
        let count = map.entry(user_id).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// Records a closed connection. True if the user just went offline.
    pub fn presence_disconnect(&self, user_id: i64) -> bool {
        let mut map = self.presence.lock().expect("presence lock");
        match map.get_mut(&user_id) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                map.remove(&user_id);
                true
            }
            None => false,
        }
    }

    pub fn is_online(&self, user_id: i64) -> bool {
        self.presence
            .lock()
            .expect("presence lock")
            .contains_key(&user_id)
    }

    pub fn dispatch(&self, audience: Audience, event: ServerEvent) {
        // An error only means nobody is connected right now.
        let _ = self.events.send(Arc::new(Dispatch { audience, event }));
    }
}
