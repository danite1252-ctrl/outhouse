//! WoW: Forever on Blizzard's Armory (the Battle.net game data and profile
//! APIs), read with the server's own Battle.net keys.
//!
//! Blizzard hasn't documented WoW: Forever's API; what's used here is what
//! players found in November 2026 (namespaces `profile-forever-{region}`,
//! `static-forever-{region}`; characters by "name-surname"). It's all
//! switched on by `BNET_CLIENT_ID` / `BNET_CLIENT_SECRET`, the namespaces can
//! be changed in the environment, and anything that goes wrong (404s, rate
//! limits, Blizzard being down) just means the Armory bits are left out.
//!
//! - App token (client credentials): fetched when needed, kept in memory
//!   until shortly before it expires, never logged or sent to browsers.
//! - Answers are kept for 5 minutes (a look-up followed by "Add character"
//!   reads the Armory once), 429s pause all calls for the time Blizzard asks.
//! - Portraits are copied into Postgres and served from
//!   `/api/wow/avatars/{key}`, so browsers never talk to Blizzard.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::Response;
use axum::Json;
use outhouse_common::models::Embed;
use outhouse_common::wow::{
    class_name, faction_of, ruleset_name, ArmoryLookup, ArmoryLookupRequest, ArmoryRefresh,
    WowCharacter, FOREVER_RACES, MAX_LEVEL,
};
use reqwest::{StatusCode, Url};
use serde_json::Value;

use crate::api::wow::{clean_name, clean_region, fetch_character};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, BnetConfig, BLIZZARD_RENDER_HOSTS};

const CALL_TIMEOUT: Duration = Duration::from_secs(6);
const ANSWER_TTL: Duration = Duration::from_secs(5 * 60);
/// Look-ups one person can do in [`LOOKUP_WINDOW`].
const LOOKUPS_PER_WINDOW: usize = 20;
const LOOKUP_WINDOW: Duration = Duration::from_secs(10 * 60);
/// "Refresh" on one character, at most this often.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(30);
const MAX_AVATAR_BYTES: usize = 1024 * 1024;
const OFF_MESSAGE: &str = "Armory look-ups aren't switched on on this server. An admin can turn them on with Battle.net API keys (see deploy/README.md).";

/// Battle.net app token, recent answers and rate limits (in memory only).
#[derive(Default)]
pub struct ArmoryState {
    token: tokio::sync::Mutex<Option<(String, Instant)>>,
    answers: Mutex<HashMap<String, (Instant, Profile)>>,
    backoff_until: Mutex<Option<Instant>>,
    lookups: Mutex<HashMap<i64, Vec<Instant>>>,
    refreshed: Mutex<HashMap<i64, Instant>>,
}

/// What we take from a character's Armory profile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub blizzard_id: Option<i64>,
    pub name: String,
    pub surname: String,
    /// One of the nine, or "" if Blizzard said something else.
    pub class: String,
    /// One of [`FOREVER_RACES`] or "".
    pub race: String,
    /// "Alliance", "Horde" or "".
    pub faction: String,
    pub level: Option<i32>,
    pub ruleset: String,
    pub guild: String,
    pub item_level: Option<i32>,
    /// The portrait on Blizzard's render server ("" = none).
    pub avatar_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArmoryError {
    /// No Battle.net keys.
    Off,
    NotFound,
    /// Blizzard asked us to slow down (or we're waiting after it did).
    Busy,
    Unavailable,
}

impl ArmoryError {
    pub fn reason(&self) -> &'static str {
        match self {
            ArmoryError::Off => "off",
            ArmoryError::NotFound => "not_found",
            ArmoryError::Busy => "busy",
            ArmoryError::Unavailable => "unavailable",
        }
    }

    pub fn message(&self, who: &str, region: &str) -> String {
        match self {
            ArmoryError::Off => OFF_MESSAGE.into(),
            ArmoryError::NotFound => format!(
                "The Armory doesn't know {who} on {} yet. Check the spelling and region; brand-new characters can take a while to show up.",
                region.to_uppercase()
            ),
            ArmoryError::Busy => {
                "The Armory is busy right now. Try again in a minute.".into()
            }
            ArmoryError::Unavailable => {
                "Couldn't reach the Armory just now. Try again in a bit.".into()
            }
        }
    }
}

// ---- Reading Blizzard's answers ---------------------------------------------

/// A string, or a localised one (`{"en_US": "…"}`).
fn text(v: &Value) -> Option<String> {
    v.as_str()
        .or_else(|| v["en_US"].as_str())
        .or_else(|| v["en_GB"].as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `"Paladin"`, `{"name": "Paladin", "id": 2}`, `{"type": "ALLIANCE"}` or `2`
/// → (name, id).
fn named(v: &Value) -> (Option<String>, Option<i64>) {
    if v.is_object() {
        (
            text(&v["name"]).or_else(|| text(&v["type"])),
            v["id"].as_i64(),
        )
    } else {
        (text(v), v.as_i64())
    }
}

/// The first of these keys that the object has.
fn first<'a>(v: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter()
        .map(|k| &v[*k])
        .find(|x| !x.is_null())
        .unwrap_or(&Value::Null)
}

pub fn class_of(v: &Value) -> String {
    let (name, id) = named(v);
    if let Some(c) = name.as_deref().and_then(class_name) {
        return c.to_string();
    }
    match id {
        Some(1) => "Warrior",
        Some(2) => "Paladin",
        Some(3) => "Hunter",
        Some(4) => "Rogue",
        Some(5) => "Priest",
        Some(7) => "Shaman",
        Some(8) => "Mage",
        Some(9) => "Warlock",
        Some(11) => "Druid",
        _ => "",
    }
    .to_string()
}

pub fn faction_from(v: &Value) -> String {
    let (name, _) = named(v);
    match name.map(|n| n.to_ascii_lowercase()).as_deref() {
        Some("alliance") => "Alliance",
        Some("horde") => "Horde",
        _ => "",
    }
    .to_string()
}

/// A race we know, from its name or (for the Classic races) its id. Skyborne
/// are neutral until they pick a side, so they need the faction.
pub fn race_of(v: &Value, faction: &str) -> String {
    let (name, id) = named(v);
    let name = name.unwrap_or_default();
    let key: String = name
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if key.starts_with("skyborne") {
        return match faction {
            "Alliance" => "Skyborne (Alliance)",
            "Horde" => "Skyborne (Horde)",
            _ if key.contains("alliance") => "Skyborne (Alliance)",
            _ if key.contains("horde") => "Skyborne (Horde)",
            _ => "",
        }
        .to_string();
    }
    if let Some(r) = FOREVER_RACES
        .iter()
        .find(|r| r.eq_ignore_ascii_case(name.trim()))
    {
        return r.to_string();
    }
    match (key.as_str(), id) {
        ("nightelf", _) | (_, Some(4)) => "Night Elf",
        ("scourge" | "forsaken", _) | (_, Some(5)) => "Undead",
        (_, Some(1)) => "Human",
        (_, Some(2)) => "Orc",
        (_, Some(3)) => "Dwarf",
        (_, Some(6)) => "Tauren",
        (_, Some(7)) => "Gnome",
        (_, Some(8)) => "Troll",
        _ => "",
    }
    .to_string()
}

pub fn ruleset_of(v: &Value) -> String {
    let (name, id) = named(v);
    name.as_deref()
        .and_then(ruleset_name)
        .or_else(|| id.and_then(|i| ruleset_name(&i.to_string())))
        .unwrap_or("")
        .to_string()
}

/// Average item level of what they're wearing (shirts and tabards don't
/// count), when the profile doesn't say.
pub fn item_level_of(profile: &Value, equipment: Option<&Value>) -> Option<i32> {
    for key in ["equipped_item_level", "average_item_level"] {
        if let Some(n) = profile[key].as_f64().filter(|n| *n > 0.0) {
            return Some(n.round() as i32);
        }
    }
    let items = equipment?["equipped_items"].as_array()?;
    let levels: Vec<f64> = items
        .iter()
        .filter(|i| {
            let slot = i["slot"]["type"].as_str().unwrap_or("");
            !matches!(slot, "SHIRT" | "TABARD" | "BODY")
        })
        .filter_map(|i| {
            i["level"]["value"]
                .as_f64()
                .or_else(|| i["item_level"].as_f64())
                .or_else(|| i["level"].as_f64())
        })
        .filter(|l| *l > 0.0)
        .collect();
    if levels.is_empty() {
        return None;
    }
    Some((levels.iter().sum::<f64>() / levels.len() as f64).round() as i32)
}

/// The portrait from `/character-media`: the avatar, else the inset, else the
/// full render.
pub fn avatar_of(media: &Value) -> Option<String> {
    let assets = media["assets"].as_array();
    for key in ["avatar", "inset", "main-raw", "main"] {
        let found = assets
            .into_iter()
            .flatten()
            .find(|a| a["key"].as_str() == Some(key))
            .and_then(|a| text(&a["value"]));
        if found.is_some() {
            return found;
        }
    }
    text(&media["avatar_url"])
}

/// A character from a profile (`/profile/wow/character/…`) or an entry in
/// the account list (`/profile/user/wow`), which use the same field names.
pub fn parse_character(v: &Value, equipment: Option<&Value>, media: Option<&Value>) -> Profile {
    let faction = faction_from(&v["faction"]);
    let race = race_of(first(v, &["playable_race", "race"]), &faction);
    let faction = if faction.is_empty() {
        faction_of(&race).to_string()
    } else {
        faction
    };
    Profile {
        blizzard_id: v["id"].as_i64(),
        name: text(&v["name"]).unwrap_or_default(),
        surname: text(first(v, &["surname", "last_name"])).unwrap_or_default(),
        class: class_of(first(v, &["playable_class", "character_class", "class"])),
        race,
        faction,
        level: v["level"].as_i64().map(|l| l as i32),
        ruleset: ruleset_of(&v["ruleset"]),
        guild: text(&v["guild"]["name"]).unwrap_or_default(),
        item_level: item_level_of(v, equipment),
        avatar_url: media.and_then(avatar_of).unwrap_or_default(),
    }
}

// ---- Calling Blizzard --------------------------------------------------------

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// "Thrall", "Stormborn" → "thrall-stormborn" (Blizzard's character slug).
pub fn slug(name: &str, surname: &str) -> String {
    format!(
        "{}-{}",
        name.trim().to_lowercase(),
        surname.trim().to_lowercase()
    )
}

/// Our name for a portrait: only a-z, 0-9 and dashes ("us-thrall-stormborn";
/// other letters become "u" and their code point).
pub fn avatar_key(region: &str, name: &str, surname: &str) -> String {
    let mut out = format!("{}-", region.to_ascii_lowercase());
    for c in slug(name, surname).chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            out.push(c);
        } else {
            out.push_str(&format!("u{:x}", c as u32));
        }
    }
    out
}

fn busy(st: &AppState) -> bool {
    st.armory
        .backoff_until
        .lock()
        .expect("armory lock")
        .is_some_and(|t| Instant::now() < t)
}

fn back_off(st: &AppState, res: &reqwest::Response) {
    let secs = res
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(30)
        .clamp(1, 300);
    tracing::info!(seconds = secs, "Battle.net asked us to slow down");
    *st.armory.backoff_until.lock().expect("armory lock") =
        Some(Instant::now() + Duration::from_secs(secs));
}

/// The app token (client credentials), from memory while it lasts.
async fn app_token(st: &AppState, cfg: &BnetConfig) -> Result<String, ArmoryError> {
    if busy(st) {
        return Err(ArmoryError::Busy);
    }
    let mut held = st.armory.token.lock().await;
    if let Some((token, until)) = held.as_ref() {
        if Instant::now() < *until {
            return Ok(token.clone());
        }
    }
    let res = st
        .http
        .post(format!("{}/token", cfg.oauth_url))
        .basic_auth(&cfg.client_id, Some(&cfg.client_secret))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .timeout(CALL_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e.without_url(), "Battle.net app token: no answer");
            ArmoryError::Unavailable
        })?;
    if res.status() == StatusCode::TOO_MANY_REQUESTS {
        back_off(st, &res);
        return Err(ArmoryError::Busy);
    }
    if !res.status().is_success() {
        tracing::warn!(status = %res.status(), "Battle.net app token refused (check BNET_CLIENT_ID / BNET_CLIENT_SECRET)");
        return Err(ArmoryError::Unavailable);
    }
    let v: Value = res.json().await.map_err(|_| ArmoryError::Unavailable)?;
    let token = v["access_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or(ArmoryError::Unavailable)?
        .to_string();
    let lasts = v["expires_in"]
        .as_u64()
        .unwrap_or(3600)
        .clamp(120, 86_400 * 2);
    *held = Some((
        token.clone(),
        Instant::now() + Duration::from_secs(lasts - 60),
    ));
    Ok(token)
}

async fn get_json(st: &AppState, token: &str, url: &str) -> Result<Value, ArmoryError> {
    if busy(st) {
        return Err(ArmoryError::Busy);
    }
    let res = st
        .http
        .get(url)
        .bearer_auth(token)
        .timeout(CALL_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            tracing::info!(error = %e.without_url(), "Battle.net call failed");
            ArmoryError::Unavailable
        })?;
    let status = res.status();
    if status.is_success() {
        return res.json().await.map_err(|_| ArmoryError::Unavailable);
    }
    match status {
        StatusCode::NOT_FOUND => Err(ArmoryError::NotFound),
        StatusCode::TOO_MANY_REQUESTS => {
            back_off(st, &res);
            Err(ArmoryError::Busy)
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            // The token may have been revoked early: get a new one next time.
            *st.armory.token.lock().await = None;
            Err(ArmoryError::Unavailable)
        }
        _ => {
            tracing::info!(%status, "Battle.net call refused");
            Err(ArmoryError::Unavailable)
        }
    }
}

/// A WoW: Forever character's profile, gear and portrait.
pub async fn fetch_profile(
    st: &AppState,
    region: &str,
    name: &str,
    surname: &str,
    fresh: bool,
) -> Result<Profile, ArmoryError> {
    let cfg = st.config.bnet.as_ref().ok_or(ArmoryError::Off)?;
    let slug = slug(name, surname);
    let key = format!("{region}/{slug}");
    {
        let mut answers = st.armory.answers.lock().expect("armory lock");
        answers.retain(|_, (at, _)| at.elapsed() < ANSWER_TTL);
        if fresh {
            answers.remove(&key);
        } else if let Some((_, p)) = answers.get(&key) {
            return Ok(p.clone());
        }
    }
    let token = app_token(st, cfg).await?;
    let base = format!(
        "{}/profile/wow/character/{}",
        cfg.api_for(region),
        url_encode(&slug)
    );
    let query = format!(
        "namespace={}&locale=en_US",
        url_encode(&cfg.namespace(&cfg.forever_profile_namespace, region))
    );
    let profile = get_json(st, &token, &format!("{base}?{query}")).await?;
    let equipment_url = format!("{base}/equipment?{query}");
    let media_url = format!("{base}/character-media?{query}");
    let (equipment, media) = tokio::join!(
        get_json(st, &token, &equipment_url),
        get_json(st, &token, &media_url)
    );
    let mut p = parse_character(&profile, equipment.as_ref().ok(), media.as_ref().ok());
    if p.name.is_empty() {
        p.name = name.to_string();
    }
    if p.surname.is_empty() {
        p.surname = surname.to_string();
    }
    st.armory
        .answers
        .lock()
        .expect("armory lock")
        .insert(key, (Instant::now(), p.clone()));
    Ok(p)
}

/// Blizzard's portrait hosts, the API's own host (a stand-in in tests) and
/// any in `ARMORY_RENDER_HOSTS`.
fn render_host_ok(cfg: &BnetConfig, url: &Url) -> bool {
    let Some(host) = url.host_str().map(|h| h.to_ascii_lowercase()) else {
        return false;
    };
    if !matches!(url.scheme(), "https" | "http") {
        return false;
    }
    let api_host = Url::parse(&cfg.api_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()));
    BLIZZARD_RENDER_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
        || api_host.as_deref() == Some(host.as_str())
        || cfg
            .render_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&host))
}

/// Copies a portrait into the database (once a day at most); its key, or
/// None if there's none or it couldn't be fetched.
async fn store_avatar(st: &AppState, key: &str, source: &str) -> Option<String> {
    let cfg = st.config.bnet.as_ref()?;
    let url = Url::parse(source).ok()?;
    if !render_host_ok(cfg, &url) {
        tracing::info!(host = ?url.host_str(), "Armory portrait from an unexpected host, skipped");
        return None;
    }
    let fresh: Option<bool> = sqlx::query_scalar(
        "SELECT fetched_at > now() - interval '1 day' FROM wow_avatars
         WHERE key = $1 AND source_url = $2",
    )
    .bind(key)
    .bind(source)
    .fetch_optional(&st.db)
    .await
    .ok()?;
    if fresh == Some(true) {
        return Some(key.to_string());
    }
    let res = st.http.get(url).timeout(CALL_TIMEOUT).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        content_type.as_str(),
        "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    ) {
        return None;
    }
    if res
        .content_length()
        .is_some_and(|n| n as usize > MAX_AVATAR_BYTES)
    {
        return None;
    }
    let data = res.bytes().await.ok()?;
    if data.is_empty() || data.len() > MAX_AVATAR_BYTES {
        return None;
    }
    sqlx::query(
        "INSERT INTO wow_avatars (key, source_url, content_type, data) VALUES ($1, $2, $3, $4)
         ON CONFLICT (key) DO UPDATE SET source_url = EXCLUDED.source_url,
             content_type = EXCLUDED.content_type, data = EXCLUDED.data, fetched_at = now()",
    )
    .bind(key)
    .bind(source)
    .bind(&content_type)
    .bind(data.as_ref())
    .execute(&st.db)
    .await
    .ok()?;
    Some(key.to_string())
}

/// Reads the Armory for one of our characters and saves what it says.
/// `Ok(Err(…))`: the Armory didn't have it (or couldn't be asked).
pub async fn sync(
    st: &AppState,
    id: i64,
    fresh: bool,
) -> AppResult<Result<WowCharacter, ArmoryError>> {
    let c = fetch_character(&st.db, id).await?;
    if c.game != "forever" || c.last_name.is_empty() {
        return Ok(Err(ArmoryError::NotFound));
    }
    let region = if c.region.is_empty() {
        st.config.armory_region.clone()
    } else {
        c.region.clone()
    };
    let p = match fetch_profile(st, &region, &c.name, &c.last_name, fresh).await {
        Ok(p) => p,
        Err(ArmoryError::NotFound) => {
            sqlx::query(
                "UPDATE wow_characters SET armory_status = 'not_found', last_synced_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .execute(&st.db)
            .await?;
            return Ok(Err(ArmoryError::NotFound));
        }
        Err(e) => return Ok(Err(e)),
    };
    let key = if p.avatar_url.is_empty() {
        None
    } else {
        store_avatar(
            st,
            &avatar_key(&region, &c.name, &c.last_name),
            &p.avatar_url,
        )
        .await
    };
    let level = p.level.map(|l| l.clamp(1, MAX_LEVEL));
    sqlx::query(
        "UPDATE wow_characters SET
             class = CASE WHEN $2 = '' THEN class ELSE $2 END,
             race = CASE WHEN $3 = '' THEN race ELSE $3 END,
             level = COALESCE($4, level),
             ruleset = CASE WHEN $5 = '' THEN ruleset ELSE $5 END,
             blizzard_id = COALESCE($6, blizzard_id),
             guild_name = $7,
             item_level = COALESCE($8, item_level),
             avatar_url = CASE WHEN $9 = '' THEN avatar_url ELSE $9 END,
             avatar_key = COALESCE($10, avatar_key),
             region = $11,
             last_synced_at = now(),
             armory_status = ''
         WHERE id = $1",
    )
    .bind(id)
    .bind(&p.class)
    .bind(&p.race)
    .bind(level)
    .bind(&p.ruleset)
    .bind(p.blizzard_id)
    .bind(&p.guild)
    .bind(p.item_level)
    .bind(&p.avatar_url)
    .bind(key)
    .bind(&region)
    .execute(&st.db)
    .await?;
    Ok(Ok(fetch_character(&st.db, id).await?))
}

/// [`sync`], for when the Armory is a nice extra: problems are only logged.
/// `fresh`: don't use an answer from the last few minutes.
pub async fn sync_quietly(st: &AppState, id: i64, fresh: bool) {
    if st.config.bnet.is_none() {
        return;
    }
    match tokio::time::timeout(Duration::from_secs(15), sync(st, id, fresh)).await {
        Ok(Ok(Ok(_))) => {}
        Ok(Ok(Err(e))) => tracing::info!(character = id, reason = e.reason(), "no Armory data"),
        Ok(Err(e)) => tracing::warn!(character = id, error = %e, "saving Armory data failed"),
        Err(_) => tracing::info!(character = id, "the Armory took too long"),
    }
}

// ---- Endpoints -------------------------------------------------------------

fn region_or_default(st: &AppState, raw: &str) -> AppResult<String> {
    let region = clean_region(raw)?;
    Ok(if region.is_empty() {
        st.config.armory_region.clone()
    } else {
        region
    })
}

/// `POST /api/wow/armory/lookup` `{"region": "us", "name": "Thrall",
/// "last_name": "Stormborn"}`: what the Armory says (`found: false` with a
/// reason and message when it can't say).
pub async fn lookup(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<ArmoryLookupRequest>,
) -> AppResult<Json<ArmoryLookup>> {
    if st.config.bnet.is_none() {
        return Err(AppError::BadRequest(OFF_MESSAGE.into()));
    }
    let region = region_or_default(&st, &req.region)?;
    let name = clean_name(&req.name, "First names are")?;
    if req.last_name.trim().is_empty() {
        return Err(AppError::BadRequest(
            "The Armory finds WoW: Forever characters by first and last name: fill in both.".into(),
        ));
    }
    let surname = clean_name(&req.last_name, "Last names are")?;
    {
        let mut all = st.armory.lookups.lock().expect("armory lock");
        all.retain(|_, v| {
            v.retain(|t| t.elapsed() < LOOKUP_WINDOW);
            !v.is_empty()
        });
        let mine = all.entry(user.id).or_default();
        if mine.len() >= LOOKUPS_PER_WINDOW {
            return Err(AppError::BadRequest(
                "That's a lot of look-ups. Give the Armory a few minutes' rest.".into(),
            ));
        }
        mine.push(Instant::now());
    }
    let who = format!("{name} {surname}");
    let p = match fetch_profile(&st, &region, &name, &surname, false).await {
        Ok(p) => p,
        Err(e) => {
            return Ok(Json(ArmoryLookup {
                found: false,
                reason: e.reason().into(),
                message: e.message(&who, &region),
                region,
                name,
                last_name: surname,
                ..Default::default()
            }))
        }
    };
    let avatar = if p.avatar_url.is_empty() {
        String::new()
    } else {
        let key = avatar_key(&region, &name, &surname);
        match store_avatar(&st, &key, &p.avatar_url).await {
            Some(k) => format!("/api/wow/avatars/{k}?v={}", chrono::Utc::now().timestamp()),
            None => String::new(),
        }
    };
    Ok(Json(ArmoryLookup {
        found: true,
        region,
        blizzard_id: p.blizzard_id,
        name,
        last_name: surname,
        class: p.class,
        race: p.race,
        faction: p.faction,
        level: p.level.map(|l| l.clamp(1, MAX_LEVEL)),
        ruleset: p.ruleset,
        guild_name: p.guild,
        item_level: p.item_level,
        avatar,
        ..Default::default()
    }))
}

/// `POST /api/characters/{id}/refresh`: reads the Armory again.
pub async fn refresh(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<ArmoryRefresh>> {
    let c = fetch_character(&st.db, id).await?;
    if c.user_id != user.id {
        return Err(AppError::NotFound);
    }
    if st.config.bnet.is_none() {
        return Err(AppError::BadRequest(OFF_MESSAGE.into()));
    }
    if c.game != "forever" {
        return Err(AppError::BadRequest(
            "Only WoW: Forever characters are on the Armory here.".into(),
        ));
    }
    if c.last_name.is_empty() {
        return Err(AppError::BadRequest(
            "Add their last name first: the Armory finds WoW: Forever characters by first and last name.".into(),
        ));
    }
    let recent = {
        let mut done = st.armory.refreshed.lock().expect("armory lock");
        done.retain(|_, t| t.elapsed() < REFRESH_COOLDOWN);
        let recent = done.contains_key(&id);
        if !recent {
            done.insert(id, Instant::now());
        }
        recent
    };
    if recent {
        return Ok(Json(ArmoryRefresh {
            found: c.armory_status != "not_found",
            message: "Refreshed a moment ago. Give it a minute.".into(),
            character: c,
        }));
    }
    let region = if c.region.is_empty() {
        st.config.armory_region.clone()
    } else {
        c.region.clone()
    };
    let who = c.full_name.clone();
    match sync(&st, id, true).await? {
        Ok(character) => Ok(Json(ArmoryRefresh {
            character,
            found: true,
            message: String::new(),
        })),
        Err(e) => Ok(Json(ArmoryRefresh {
            character: fetch_character(&st.db, id).await?,
            found: false,
            message: e.message(&who, &region),
        })),
    }
}

/// `GET /api/wow/avatars/{key}`: a copied Armory portrait (public, like
/// the Armory itself).
pub async fn avatar(State(st): State<AppState>, Path(key): Path<String>) -> AppResult<Response> {
    if key.is_empty()
        || key.len() > 120
        || !key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(AppError::NotFound);
    }
    let row: Option<(String, Vec<u8>)> =
        sqlx::query_as("SELECT content_type, data FROM wow_avatars WHERE key = $1")
            .bind(&key)
            .fetch_optional(&st.db)
            .await?;
    let (content_type, data) = row.ok_or(AppError::NotFound)?;
    let content_type = if content_type.starts_with("image/") {
        content_type
    } else {
        "application/octet-stream".into()
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .body(Body::from(data))
        .map_err(|e| AppError::Other(e.into()))
}

// ---- Keeping levels fresh ----------------------------------------------------

/// Every half hour, reads the Armory again for characters going to raids in
/// the next two weeks (every `ARMORY_REFRESH_HOURS`, default 6) and, once a
/// day, for anyone else it has seen before. A few at a time, slowly.
pub fn spawn_refresher(st: AppState) {
    if st.config.bnet.is_none() || st.config.armory_refresh_hours == 0 {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30 * 60));
        loop {
            tick.tick().await;
            if let Err(e) = refresh_due(&st).await {
                tracing::warn!(error = %e, "Armory refresh failed");
            }
        }
    });
}

async fn refresh_due(st: &AppState) -> AppResult<()> {
    let hours = st.config.armory_refresh_hours.min(24 * 30) as i32;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT c.id FROM wow_characters c
         WHERE c.game = 'forever' AND c.last_name <> ''
           AND (c.armory_status <> 'not_found' OR c.last_synced_at < now() - interval '1 day')
           AND ((EXISTS (SELECT 1 FROM event_signups s JOIN events e ON e.id = s.event_id
                         WHERE s.character_id = c.id
                           AND e.starts_at > now() - interval '3 hours'
                           AND e.starts_at < now() + interval '14 days')
                 AND (c.last_synced_at IS NULL
                      OR c.last_synced_at < now() - make_interval(hours => $1)))
                OR (c.last_synced_at IS NOT NULL
                    AND c.last_synced_at < now() - interval '1 day'))
         ORDER BY c.last_synced_at NULLS FIRST
         LIMIT 25",
    )
    .bind(hours)
    .fetch_all(&st.db)
    .await?;
    for id in ids {
        if busy(st) {
            break;
        }
        match sync(st, id, true).await {
            Ok(Err(ArmoryError::Busy | ArmoryError::Unavailable | ArmoryError::Off)) => break,
            Err(e) => tracing::warn!(character = id, error = %e, "Armory refresh failed"),
            _ => {}
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    // Portraits nobody uses any more (look-ups that weren't saved, old ones).
    sqlx::query(
        "DELETE FROM wow_avatars a
         WHERE a.fetched_at < now() - interval '1 day'
           AND NOT EXISTS (SELECT 1 FROM wow_characters c WHERE c.avatar_key = a.key)",
    )
    .execute(&st.db)
    .await?;
    Ok(())
}

// ---- /item from WoW: Forever's own item data -----------------------------------

fn quality_of(v: &Value) -> i32 {
    let (name, _) = named(v);
    match name
        .map(|n| n.to_ascii_uppercase())
        .as_deref()
        .unwrap_or("")
    {
        "POOR" => 0,
        "COMMON" => 1,
        "UNCOMMON" => 2,
        "RARE" => 3,
        "EPIC" => 4,
        "LEGENDARY" => 5,
        "ARTIFACT" => 6,
        "HEIRLOOM" => 7,
        _ => 1,
    }
}

/// An `/item` card from Battle.net's `static-forever` item data (used when
/// Wowhead doesn't have the item); None if it isn't there either.
pub async fn item_embed(st: &AppState, query: &str) -> Option<Embed> {
    let cfg = st.config.bnet.as_ref()?;
    let region = cfg.region.clone();
    let token = app_token(st, cfg).await.ok()?;
    let api = cfg.api_for(&region);
    let ns = url_encode(&cfg.namespace(&cfg.forever_static_namespace, &region));
    let query = query.trim();
    let id = match crate::wowhead::item_id_in(query) {
        Some(id) => id as i64,
        None => {
            let url = format!(
                "{api}/data/wow/search/item?namespace={ns}&locale=en_US&name.en_US={}&orderby=id&_page=1",
                url_encode(query)
            );
            let v = get_json(st, &token, &url).await.ok()?;
            let results = v["results"].as_array()?;
            let pick = results
                .iter()
                .find(|r| text(&r["data"]["name"]).is_some_and(|n| n.eq_ignore_ascii_case(query)))
                .or_else(|| results.first())?;
            pick["data"]["id"].as_i64()?
        }
    };
    if !(1..=10_000_000).contains(&id) {
        return None;
    }
    let item = get_json(
        st,
        &token,
        &format!("{api}/data/wow/item/{id}?namespace={ns}&locale=en_US"),
    )
    .await
    .ok()?;
    let media = get_json(
        st,
        &token,
        &format!("{api}/data/wow/media/item/{id}?namespace={ns}&locale=en_US"),
    )
    .await
    .ok();
    let name = text(&item["name"])?;
    let mut lines = Vec::new();
    let kind = [
        text(&item["inventory_type"]["name"]),
        text(&item["item_subclass"]["name"]),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    if !kind.is_empty() {
        lines.push(kind);
    }
    if let Some(l) = item["level"].as_i64().filter(|l| *l > 0) {
        lines.push(format!("Item Level {l}"));
    }
    if let Some(l) = item["required_level"].as_i64().filter(|l| *l > 0) {
        lines.push(format!("Requires Level {l}"));
    }
    let info = crate::wowhead::ItemInfo {
        item_id: id as i32,
        name,
        quality: quality_of(&item["quality"]),
        icon: String::new(),
        tooltip: lines.join("\n"),
        lines: String::new(),
    };
    let mut embed = crate::wowhead::item_embed(&st.config, &info, None);
    embed.thumbnail_url = media
        .as_ref()
        .and_then(|m| {
            m["assets"]
                .as_array()?
                .iter()
                .find(|a| a["key"].as_str() == Some("icon"))
                .and_then(|a| text(&a["value"]))
        })
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"));
    embed.footer = Some(format!("Item {id} · WoW: Forever data from Battle.net"));
    Some(embed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_forever_profiles() {
        let profile: Value = serde_json::from_str(
            r#"{"id": 1001, "name": "Breezy", "surname": "Cloudsong", "level": 37,
                "playable_class": {"id": 7, "name": "Shaman"},
                "playable_race": {"id": 99, "name": "Skyborne"},
                "faction": {"type": "HORDE", "name": "Horde"},
                "ruleset": {"id": 15},
                "guild": {"name": "Forever Flush", "id": 5}}"#,
        )
        .unwrap();
        let equipment: Value = serde_json::from_str(
            r#"{"equipped_items": [
                {"slot": {"type": "HEAD"}, "level": {"value": 40}},
                {"slot": {"type": "SHIRT"}, "level": {"value": 1}},
                {"slot": {"type": "CHEST"}, "level": {"value": 45}}]}"#,
        )
        .unwrap();
        let media: Value = serde_json::from_str(
            r#"{"assets": [{"key": "main-raw", "value": "https://render.worldofwarcraft.com/forever-us/main.png"},
                           {"key": "avatar", "value": "https://render.worldofwarcraft.com/forever-us/avatar.jpg"}]}"#,
        )
        .unwrap();
        let p = parse_character(&profile, Some(&equipment), Some(&media));
        assert_eq!(p.blizzard_id, Some(1001));
        assert_eq!(
            (p.name.as_str(), p.surname.as_str()),
            ("Breezy", "Cloudsong")
        );
        assert_eq!(p.class, "Shaman");
        assert_eq!(p.race, "Skyborne (Horde)");
        assert_eq!(p.faction, "Horde");
        assert_eq!(p.ruleset, "Roleplay PvP");
        assert_eq!(p.guild, "Forever Flush");
        assert_eq!(p.item_level, Some(43));
        assert!(p.avatar_url.ends_with("/avatar.jpg"));

        // The account list: plain strings, ids only, no realm.
        let listed: Value = serde_json::from_str(
            r#"{"id": 7, "name": "Stumpy", "surname": "Ironbelly", "level": 12,
                "playable_class": {"id": 1}, "playable_race": {"id": 3},
                "faction": {"type": "ALLIANCE"}, "equipped_item_level": 14.4}"#,
        )
        .unwrap();
        let p = parse_character(&listed, None, None);
        assert_eq!(
            (p.class.as_str(), p.race.as_str(), p.faction.as_str()),
            ("Warrior", "Dwarf", "Alliance")
        );
        assert_eq!(p.item_level, Some(14));
        assert_eq!(p.ruleset, "");
        assert_eq!(race_of(&Value::from("Skyborne"), ""), "");
        assert_eq!(race_of(&Value::from("Scourge"), ""), "Undead");
        assert_eq!(race_of(&Value::from("Night Elf"), ""), "Night Elf");
    }

    #[test]
    fn keys_are_safe() {
        assert_eq!(
            avatar_key("us", "Thrall", "Stormborn"),
            "us-thrall-stormborn"
        );
        assert_eq!(avatar_key("eu", "Zoë", "Ash"), "eu-zoueb-ash");
        assert_eq!(slug(" Thrall", "StormBorn "), "thrall-stormborn");
    }
}
