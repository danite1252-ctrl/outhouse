//! Premium: "Golden Throne" (a person) and "Golden Outhouse" (an outhouse).
//!
//! There are no tiers: something is premium or it isn't. Entitlements are
//! rows in `user_premium` / `outhouse_premium` (a grant by the site owner,
//! or staff with `grant_premium`, now; Stripe later just adds rows with
//! source 'stripe'). Every Golden Throne member also has one free Golden
//! Outhouse slot (`premium_slots`) they can put on any outhouse they're in,
//! movable once every 7 days. An outhouse is premium while it has an active
//! entitlement or a slot from a member who still has Golden Throne.
//!
//! Every check on the server goes through [`user_has_premium`] /
//! [`guild_has_premium`] (cached for a few seconds; SQL uses the same
//! `oh_user_premium` / `oh_guild_premium` functions). Users, members and
//! outhouses carry a `premium` flag for the web client.
//!
//! The owner's switches (`premium_settings`, SITE OWNER → Premium) decide
//! whether a few perks are premium or free for everyone: message length,
//! other outhouses' emoji and animated pictures. Environment variables give
//! their defaults.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::DM_GUILD;
use outhouse_common::permissions as p;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::staff::StaffActor;
use crate::audit;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::member_ctx;
use crate::state::{AppState, Audience};

// ---- Settings from the environment ---------------------------------------------------

pub const FREE_UPLOAD_BYTES: usize = super::messages::MAX_FILE_BYTES;

pub struct PremiumConfig {
    /// Per-file upload limit in premium outhouses and premium people's DMs
    /// (`PREMIUM_UPLOAD_MB`, default 100).
    pub upload_bytes: usize,
    /// Personal soundboard sounds per Golden Throne member
    /// (`PREMIUM_PERSONAL_SOUNDS`, default 10).
    pub personal_sounds: i64,
    env_free_chars: usize,
    env_premium_chars: usize,
    env_emoji_premium: bool,
    env_animated_premium: bool,
}

static CONFIG: OnceLock<PremiumConfig> = OnceLock::new();

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name).ok().map(|v| v.trim().to_lowercase()) {
        Some(v) if ["1", "true", "yes", "on", "premium"].contains(&v.as_str()) => true,
        Some(v) if ["0", "false", "no", "off", "free"].contains(&v.as_str()) => false,
        _ => default,
    }
}

pub fn config() -> &'static PremiumConfig {
    CONFIG.get_or_init(|| {
        let upload_mb = env_num("PREMIUM_UPLOAD_MB", 100).clamp(25, 2048);
        PremiumConfig {
            upload_bytes: upload_mb * 1024 * 1024,
            personal_sounds: env_num("PREMIUM_PERSONAL_SOUNDS", 10).min(200) as i64,
            env_free_chars: env_num("MESSAGE_CHARS", 2000).clamp(100, 20_000),
            env_premium_chars: env_num("PREMIUM_MESSAGE_CHARS", 4000).clamp(100, 20_000),
            env_emoji_premium: env_bool("PREMIUM_EMOJI", true),
            env_animated_premium: env_bool("PREMIUM_ANIMATED", false),
        }
    })
}

// ---- The owner's switches ----------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Switches {
    pub free_chars: usize,
    pub premium_chars: usize,
    /// Other outhouses' custom emoji need Golden Throne.
    pub emoji_premium: bool,
    /// Animated pictures need premium (others see the first frame).
    pub animated_premium: bool,
}

static SWITCHES: RwLock<Option<Switches>> = RwLock::new(None);

fn env_switches() -> Switches {
    let c = config();
    Switches {
        free_chars: c.env_free_chars,
        premium_chars: c.env_premium_chars.max(c.env_free_chars),
        emoji_premium: c.env_emoji_premium,
        animated_premium: c.env_animated_premium,
    }
}

pub fn switches() -> Switches {
    SWITCHES
        .read()
        .ok()
        .and_then(|s| *s)
        .unwrap_or_else(env_switches)
}

#[derive(sqlx::FromRow)]
struct SettingsRow {
    free_message_chars: Option<i32>,
    premium_message_chars: Option<i32>,
    emoji_premium: Option<bool>,
    animated_premium: Option<bool>,
}

/// Reads the owner's switches (environment defaults for any not set) and
/// notes the animated-pictures one for SQL (`oh_file_url`).
pub async fn load_switches(db: &PgPool) -> AppResult<Switches> {
    let row = sqlx::query_as::<_, SettingsRow>(
        "SELECT free_message_chars, premium_message_chars, emoji_premium, animated_premium
         FROM premium_settings WHERE id = 1",
    )
    .fetch_optional(db)
    .await?;
    let env = env_switches();
    let s = match row {
        Some(r) => {
            let free = r
                .free_message_chars
                .map(|n| n as usize)
                .unwrap_or(env.free_chars);
            Switches {
                free_chars: free,
                premium_chars: r
                    .premium_message_chars
                    .map(|n| n as usize)
                    .unwrap_or(env.premium_chars)
                    .max(free),
                emoji_premium: r.emoji_premium.unwrap_or(env.emoji_premium),
                animated_premium: r.animated_premium.unwrap_or(env.animated_premium),
            }
        }
        None => env,
    };
    sqlx::query(
        "INSERT INTO premium_settings (id, animated_premium_now) VALUES (1, $1)
         ON CONFLICT (id) DO UPDATE SET animated_premium_now = $1",
    )
    .bind(s.animated_premium)
    .execute(db)
    .await?;
    if let Ok(mut w) = SWITCHES.write() {
        *w = Some(s);
    }
    Ok(s)
}

// ---- Is it premium? (cached briefly) -------------------------------------------------

const CACHE_TTL: Duration = Duration::from_secs(10);
static USER_CACHE: Mutex<Option<HashMap<i64, (bool, Instant)>>> = Mutex::new(None);
static GUILD_CACHE: Mutex<Option<HashMap<i64, (bool, Instant)>>> = Mutex::new(None);

fn cached(cache: &Mutex<Option<HashMap<i64, (bool, Instant)>>>, id: i64) -> Option<bool> {
    let guard = cache.lock().ok()?;
    guard
        .as_ref()?
        .get(&id)
        .filter(|(_, at)| at.elapsed() < CACHE_TTL)
        .map(|(v, _)| *v)
}

fn remember(cache: &Mutex<Option<HashMap<i64, (bool, Instant)>>>, id: i64, value: bool) {
    if let Ok(mut guard) = cache.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() > 50_000 {
            map.clear();
        }
        map.insert(id, (value, Instant::now()));
    }
}

/// Forgets every cached answer (after a grant, revoke, expiry or slot move:
/// a person's slot changes an outhouse's answer too).
pub fn forget_all() {
    for cache in [&USER_CACHE, &GUILD_CACHE] {
        if let Ok(mut guard) = cache.lock() {
            *guard = None;
        }
    }
}

/// Does this person have Golden Throne right now?
pub async fn user_has_premium(db: &PgPool, user_id: i64) -> AppResult<bool> {
    if let Some(v) = cached(&USER_CACHE, user_id) {
        return Ok(v);
    }
    let v: bool = sqlx::query_scalar("SELECT oh_user_premium($1)")
        .bind(user_id)
        .fetch_one(db)
        .await?;
    remember(&USER_CACHE, user_id, v);
    Ok(v)
}

/// Is this outhouse a Golden Outhouse right now?
pub async fn guild_has_premium(db: &PgPool, guild_id: i64) -> AppResult<bool> {
    if guild_id == DM_GUILD {
        return Ok(false);
    }
    if let Some(v) = cached(&GUILD_CACHE, guild_id) {
        return Ok(v);
    }
    let v: bool = sqlx::query_scalar("SELECT oh_guild_premium($1)")
        .bind(guild_id)
        .fetch_one(db)
        .await?;
    remember(&GUILD_CACHE, guild_id, v);
    Ok(v)
}

// ---- Limits used elsewhere ---------------------------------------------------------------

/// The biggest file one person may upload in a channel of `guild_id`:
/// the premium size in a Golden Outhouse, and in DMs for Golden Throne
/// senders; 25 MB otherwise.
pub async fn upload_limit(db: &PgPool, guild_id: i64, user_id: i64) -> AppResult<usize> {
    let premium = if guild_id == DM_GUILD {
        user_has_premium(db, user_id).await?
    } else {
        guild_has_premium(db, guild_id).await?
    };
    Ok(if premium {
        config().upload_bytes
    } else {
        FREE_UPLOAD_BYTES
    })
}

/// The largest any upload can be (the body-size layers allow this much;
/// each handler then applies [`upload_limit`]).
pub fn max_upload_bytes() -> usize {
    config().upload_bytes.max(FREE_UPLOAD_BYTES)
}

pub fn mb(bytes: usize) -> usize {
    bytes / 1024 / 1024
}

/// "x.pdf is too big (25 MB max…)" with a pointer to Golden Outhouse.
pub fn too_big(filename: &str, limit: usize, dm: bool) -> AppError {
    let more = if limit < config().upload_bytes {
        if dm {
            format!(
                " Golden Throne members can send up to {} MB in DMs.",
                mb(config().upload_bytes)
            )
        } else {
            format!(
                " Golden Outhouses take files up to {} MB.",
                mb(config().upload_bytes)
            )
        }
    } else {
        String::new()
    };
    AppError::TooLarge(format!(
        "{filename} is too big ({} MB max here).{more}",
        mb(limit)
    ))
}

/// Refuses a request whose Content-Length is already over `limit` (413
/// before any of the body is read).
pub fn check_content_length(headers: &HeaderMap, limit: usize, dm: bool) -> AppResult<()> {
    let len = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if let Some(len) = len {
        if len > limit {
            return Err(too_big("That upload", limit, dm));
        }
    }
    Ok(())
}

/// Reads one multipart file, stopping (413) as soon as it's over `limit`.
pub async fn read_field_limited(
    mut field: axum::extract::multipart::Field<'_>,
    filename: &str,
    limit: usize,
    dm: bool,
) -> AppResult<axum::body::Bytes> {
    let mut buf = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| AppError::BadRequest(format!("Upload failed: {e}")))?
    {
        if buf.len() + chunk.len() > limit {
            return Err(too_big(filename, limit, dm));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.into())
}

/// The longest message this person may send (characters).
pub async fn message_chars(db: &PgPool, user_id: i64) -> AppResult<usize> {
    let s = switches();
    if s.premium_chars > s.free_chars && user_has_premium(db, user_id).await? {
        Ok(s.premium_chars)
    } else {
        Ok(s.free_chars)
    }
}

// ---- App themes --------------------------------------------------------------------------

/// (key, name, free for everyone).
pub const THEMES: &[(&str, &str, bool)] = &[
    ("ember", "Ember", true),
    ("contrast", "High contrast", true),
    ("midnight", "Midnight blue", false),
    ("forest", "Forest", false),
    ("royal", "Royal purple", false),
    ("throne", "Gold throne", false),
];

pub fn theme_is_free(key: &str) -> bool {
    THEMES.iter().any(|(k, _, free)| *k == key && *free)
}

/// The theme someone sees: theirs, or Ember once premium lapses.
pub async fn effective_theme(db: &PgPool, user_id: i64, theme: &str) -> AppResult<String> {
    if theme_is_free(theme) || user_has_premium(db, user_id).await? {
        Ok(theme.to_string())
    } else {
        Ok("ember".into())
    }
}

// ---- Status JSON ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct EntRow {
    id: i64,
    source: String,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
    note: String,
    sponsor: Option<String>,
}

/// When an entitlement set runs out (None = forever, or nothing active).
fn until(rows: &[EntRow]) -> (bool, Option<DateTime<Utc>>) {
    if rows.is_empty() {
        return (false, None);
    }
    if rows.iter().any(|r| r.ends_at.is_none()) {
        return (true, None);
    }
    (true, rows.iter().filter_map(|r| r.ends_at).max())
}

fn ent_json(r: &EntRow) -> Value {
    json!({
        "id": r.id.to_string(),
        "source": r.source,
        "starts_at": r.starts_at,
        "ends_at": r.ends_at,
        "note": r.note,
        "sponsor": r.sponsor,
    })
}

const ACTIVE: &str =
    "revoked_at IS NULL AND starts_at <= now() AND (ends_at IS NULL OR ends_at > now())";

async fn user_entitlements(db: &PgPool, user_id: i64) -> AppResult<Vec<EntRow>> {
    Ok(sqlx::query_as::<_, EntRow>(&format!(
        "SELECT id, source, starts_at, ends_at, note, NULL::TEXT AS sponsor
         FROM user_premium WHERE user_id = $1 AND {ACTIVE}
         ORDER BY starts_at"
    ))
    .bind(user_id)
    .fetch_all(db)
    .await?)
}

const SLOT_COOLDOWN_DAYS: i64 = 7;

#[derive(sqlx::FromRow)]
struct SlotRow {
    guild_id: Option<i64>,
    guild_name: Option<String>,
    applied_at: DateTime<Utc>,
}

async fn slot_of(db: &PgPool, user_id: i64) -> AppResult<Option<SlotRow>> {
    Ok(sqlx::query_as::<_, SlotRow>(
        "SELECT s.guild_id, g.name AS guild_name, s.applied_at
         FROM premium_slots s LEFT JOIN guilds g ON g.id = s.guild_id
         WHERE s.user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?)
}

fn perks_json() -> Value {
    let s = switches();
    let c = config();
    json!({
        "upload_mb": mb(c.upload_bytes),
        "free_upload_mb": mb(FREE_UPLOAD_BYTES),
        "message_chars": s.free_chars,
        "premium_message_chars": s.premium_chars,
        "personal_sounds": c.personal_sounds,
        "emoji_premium": s.emoji_premium,
        "animated_premium": s.animated_premium,
        "themes": THEMES.iter().map(|(k, n, free)| json!({"key": k, "name": n, "free": free})).collect::<Vec<_>>(),
    })
}

/// `GET /api/premium/perks`: what premium includes on this server.
pub async fn perks(AuthUser(_user): AuthUser) -> Json<Value> {
    Json(perks_json())
}

/// `GET /api/users/@me/premium`: your Golden Throne status and slot.
pub async fn my_status(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    let rows = user_entitlements(&st.db, user.id).await?;
    let (active, ends) = until(&rows);
    let slot = slot_of(&st.db, user.id).await?;
    let slot_json = slot.map(|s| {
        let can_move_at = s.applied_at + chrono::Duration::days(SLOT_COOLDOWN_DAYS);
        json!({
            "guild_id": s.guild_id.map(|g| g.to_string()),
            "guild_name": s.guild_name,
            "applied_at": s.applied_at,
            "can_move_at": (can_move_at > Utc::now()).then_some(can_move_at),
        })
    });
    let (theme, gradient): (String, Option<Vec<i32>>) =
        sqlx::query_as("SELECT app_theme, profile_gradient FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&st.db)
            .await?;
    Ok(Json(json!({
        "active": active,
        "until": ends,
        "forever": active && ends.is_none(),
        "entitlements": rows.iter().map(ent_json).collect::<Vec<_>>(),
        "slot": slot_json,
        "theme": theme,
        "theme_in_use": effective_theme(&st.db, user.id, &theme).await?,
        "profile_gradient": gradient,
        "perks": perks_json(),
    })))
}

async fn guild_entitlements(db: &PgPool, guild_id: i64) -> AppResult<Vec<EntRow>> {
    Ok(sqlx::query_as::<_, EntRow>(
        "SELECT p.id, p.source, p.starts_at, p.ends_at, p.note, u.display_name AS sponsor
         FROM outhouse_premium p LEFT JOIN users u ON u.id = p.sponsor_user_id
         WHERE p.guild_id = $1 AND p.revoked_at IS NULL AND p.starts_at <= now()
           AND (p.ends_at IS NULL OR p.ends_at > now())
         UNION ALL
         SELECT 0::BIGINT AS id, 'slot' AS source, s.applied_at AS starts_at,
                (SELECT CASE WHEN bool_or(x.ends_at IS NULL) THEN NULL ELSE max(x.ends_at) END
                   FROM user_premium x
                   WHERE x.user_id = s.user_id AND x.revoked_at IS NULL AND x.starts_at <= now()
                     AND (x.ends_at IS NULL OR x.ends_at > now())) AS ends_at,
                '' AS note, u.display_name AS sponsor
         FROM premium_slots s
         JOIN users u ON u.id = s.user_id
         JOIN guild_members m ON m.guild_id = s.guild_id AND m.user_id = s.user_id
         WHERE s.guild_id = $1 AND oh_user_premium(s.user_id)
         ORDER BY starts_at",
    )
    .bind(guild_id)
    .fetch_all(db)
    .await?)
}

#[derive(sqlx::FromRow)]
struct VanityRow {
    vanity_code: Option<String>,
    vanity_uses: i32,
    vanity_paused_at: Option<DateTime<Utc>>,
    allow_personal_sounds: bool,
}

pub const VANITY_HOLD_DAYS: i64 = 30;

/// `GET /api/guilds/{id}/premium`: the outhouse's Golden Outhouse status
/// (any member), vanity link and premium settings.
pub async fn guild_status(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let manage = ctx.guild_perms() & p::MANAGE_GUILD == p::MANAGE_GUILD;
    let rows = guild_entitlements(&st.db, guild_id).await?;
    let (active, ends) = until(&rows);
    let v = sqlx::query_as::<_, VanityRow>(
        "SELECT vanity_code, vanity_uses, vanity_paused_at, allow_personal_sounds
         FROM guilds WHERE id = $1",
    )
    .bind(guild_id)
    .fetch_one(&st.db)
    .await?;
    let base = super::invites::public_base(&st, &headers);
    let my_slot = slot_of(&st.db, user.id).await?;
    Ok(Json(json!({
        "active": active,
        "until": ends,
        "forever": active && ends.is_none(),
        "sources": rows.iter().map(ent_json).collect::<Vec<_>>(),
        "vanity": v.vanity_code.as_ref().map(|code| json!({
            "code": code,
            "url": format!("{base}/invite/{code}"),
            "active": active,
            "uses": v.vanity_uses,
            "paused_at": v.vanity_paused_at,
            "release_at": v.vanity_paused_at.map(|t| t + chrono::Duration::days(VANITY_HOLD_DAYS)),
        })),
        "allow_personal_sounds": v.allow_personal_sounds,
        "can_manage": manage,
        "my_slot_here": my_slot.as_ref().is_some_and(|s| s.guild_id == Some(guild_id)),
        "perks": perks_json(),
    })))
}

// ---- The free Golden Outhouse slot --------------------------------------------------------

#[derive(Deserialize)]
pub struct SlotBody {
    #[serde(with = "outhouse_common::id::as_string")]
    guild_id: i64,
}

/// `PUT /api/users/@me/premium/slot {"guild_id"}`: put your free Golden
/// Outhouse slot on an outhouse you're in (movable once every 7 days).
pub async fn set_slot(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<SlotBody>,
) -> AppResult<Json<Value>> {
    if !user_has_premium(&st.db, user.id).await? {
        return Err(AppError::Refused(
            "The free Golden Outhouse slot comes with Golden Throne.".into(),
        ));
    }
    member_ctx(&st.db, body.guild_id, user.id).await?;
    let current = slot_of(&st.db, user.id).await?;
    if let Some(s) = &current {
        if s.guild_id == Some(body.guild_id) {
            return my_status(State(st), AuthUser(user)).await;
        }
        let next = s.applied_at + chrono::Duration::days(SLOT_COOLDOWN_DAYS);
        if next > Utc::now() {
            return Err(AppError::Conflict(format!(
                "You can move your Golden Outhouse slot once every {SLOT_COOLDOWN_DAYS} days. Next move: {}.",
                next.format("%-d %b %Y, %H:%M UTC")
            )));
        }
    }
    sqlx::query(
        "INSERT INTO premium_slots (user_id, guild_id, applied_at) VALUES ($1, $2, now())
         ON CONFLICT (user_id) DO UPDATE SET guild_id = $2, applied_at = now()",
    )
    .bind(user.id)
    .bind(body.guild_id)
    .execute(&st.db)
    .await?;
    forget_all();
    let old = current.and_then(|s| s.guild_id);
    if let Some(old) = old {
        guild_changed(&st, old, None).await;
    }
    audit::Entry::new(body.guild_id, user.id, "premium_slot_add")
        .details("Put their free Golden Outhouse slot on this outhouse")
        .save(&st)
        .await;
    guild_changed(&st, body.guild_id, None).await;
    my_status(State(st), AuthUser(user)).await
}

/// `DELETE /api/users/@me/premium/slot`: take your slot off (moving it to
/// another outhouse afterwards still waits for the 7 days).
pub async fn clear_slot(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    let old: Option<Option<i64>> = sqlx::query_scalar(
        "UPDATE premium_slots s SET guild_id = NULL
         FROM (SELECT user_id, guild_id FROM premium_slots WHERE user_id = $1) o
         WHERE s.user_id = o.user_id RETURNING o.guild_id",
    )
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    forget_all();
    if let Some(Some(g)) = old {
        audit::Entry::new(g, user.id, "premium_slot_remove")
            .details("Took their free Golden Outhouse slot off this outhouse")
            .save(&st)
            .await;
        guild_changed(&st, g, None).await;
    }
    my_status(State(st), AuthUser(user)).await
}

// ---- Telling clients ----------------------------------------------------------------------

/// An outhouse's premium may have changed: tell its members (rail badge,
/// limits) and pause or resume its vanity link.
pub async fn guild_changed(st: &AppState, guild_id: i64, actor: Option<i64>) {
    forget_all();
    let _ = sync_vanity(st, guild_id, actor).await;
    if let Ok(g) = super::guilds::fetch_guild(&st.db, guild_id).await {
        st.dispatch(Audience::Guild(guild_id), ServerEvent::GuildUpdate(g));
    }
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
}

/// A person's premium may have changed: their own clients (status, theme),
/// the outhouses they're in (badge, animated pictures) and any outhouse
/// their slot is on.
pub async fn user_changed(st: &AppState, user_id: i64) {
    forget_all();
    if let Ok(u) = crate::auth::fetch_user(&st.db, user_id).await {
        st.dispatch(Audience::User(user_id), ServerEvent::UserUpdate(u));
    }
    let _ = super::account::refresh_my_guilds(st, user_id).await;
    let slot: Option<Option<i64>> =
        sqlx::query_scalar("SELECT guild_id FROM premium_slots WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
    if let Some(Some(g)) = slot {
        guild_changed(st, g, None).await;
    }
}

// ---- Vanity invite links ------------------------------------------------------------------

/// Never vanity links (as well as the owner's blocked words).
const RESERVED_VANITY: &[&str] = &[
    "admin",
    "administrator",
    "outhouse",
    "outhouses",
    "staff",
    "support",
    "api",
    "app",
    "apps",
    "login",
    "logout",
    "signin",
    "signup",
    "register",
    "invite",
    "invites",
    "help",
    "official",
    "mod",
    "mods",
    "moderator",
    "system",
    "everyone",
    "here",
    "owner",
    "safety",
    "security",
    "police",
    "report",
    "reports",
    "settings",
    "premium",
    "golden",
    "throne",
    "golden-throne",
    "golden-outhouse",
    "billing",
    "stripe",
    "payment",
    "payments",
    "files",
    "emojis",
    "stickers",
    "evidence",
    "reset",
    "verify",
    "verify-email",
    "www",
    "root",
    "null",
    "undefined",
    "test",
    "discord",
    "nitro",
    "boost",
];

/// Lower-cases and checks the shape: 3–32 of a–z, 0–9 and hyphens, no
/// hyphen at either end.
pub fn clean_vanity(raw: &str) -> Result<String, &'static str> {
    let code = raw.trim().trim_start_matches('/').to_lowercase();
    if !(3..=32).contains(&code.len()) {
        return Err("Vanity links are 3 to 32 characters.");
    }
    if !code
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("Use only letters, numbers and hyphens.");
    }
    if code.starts_with('-') || code.ends_with('-') {
        return Err("Vanity links can't start or end with a hyphen.");
    }
    if code.contains("--") {
        return Err("Use one hyphen at a time.");
    }
    Ok(code)
}

pub fn reserved_vanity(code: &str) -> bool {
    let plain: String = code.chars().filter(|c| *c != '-').collect();
    RESERVED_VANITY
        .iter()
        .any(|r| *r == code || r.replace('-', "") == plain)
}

#[derive(Deserialize)]
pub struct VanityBody {
    code: String,
}

/// `PUT /api/guilds/{id}/vanity {"code"}`: set the outhouse's vanity invite
/// link (Manage Outhouse; Golden Outhouse only).
pub async fn set_vanity(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    headers: HeaderMap,
    Json(body): Json<VanityBody>,
) -> AppResult<Json<Value>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    if !guild_has_premium(&st.db, guild_id).await? {
        return Err(AppError::Refused(
            "Vanity invite links come with Golden Outhouse.".into(),
        ));
    }
    let code = clean_vanity(&body.code).map_err(|e| AppError::BadRequest(e.into()))?;
    if reserved_vanity(&code) {
        return Err(AppError::BadRequest(
            "That link is reserved. Try another.".into(),
        ));
    }
    if super::names::blocked_word_in(&st.db, &code)
        .await?
        .is_some()
    {
        return Err(AppError::BadRequest(
            "That link isn't allowed here. Try another.".into(),
        ));
    }
    let clash: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM invites WHERE lower(code) = $1)
             OR EXISTS(SELECT 1 FROM guilds WHERE vanity_code = $1 AND id <> $2)",
    )
    .bind(&code)
    .bind(guild_id)
    .fetch_one(&st.db)
    .await?;
    if clash {
        return Err(AppError::Conflict(
            "That link is taken. Try another.".into(),
        ));
    }
    let old: Option<String> = sqlx::query_scalar("SELECT vanity_code FROM guilds WHERE id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    let res = sqlx::query(
        "UPDATE guilds SET vanity_code = $2, vanity_set_at = now(), vanity_paused_at = NULL,
                vanity_uses = CASE WHEN vanity_code = $2 THEN vanity_uses ELSE 0 END
         WHERE id = $1",
    )
    .bind(guild_id)
    .bind(&code)
    .execute(&st.db)
    .await;
    if let Err(sqlx::Error::Database(e)) = &res {
        if e.is_unique_violation() {
            return Err(AppError::Conflict(
                "That link is taken. Try another.".into(),
            ));
        }
    }
    res?;
    if old.as_deref() != Some(code.as_str()) {
        audit::Entry::new(guild_id, user.id, "vanity_set")
            .details(match old {
                Some(o) => format!("Vanity invite link: /invite/{o} → /invite/{code}"),
                None => format!("Vanity invite link: /invite/{code}"),
            })
            .save(&st)
            .await;
    }
    guild_status(State(st), AuthUser(user), Path(guild_id), headers).await
}

/// `DELETE /api/guilds/{id}/vanity`: removes it (the name is free again).
pub async fn clear_vanity(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let old: Option<String> = sqlx::query_scalar(
        "UPDATE guilds g SET vanity_code = NULL, vanity_paused_at = NULL, vanity_set_at = NULL
         FROM (SELECT id, vanity_code FROM guilds WHERE id = $1) o
         WHERE g.id = o.id RETURNING o.vanity_code",
    )
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await?
    .flatten();
    if let Some(o) = old {
        audit::Entry::new(guild_id, user.id, "vanity_clear")
            .details(format!("Removed the vanity invite link /invite/{o}"))
            .save(&st)
            .await;
    }
    guild_status(State(st), AuthUser(user), Path(guild_id), headers).await
}

/// Pauses a lapsed outhouse's vanity link (held 30 days), resumes it when
/// premium is back, and releases it after the hold.
async fn sync_vanity(st: &AppState, guild_id: i64, actor: Option<i64>) -> AppResult<()> {
    let row: Option<(Option<String>, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT vanity_code, vanity_paused_at FROM guilds WHERE id = $1")
            .bind(guild_id)
            .fetch_optional(&st.db)
            .await?;
    let Some((Some(code), paused)) = row else {
        return Ok(());
    };
    let premium = guild_has_premium(&st.db, guild_id).await?;
    let entry = |action: &'static str| {
        let mut e = match actor {
            Some(a) => audit::Entry::new(guild_id, a, action),
            None => audit::Entry::system(guild_id, action),
        };
        e = e.target("guild", guild_id, format!("/invite/{code}"));
        e
    };
    match (premium, paused) {
        (true, Some(_)) => {
            sqlx::query("UPDATE guilds SET vanity_paused_at = NULL WHERE id = $1")
                .bind(guild_id)
                .execute(&st.db)
                .await?;
            entry("vanity_resumed")
                .details(format!(
                    "Golden Outhouse is back: /invite/{code} works again"
                ))
                .save(st)
                .await;
        }
        (false, None) => {
            sqlx::query("UPDATE guilds SET vanity_paused_at = now() WHERE id = $1")
                .bind(guild_id)
                .execute(&st.db)
                .await?;
            entry("vanity_paused")
                .details(format!(
                    "Golden Outhouse ended: /invite/{code} is paused and held for this outhouse for {VANITY_HOLD_DAYS} days"
                ))
                .save(st)
                .await;
        }
        (false, Some(at)) if at + chrono::Duration::days(VANITY_HOLD_DAYS) <= Utc::now() => {
            sqlx::query(
                "UPDATE guilds SET vanity_code = NULL, vanity_paused_at = NULL, vanity_set_at = NULL
                 WHERE id = $1",
            )
            .bind(guild_id)
            .execute(&st.db)
            .await?;
            entry("vanity_released")
                .details(format!(
                    "/invite/{code} was released after {VANITY_HOLD_DAYS} days without Golden Outhouse"
                ))
                .save(st)
                .await;
        }
        _ => {}
    }
    Ok(())
}

// ---- Outhouse premium settings ------------------------------------------------------------

#[derive(Deserialize)]
pub struct GuildSettingsBody {
    allow_personal_sounds: Option<bool>,
}

/// `PUT /api/guilds/{id}/premium-settings {"allow_personal_sounds"}`
/// (Manage Outhouse; free).
pub async fn set_guild_settings(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    headers: HeaderMap,
    Json(body): Json<GuildSettingsBody>,
) -> AppResult<Json<Value>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    if let Some(allow) = body.allow_personal_sounds {
        sqlx::query("UPDATE guilds SET allow_personal_sounds = $2 WHERE id = $1")
            .bind(guild_id)
            .bind(allow)
            .execute(&st.db)
            .await?;
        audit::Entry::new(guild_id, user.id, "guild_update")
            .details(if allow {
                "Allowed personal soundboard sounds"
            } else {
                "Turned off personal soundboard sounds"
            })
            .save(&st)
            .await;
        st.dispatch(
            Audience::Guild(guild_id),
            ServerEvent::SoundsChanged { guild_id },
        );
    }
    guild_status(State(st), AuthUser(user), Path(guild_id), headers).await
}

pub async fn personal_sounds_allowed(db: &PgPool, guild_id: i64) -> AppResult<bool> {
    if guild_id == DM_GUILD {
        return Ok(true);
    }
    Ok(
        sqlx::query_scalar("SELECT allow_personal_sounds FROM guilds WHERE id = $1")
            .bind(guild_id)
            .fetch_optional(db)
            .await?
            .unwrap_or(true),
    )
}

// ---- Themes and profile colours -----------------------------------------------------------

#[derive(Deserialize)]
pub struct ThemeBody {
    theme: String,
}

/// `PUT /api/users/@me/theme {"theme"}`: Ember and High contrast are free.
pub async fn set_theme(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<ThemeBody>,
) -> AppResult<Json<Value>> {
    if !THEMES.iter().any(|(k, _, _)| *k == body.theme) {
        return Err(AppError::BadRequest(
            "There's no theme by that name.".into(),
        ));
    }
    if !theme_is_free(&body.theme) && !user_has_premium(&st.db, user.id).await? {
        return Err(AppError::Refused(
            "That theme comes with Golden Throne.".into(),
        ));
    }
    sqlx::query("UPDATE users SET app_theme = $2 WHERE id = $1")
        .bind(user.id)
        .bind(&body.theme)
        .execute(&st.db)
        .await?;
    user_changed(&st, user.id).await;
    my_status(State(st), AuthUser(user)).await
}

#[derive(Deserialize)]
pub struct GradientBody {
    /// Two colours (0xRRGGBB), or null to go back to the plain look.
    colors: Option<Vec<i32>>,
}

/// `PUT /api/users/@me/profile-gradient {"colors": [a, b] | null}`.
pub async fn set_gradient(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<GradientBody>,
) -> AppResult<Json<Value>> {
    if let Some(c) = &body.colors {
        if c.len() != 2 || c.iter().any(|x| !(0..=0xFFFFFF).contains(x)) {
            return Err(AppError::BadRequest("Pick two colours.".into()));
        }
        if !user_has_premium(&st.db, user.id).await? {
            return Err(AppError::Refused(
                "Profile colours come with Golden Throne.".into(),
            ));
        }
    }
    sqlx::query("UPDATE users SET profile_gradient = $2 WHERE id = $1")
        .bind(user.id)
        .bind(&body.colors)
        .execute(&st.db)
        .await?;
    user_changed(&st, user.id).await;
    my_status(State(st), AuthUser(user)).await
}

// ---- The site owner's tools ----------------------------------------------------------------

#[derive(Deserialize)]
pub struct GrantBody {
    /// "month", "year", "forever" or "custom" (with `ends_at`).
    duration: String,
    ends_at: Option<DateTime<Utc>>,
    #[serde(default)]
    note: String,
}

fn grant_end(body: &GrantBody) -> AppResult<Option<DateTime<Utc>>> {
    let now = Utc::now();
    Ok(match body.duration.as_str() {
        "month" => Some(now + chrono::Duration::days(30)),
        "year" => Some(now + chrono::Duration::days(365)),
        "forever" => None,
        "custom" => {
            let at = body
                .ends_at
                .ok_or_else(|| AppError::BadRequest("Pick an end date.".into()))?;
            if at <= now {
                return Err(AppError::BadRequest(
                    "The end date has to be in the future.".into(),
                ));
            }
            Some(at)
        }
        _ => return Err(AppError::BadRequest("Pick how long.".into())),
    })
}

fn until_text(end: Option<DateTime<Utc>>) -> String {
    match end {
        Some(at) => format!("until {}", at.format("%-d %b %Y")),
        None => "forever".into(),
    }
}

/// `POST /api/safety/users/{id}/premium`: grant Golden Throne.
pub async fn grant_user(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(user_id): Path<i64>,
    Json(body): Json<GrantBody>,
) -> AppResult<Json<Value>> {
    a.require("grant_premium")?;
    let (name, bot): (String, bool) =
        sqlx::query_as("SELECT username, is_bot FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&st.db)
            .await?
            .ok_or(AppError::NotFound)?;
    if bot {
        return Err(AppError::BadRequest(
            "Bots can't have Golden Throne.".into(),
        ));
    }
    let end = grant_end(&body)?;
    let note: String = body.note.trim().chars().take(200).collect();
    sqlx::query(
        "INSERT INTO user_premium (id, user_id, source, ends_at, granted_by, note)
         VALUES ($1, $2, 'grant', $3, $4, $5)",
    )
    .bind(st.ids.next_id())
    .bind(user_id)
    .bind(end)
    .bind(a.id())
    .bind(&note)
    .execute(&st.db)
    .await?;
    forget_all();
    super::safety::log_action(
        &st,
        Some(a.id()),
        "premium_grant",
        None,
        Some(user_id),
        format!(
            "Granted Golden Throne to @{name} {}{}",
            until_text(end),
            if note.is_empty() {
                String::new()
            } else {
                format!(" ({note})")
            }
        ),
    )
    .await;
    user_changed(&st, user_id).await;
    Ok(Json(
        json!({ "ok": true, "active": user_has_premium(&st.db, user_id).await? }),
    ))
}

#[derive(Deserialize, Default)]
pub struct RevokeBody {
    /// One entitlement; missing = every active grant.
    id: Option<String>,
}

/// `POST /api/safety/users/{id}/premium/revoke`
pub async fn revoke_user(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(user_id): Path<i64>,
    body: Option<Json<RevokeBody>>,
) -> AppResult<Json<Value>> {
    a.require("grant_premium")?;
    let only: Option<i64> = body.and_then(|b| b.0.id).and_then(|s| s.parse().ok());
    let n = sqlx::query(&format!(
        "UPDATE user_premium SET revoked_at = now(), revoked_by = $2, ended_logged = true
         WHERE user_id = $1 AND source = 'grant' AND {ACTIVE} AND ($3::BIGINT IS NULL OR id = $3)"
    ))
    .bind(user_id)
    .bind(a.id())
    .bind(only)
    .execute(&st.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::BadRequest("Nothing to revoke.".into()));
    }
    forget_all();
    let name: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    super::safety::log_action(
        &st,
        Some(a.id()),
        "premium_revoke",
        None,
        Some(user_id),
        format!("Revoked Golden Throne from @{name}"),
    )
    .await;
    user_changed(&st, user_id).await;
    Ok(Json(
        json!({ "ok": true, "active": user_has_premium(&st.db, user_id).await? }),
    ))
}

/// `POST /api/safety/outhouses/{id}/premium`: grant Golden Outhouse.
pub async fn grant_guild(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(guild_id): Path<i64>,
    Json(body): Json<GrantBody>,
) -> AppResult<Json<Value>> {
    a.require("grant_premium")?;
    let name: String = sqlx::query_scalar("SELECT name FROM guilds WHERE id = $1")
        .bind(guild_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let end = grant_end(&body)?;
    let note: String = body.note.trim().chars().take(200).collect();
    sqlx::query(
        "INSERT INTO outhouse_premium (id, guild_id, source, ends_at, granted_by, note)
         VALUES ($1, $2, 'grant', $3, $4, $5)",
    )
    .bind(st.ids.next_id())
    .bind(guild_id)
    .bind(end)
    .bind(a.id())
    .bind(&note)
    .execute(&st.db)
    .await?;
    forget_all();
    super::safety::log_for_guild(
        &st,
        Some(a.id()),
        "premium_grant",
        guild_id,
        None,
        format!(
            "Granted Golden Outhouse to {name} {}{}",
            until_text(end),
            if note.is_empty() {
                String::new()
            } else {
                format!(" ({note})")
            }
        ),
    )
    .await;
    audit::Entry::system(guild_id, "premium_on")
        .details(format!("Golden Outhouse {}", until_text(end)))
        .save(&st)
        .await;
    guild_changed(&st, guild_id, None).await;
    Ok(Json(
        json!({ "ok": true, "active": guild_has_premium(&st.db, guild_id).await? }),
    ))
}

/// `POST /api/safety/outhouses/{id}/premium/revoke`
pub async fn revoke_guild(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(guild_id): Path<i64>,
    body: Option<Json<RevokeBody>>,
) -> AppResult<Json<Value>> {
    a.require("grant_premium")?;
    let only: Option<i64> = body.and_then(|b| b.0.id).and_then(|s| s.parse().ok());
    let n = sqlx::query(&format!(
        "UPDATE outhouse_premium SET revoked_at = now(), revoked_by = $2, ended_logged = true
         WHERE guild_id = $1 AND source = 'grant' AND {ACTIVE} AND ($3::BIGINT IS NULL OR id = $3)"
    ))
    .bind(guild_id)
    .bind(a.id())
    .bind(only)
    .execute(&st.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::BadRequest("Nothing to revoke.".into()));
    }
    forget_all();
    let name: String = sqlx::query_scalar("SELECT name FROM guilds WHERE id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    super::safety::log_for_guild(
        &st,
        Some(a.id()),
        "premium_revoke",
        guild_id,
        None,
        format!("Revoked Golden Outhouse from {name}"),
    )
    .await;
    if !guild_has_premium(&st.db, guild_id).await? {
        audit::Entry::system(guild_id, "premium_off")
            .details("Golden Outhouse ended")
            .save(&st)
            .await;
    }
    guild_changed(&st, guild_id, None).await;
    Ok(Json(
        json!({ "ok": true, "active": guild_has_premium(&st.db, guild_id).await? }),
    ))
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// "people", "outhouses" or "all" (default)
    kind: Option<String>,
    /// "active" (default), "ended" or "all"
    status: Option<String>,
    q: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ListRow {
    kind: String,
    id: i64,
    target_id: i64,
    target_name: String,
    source: String,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    note: String,
    granted_by: Option<String>,
    sponsor: Option<String>,
    active: bool,
}

/// `GET /api/safety/premium?kind=&status=&q=`: entitlements (SITE OWNER →
/// Premium), newest first, plus the owner's switches.
pub async fn list(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    a.require("grant_premium")?;
    let kind = q.kind.unwrap_or_else(|| "all".into());
    let status = q.status.unwrap_or_else(|| "active".into());
    let search =
        q.q.map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
    let rows = sqlx::query_as::<_, ListRow>(
        "SELECT * FROM (
            SELECT 'user' AS kind, p.id, p.user_id AS target_id, u.username AS target_name,
                   p.source, p.starts_at, p.ends_at, p.revoked_at, p.note,
                   gb.username AS granted_by, NULL::TEXT AS sponsor,
                   (p.revoked_at IS NULL AND p.starts_at <= now() AND (p.ends_at IS NULL OR p.ends_at > now())) AS active
            FROM user_premium p JOIN users u ON u.id = p.user_id
            LEFT JOIN users gb ON gb.id = p.granted_by
            UNION ALL
            SELECT 'outhouse', p.id, p.guild_id, g.name, p.source, p.starts_at, p.ends_at,
                   p.revoked_at, p.note, gb.username, su.username,
                   (p.revoked_at IS NULL AND p.starts_at <= now() AND (p.ends_at IS NULL OR p.ends_at > now()))
            FROM outhouse_premium p JOIN guilds g ON g.id = p.guild_id
            LEFT JOIN users gb ON gb.id = p.granted_by
            LEFT JOIN users su ON su.id = p.sponsor_user_id
            UNION ALL
            SELECT 'outhouse', 0, s.guild_id, g.name, 'slot', s.applied_at, NULL, NULL, '',
                   NULL, u.username, oh_user_premium(s.user_id)
            FROM premium_slots s JOIN guilds g ON g.id = s.guild_id JOIN users u ON u.id = s.user_id
         ) x
         WHERE ($1 = 'all' OR ($1 = 'people' AND kind = 'user') OR ($1 = 'outhouses' AND kind = 'outhouse'))
           AND ($2 = 'all' OR ($2 = 'active' AND active) OR ($2 = 'ended' AND NOT active))
           AND ($3::TEXT IS NULL OR strpos(lower(target_name), $3) > 0
                OR strpos(lower(COALESCE(sponsor, '')), $3) > 0)
         ORDER BY active DESC, starts_at DESC
         LIMIT 500",
    )
    .bind(&kind)
    .bind(&status)
    .bind(&search)
    .fetch_all(&st.db)
    .await?;
    let s = switches();
    Ok(Json(json!({
        "entitlements": rows.iter().map(|r| json!({
            "kind": r.kind,
            "id": r.id.to_string(),
            "target_id": r.target_id.to_string(),
            "target_name": r.target_name,
            "source": r.source,
            "starts_at": r.starts_at,
            "ends_at": r.ends_at,
            "revoked_at": r.revoked_at,
            "note": r.note,
            "granted_by": r.granted_by,
            "sponsor": r.sponsor,
            "active": r.active,
        })).collect::<Vec<_>>(),
        "settings": switches_json(&s),
        "owner": a.owner,
        "perks": perks_json(),
    })))
}

fn switches_json(s: &Switches) -> Value {
    let env = env_switches();
    json!({
        "message_chars": s.free_chars,
        "premium_message_chars": s.premium_chars,
        "emoji_premium": s.emoji_premium,
        "animated_premium": s.animated_premium,
        "defaults": {
            "message_chars": env.free_chars,
            "premium_message_chars": env.premium_chars,
            "emoji_premium": env.emoji_premium,
            "animated_premium": env.animated_premium,
        },
    })
}

#[derive(Deserialize)]
pub struct SwitchBody {
    message_chars: Option<i32>,
    premium_message_chars: Option<i32>,
    emoji_premium: Option<bool>,
    animated_premium: Option<bool>,
    /// Go back to the environment's defaults.
    #[serde(default)]
    reset: bool,
}

/// `PUT /api/safety/premium-settings`: the owner's switches (site owner only).
pub async fn set_switches(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Json(body): Json<SwitchBody>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let before = switches();
    for n in [body.message_chars, body.premium_message_chars]
        .into_iter()
        .flatten()
    {
        if !(100..=20_000).contains(&n) {
            return Err(AppError::BadRequest(
                "Message limits are 100 to 20,000 characters.".into(),
            ));
        }
    }
    if body.reset {
        sqlx::query(
            "UPDATE premium_settings SET free_message_chars = NULL, premium_message_chars = NULL,
                    emoji_premium = NULL, animated_premium = NULL, updated_by = $1, updated_at = now()
             WHERE id = 1",
        )
        .bind(a.id())
        .execute(&st.db)
        .await?;
    } else {
        sqlx::query(
            "UPDATE premium_settings SET
                free_message_chars = COALESCE($1, free_message_chars),
                premium_message_chars = COALESCE($2, premium_message_chars),
                emoji_premium = COALESCE($3, emoji_premium),
                animated_premium = COALESCE($4, animated_premium),
                updated_by = $5, updated_at = now()
             WHERE id = 1",
        )
        .bind(body.message_chars)
        .bind(body.premium_message_chars)
        .bind(body.emoji_premium)
        .bind(body.animated_premium)
        .bind(a.id())
        .execute(&st.db)
        .await?;
    }
    let s = load_switches(&st.db).await?;
    let word = |b: bool| if b { "premium" } else { "free for everyone" };
    super::safety::log_action(
        &st,
        Some(a.id()),
        "premium_settings",
        None,
        None,
        format!(
            "Premium switches: messages {} / {} characters, other outhouses' emoji {}, animated pictures {} (was {} / {}, {}, {})",
            s.free_chars,
            s.premium_chars,
            word(s.emoji_premium),
            word(s.animated_premium),
            before.free_chars,
            before.premium_chars,
            word(before.emoji_premium),
            word(before.animated_premium)
        ),
    )
    .await;
    if s.animated_premium != before.animated_premium {
        // Every picture link may have changed.
        let guilds: Vec<i64> = sqlx::query_scalar("SELECT id FROM guilds")
            .fetch_all(&st.db)
            .await?;
        for g in guilds {
            if let Ok(guild) = super::guilds::fetch_guild(&st.db, g).await {
                st.dispatch(Audience::Guild(g), ServerEvent::GuildUpdate(guild));
            }
            st.dispatch(
                Audience::Guild(g),
                ServerEvent::GuildRefresh { guild_id: g },
            );
        }
    }
    Ok(Json(switches_json(&s)))
}

// ---- Expiry ---------------------------------------------------------------------------------

/// Ends entitlements whose time is up (logged once), tidies slots whose
/// owner left the outhouse or lost Golden Throne, and pauses or releases
/// vanity links. Returns how many entitlements ended.
pub async fn sweep(st: &AppState) -> AppResult<u64> {
    let users: Vec<(i64, String)> = sqlx::query_as(
        "UPDATE user_premium p SET ended_logged = true
         FROM users u
         WHERE u.id = p.user_id AND NOT p.ended_logged AND p.revoked_at IS NULL
           AND p.ends_at IS NOT NULL AND p.ends_at <= now()
         RETURNING p.user_id, u.username",
    )
    .fetch_all(&st.db)
    .await?;
    let guilds: Vec<(i64, String)> = sqlx::query_as(
        "UPDATE outhouse_premium p SET ended_logged = true
         FROM guilds g
         WHERE g.id = p.guild_id AND NOT p.ended_logged AND p.revoked_at IS NULL
           AND p.ends_at IS NOT NULL AND p.ends_at <= now()
         RETURNING p.guild_id, g.name",
    )
    .fetch_all(&st.db)
    .await?;
    let ended = (users.len() + guilds.len()) as u64;
    if ended > 0 {
        forget_all();
    }
    for (uid, name) in &users {
        let still = user_has_premium(&st.db, *uid).await?;
        super::safety::log_action(
            st,
            None,
            "premium_expired",
            None,
            Some(*uid),
            format!(
                "A Golden Throne grant for @{name} ran out{}",
                if still {
                    " (they still have another)"
                } else {
                    ""
                }
            ),
        )
        .await;
        if !still {
            // Their slot goes (it's empty until they have Golden Throne again).
            let slot: Option<Option<i64>> = sqlx::query_scalar(
                "UPDATE premium_slots s SET guild_id = NULL
                 FROM (SELECT user_id, guild_id FROM premium_slots WHERE user_id = $1) o
                 WHERE s.user_id = o.user_id RETURNING o.guild_id",
            )
            .bind(uid)
            .fetch_optional(&st.db)
            .await?;
            forget_all();
            if let Some(Some(g)) = slot {
                guild_changed(st, g, None).await;
            }
        }
        user_changed(st, *uid).await;
    }
    for (gid, name) in &guilds {
        let still = guild_has_premium(&st.db, *gid).await?;
        super::safety::log_for_guild(
            st,
            None,
            "premium_expired",
            *gid,
            None,
            format!(
                "A Golden Outhouse grant for {name} ran out{}",
                if still {
                    " (it's still premium another way)"
                } else {
                    ""
                }
            ),
        )
        .await;
        if !still {
            audit::Entry::system(*gid, "premium_off")
                .details("Golden Outhouse ended")
                .save(st)
                .await;
        }
        guild_changed(st, *gid, None).await;
    }
    // Slots whose owner left the outhouse, or lost Golden Throne some other way.
    let stale: Vec<i64> = sqlx::query_scalar(
        "UPDATE premium_slots s SET guild_id = NULL
         FROM (SELECT user_id, guild_id FROM premium_slots
               WHERE guild_id IS NOT NULL
                 AND (NOT oh_user_premium(user_id)
                      OR NOT EXISTS (SELECT 1 FROM guild_members m
                                     WHERE m.guild_id = premium_slots.guild_id
                                       AND m.user_id = premium_slots.user_id))) o
         WHERE s.user_id = o.user_id RETURNING o.guild_id",
    )
    .fetch_all(&st.db)
    .await?;
    for g in stale {
        guild_changed(st, g, None).await;
    }
    // Vanity links: pause lapsed ones, resume renewed ones, release old holds.
    let vanity: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM guilds WHERE vanity_code IS NOT NULL")
            .fetch_all(&st.db)
            .await?;
    for g in vanity {
        let paused_before: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT vanity_paused_at FROM guilds WHERE id = $1")
                .bind(g)
                .fetch_one(&st.db)
                .await?;
        sync_vanity(st, g, None).await?;
        let paused_after: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT vanity_paused_at FROM guilds WHERE id = $1")
                .bind(g)
                .fetch_optional(&st.db)
                .await?
                .flatten();
        if paused_before.is_some() != paused_after.is_some() {
            if let Ok(guild) = super::guilds::fetch_guild(&st.db, g).await {
                st.dispatch(Audience::Guild(g), ServerEvent::GuildUpdate(guild));
            }
        }
    }
    Ok(ended)
}

pub fn spawn_sweeper(st: AppState) {
    tokio::spawn(async move {
        if let Err(e) = load_switches(&st.db).await {
            tracing::warn!(error = %e, "couldn't load the premium switches");
        }
        loop {
            if let Err(e) = sweep(&st).await {
                tracing::warn!(error = %e, "premium sweep failed");
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

/// `POST /api/premium/sweep`: runs the expiry sweep now (tests only, with
/// `OUTHOUSE_TEST_HOOKS=1`; the tests move the clock in SQL).
pub async fn sweep_now(
    State(st): State<AppState>,
    AuthUser(_user): AuthUser,
) -> AppResult<Json<Value>> {
    if std::env::var("OUTHOUSE_TEST_HOOKS").ok().as_deref() != Some("1") {
        return Err(AppError::NotFound);
    }
    forget_all();
    let ended = sweep(&st).await?;
    Ok(Json(json!({ "ended": ended })))
}

/// For the People page: a person's Golden Throne state.
pub async fn person_json(db: &PgPool, user_id: i64) -> AppResult<Value> {
    let rows = user_entitlements(db, user_id).await?;
    let (active, ends) = until(&rows);
    Ok(json!({
        "active": active,
        "until": ends,
        "forever": active && ends.is_none(),
        "entitlements": rows.iter().map(ent_json).collect::<Vec<_>>(),
    }))
}

/// For the Outhouses directory: an outhouse's Golden Outhouse state.
pub async fn outhouse_json(db: &PgPool, guild_id: i64) -> AppResult<Value> {
    let rows = guild_entitlements(db, guild_id).await?;
    let (active, ends) = until(&rows);
    let vanity: Option<String> = sqlx::query_scalar("SELECT vanity_code FROM guilds WHERE id = $1")
        .bind(guild_id)
        .fetch_optional(db)
        .await?
        .flatten();
    Ok(json!({
        "active": active,
        "until": ends,
        "forever": active && ends.is_none(),
        "sources": rows.iter().map(ent_json).collect::<Vec<_>>(),
        "vanity": vanity,
    }))
}

// ---- Animated pictures ------------------------------------------------------------------

/// Notes whether a just-uploaded avatar, banner or outhouse picture moves
/// (and which outhouse it belongs to), for `oh_file_url`.
pub async fn note_picture(db: &PgPool, key: &str, data: &[u8], guild_id: Option<i64>) {
    let animated = crate::media::is_animated(data);
    if let Err(e) = sqlx::query(
        "UPDATE profile_files SET animated = $2, guild_id = COALESCE($3, guild_id)
         WHERE storage_key = $1",
    )
    .bind(key)
    .bind(animated)
    .bind(guild_id)
    .execute(db)
    .await
    {
        tracing::warn!(error = %e, "couldn't note whether a picture is animated");
    }
}

/// Pictures uploaded before this check existed: look at each once.
pub fn spawn_animated_check(st: AppState) {
    tokio::spawn(async move {
        loop {
            let keys: Vec<String> = match sqlx::query_scalar(
                "SELECT storage_key FROM profile_files WHERE animated IS NULL LIMIT 200",
            )
            .fetch_all(&st.db)
            .await
            {
                Ok(k) => k,
                Err(e) => {
                    tracing::warn!(error = %e, "couldn't list pictures to check");
                    return;
                }
            };
            if keys.is_empty() {
                return;
            }
            for key in keys {
                let animated = match tokio::fs::read(st.config.upload_dir.join(&key)).await {
                    Ok(data) => crate::media::is_animated(&data),
                    Err(_) => false,
                };
                let _ =
                    sqlx::query("UPDATE profile_files SET animated = $2 WHERE storage_key = $1")
                        .bind(&key)
                        .bind(animated)
                        .execute(&st.db)
                        .await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
}

/// The caller's status + `StatusCode` helper for empty answers.
#[allow(dead_code)]
fn no_content() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanity_shapes() {
        assert_eq!(clean_vanity(" Raid-Night ").unwrap(), "raid-night");
        assert!(clean_vanity("ab").is_err());
        assert!(clean_vanity(&"a".repeat(33)).is_err());
        assert!(clean_vanity(&"a".repeat(32)).is_ok());
        assert!(clean_vanity("-raid").is_err());
        assert!(clean_vanity("raid-").is_err());
        assert!(clean_vanity("raid--night").is_err());
        assert!(clean_vanity("raid_night").is_err());
        assert!(clean_vanity("raid night").is_err());
        assert!(clean_vanity("ráid").is_err());
    }

    #[test]
    fn reserved_words() {
        assert!(reserved_vanity("admin"));
        assert!(reserved_vanity("outhouse"));
        assert!(reserved_vanity("golden-throne"));
        assert!(reserved_vanity("goldenthrone"));
        assert!(reserved_vanity("every-one"));
        assert!(!reserved_vanity("the-long-drop"));
    }

    #[test]
    fn themes() {
        assert!(theme_is_free("ember"));
        assert!(theme_is_free("contrast"));
        assert!(!theme_is_free("throne"));
        assert!(!theme_is_free("nope"));
    }
}
