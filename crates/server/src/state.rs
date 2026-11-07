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
    /// Where Outhouse reaches LiveKit's API (server to server), e.g.
    /// http://127.0.0.1:7880.
    pub livekit_api_url: String,
    /// Where Wowhead lives (links and name searches), e.g. https://www.wowhead.com.
    pub wowhead_url: String,
    /// Wowhead's section for the game: "classic" for Classic era / WoW: Forever.
    pub wowhead_branch: String,
    /// Wowhead's tooltip service (item details as JSON).
    pub wowhead_tooltip_url: String,
    /// Wowhead's data set for tooltips (4 = Classic era).
    pub wowhead_data_env: String,
    /// Item icons: `{this}/{icon}.jpg`.
    pub wowhead_icon_url: String,
    /// Don't fetch pages for link previews (air-gapped servers and tests).
    /// WoW links still get a card made from the link itself.
    pub previews_offline: bool,
    /// The site's public address (e.g. https://outhouse.example), used for
    /// the Battle.net sign-in return address.
    pub public_url: Option<String>,
    /// Battle.net sign-in, when its keys are set.
    pub bnet: Option<BnetConfig>,
    /// Web Push: who the push services can contact about this server
    /// (a mailto: or https: address).
    pub vapid_subject: String,
    /// Extra push service hosts to allow (besides Google, Mozilla,
    /// Microsoft and Apple), comma-separated in `PUSH_ALLOWED_HOSTS`.
    pub push_allowed_hosts: Vec<String>,
    /// Endpoint prefixes allowed as-is, even over plain HTTP (tests only:
    /// `PUSH_EXTRA_ENDPOINTS=http://127.0.0.1:8097/`).
    pub push_extra_endpoints: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BnetConfig {
    pub client_id: String,
    pub client_secret: String,
    /// "us", "eu", "kr" or "tw".
    pub region: String,
    /// https://oauth.battle.net
    pub oauth_url: String,
    /// https://{region}.api.blizzard.com
    pub api_url: String,
    /// Profile namespaces to try for the character list, in order.
    pub namespaces: Vec<String>,
}

impl BnetConfig {
    fn from_env() -> Option<Self> {
        let client_id = env_opt("BNET_CLIENT_ID")?;
        let client_secret = env_opt("BNET_CLIENT_SECRET")?;
        let region = env_opt("BNET_REGION")
            .unwrap_or_else(|| "us".into())
            .to_ascii_lowercase();
        let namespaces = env_opt("BNET_PROFILE_NAMESPACES")
            .unwrap_or_else(|| "profile-classic1x-{region},profile-classic-{region}".into())
            .split(',')
            .map(|n| n.trim().replace("{region}", &region))
            .filter(|n| !n.is_empty())
            .collect();
        Some(Self {
            api_url: env_opt("BNET_API_URL")
                .unwrap_or_else(|| format!("https://{region}.api.blizzard.com"))
                .trim_end_matches('/')
                .to_string(),
            oauth_url: env_opt("BNET_OAUTH_URL")
                .unwrap_or_else(|| "https://oauth.battle.net".into())
                .trim_end_matches('/')
                .to_string(),
            client_id,
            client_secret,
            region,
            namespaces,
        })
    }
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
            livekit_api_url: url_env("LIVEKIT_API_URL", "http://127.0.0.1:7880"),
            wowhead_url: url_env("WOWHEAD_URL", "https://www.wowhead.com"),
            wowhead_branch: env_opt("WOWHEAD_BRANCH")
                .unwrap_or_else(|| "classic".into())
                .trim_matches('/')
                .to_string(),
            wowhead_tooltip_url: url_env("WOWHEAD_TOOLTIP_URL", "https://nether.wowhead.com"),
            wowhead_data_env: env_opt("WOWHEAD_DATA_ENV").unwrap_or_else(|| "4".into()),
            wowhead_icon_url: url_env(
                "WOWHEAD_ICON_URL",
                "https://wow.zamimg.com/images/wow/icons/large",
            ),
            previews_offline: matches!(
                env_opt("LINK_PREVIEWS_OFFLINE").as_deref(),
                Some("1" | "true" | "yes" | "on")
            ),
            public_url: env_opt("PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string()),
            bnet: BnetConfig::from_env(),
            vapid_subject: env_opt("VAPID_SUBJECT")
                .or_else(|| env_opt("PUBLIC_URL").filter(|u| u.starts_with("https://")))
                .unwrap_or_else(|| "mailto:admin@outhouse.invalid".into()),
            push_allowed_hosts: list_env("PUSH_ALLOWED_HOSTS"),
            push_extra_endpoints: list_env("PUSH_EXTRA_ENDPOINTS"),
        }
    }
}

fn url_env(name: &str, default: &str) -> String {
    env_opt(name)
        .unwrap_or_else(|| default.into())
        .trim_end_matches('/')
        .to_string()
}

fn list_env(name: &str) -> Vec<String> {
    env_opt(name)
        .map(|v| {
            v.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default()
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
    /// User ID → when their entrance sound last played.
    pub entrance_cooldowns: Arc<Mutex<HashMap<i64, Instant>>>,
    /// Slash commands waiting for their bot to reply (kept 15 minutes).
    pub interactions: Arc<Mutex<HashMap<i64, PendingInteraction>>>,
    /// User ID → how many of their open connections say they're idle
    /// (tab hidden or no input for a while). Push notifications go out when
    /// someone is offline or every connection is idle.
    pub idle: Arc<Mutex<HashMap<i64, usize>>>,
    /// The Web Push signing key (None if it couldn't be loaded).
    pub vapid: Option<Arc<crate::push::Vapid>>,
    pub started_at: chrono::DateTime<chrono::Utc>,
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
            entrance_cooldowns: Arc::new(Mutex::new(HashMap::new())),
            interactions: Arc::new(Mutex::new(HashMap::new())),
            idle: Arc::new(Mutex::new(HashMap::new())),
            vapid: None,
            started_at: chrono::Utc::now(),
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

    /// One of the user's connections became idle (true) or active again.
    pub fn set_idle(&self, user_id: i64, idle: bool) {
        let mut map = self.idle.lock().expect("idle lock");
        if idle {
            *map.entry(user_id).or_insert(0) += 1;
        } else if let Some(count) = map.get_mut(&user_id) {
            if *count > 1 {
                *count -= 1;
            } else {
                map.remove(&user_id);
            }
        }
    }

    /// Not looking at Outhouse right now: offline, or every open connection
    /// is idle.
    pub fn is_away(&self, user_id: i64) -> bool {
        let online = self
            .presence
            .lock()
            .expect("presence lock")
            .get(&user_id)
            .copied()
            .unwrap_or(0);
        if online == 0 {
            return true;
        }
        let idle = self
            .idle
            .lock()
            .expect("idle lock")
            .get(&user_id)
            .copied()
            .unwrap_or(0);
        idle >= online
    }

    pub fn dispatch(&self, audience: Audience, event: ServerEvent) {
        // An error only means nobody is connected right now.
        let _ = self.events.send(Arc::new(Dispatch { audience, event }));
    }
}
