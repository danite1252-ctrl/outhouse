//! The local image detector: uploaded pictures are scored for nudity by a
//! small model in a sidecar container (`DETECTOR_URL`, e.g.
//! `http://detector:8095`; see deploy/detector). Pictures never leave the
//! server, and nothing but the score and labels is stored.
//!
//! Scanning happens in the background, one picture at a time, after the
//! upload is saved: chat never waits for it. What a flagged picture does
//! depends on where it was posted:
//!
//! - an 18+ channel only adults can see: shown as normal;
//! - sent to or from a teen (DMs, group DMs, any channel a teen can see):
//!   held for everyone (teens never get it) and a detector report goes to
//!   the site owner;
//! - anywhere else: held for review (or only blurred, if the owner chose
//!   "blur only") with a detector report;
//! - avatars, banners, emoji and stickers: replaced by a placeholder until
//!   the owner approves them.
//!
//! While a picture to or from a teen is still being checked it's held as
//! "Checking…" (the per-outhouse "hold images for teens" switch). If the
//! sidecar is down, uploads still work and the queue waits (with backoff).
//!
//! Child-safety rules always win: quarantined files are never served, and a
//! picture held in a teen context is never shown to anyone, the site owner
//! included.
//!
//! The sidecar also finds faces and estimates each one's age (a preliminary
//! automated estimate, not evidence; only the numbers are kept). A picture
//! that's explicit (nudity score at or above the escalation score, default
//! 0.40) with a face estimated clearly young (the youngest at or under the
//! age threshold, default 13) is escalated to a child-safety report, in
//! every context (18+ channels too): the file is quarantined, the message
//! hidden and the owner alerted, exactly like a child-safety report someone
//! made. A young face without nudity does nothing. See [`AgeRule`].

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::HeaderValue;
use axum::response::Response;
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{AttachmentHold, Channel, User, DM_GUILD};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Json as SqlJson;
use sqlx::PgPool;
use tokio::sync::Notify;

use super::audit::is_site_owner;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

pub const KIND_ATTACHMENT: &str = "attachment";
pub const KIND_AVATAR: &str = "avatar";
pub const KIND_BANNER: &str = "banner";
pub const KIND_EMOJI: &str = "emoji";
pub const KIND_STICKER: &str = "sticker";
pub const KIND_GUILD_ICON: &str = "guild_icon";
pub const KIND_GUILD_BANNER: &str = "guild_banner";
/// Scanned only for a report's review screen (nothing is held).
const KIND_REPORT: &str = "report";

/// A threshold for everything, for checking the detector works in
/// production with harmless pictures (`DETECTOR_THRESHOLD=0.001` flags
/// nearly anything). Leave it unset normally.
fn threshold_override() -> Option<f32> {
    std::env::var("DETECTOR_THRESHOLD")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|t| t.is_finite() && (0.0..=1.0).contains(t))
}

/// Sensitivity → the score a picture needs to be flagged.
pub fn threshold(sensitivity: &str) -> f32 {
    if let Some(t) = threshold_override() {
        return t;
    }
    preset_threshold(sensitivity)
}

fn preset_threshold(sensitivity: &str) -> f32 {
    match sensitivity {
        "low" => 0.85,
        "high" => 0.40,
        _ => 0.60,
    }
}

/// Picture types the detector reads.
pub fn scannable(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

/// The sidecar's address (`DETECTOR_URL`), if it's set up.
pub fn service_url() -> Option<String> {
    std::env::var("DETECTOR_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
}

// ---- Age estimates and the escalation rule -----------------------------------------

/// How every age estimate is described, wherever it's shown or exported.
pub const AGE_NOTE: &str = "preliminary automated estimate, not evidence";
/// The age thresholds the owner can pick.
pub const AGE_THRESHOLDS: &[i32] = &[10, 13, 16];
/// Escalation scores offered in the settings (any 0.05–0.95 is accepted).
pub const ESCALATION_SCORES: &[f32] = &[0.25, 0.40, 0.60];

fn env_f32(name: &str, lo: f32, hi: f32) -> Option<f32> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|t| t.is_finite() && (lo..=hi).contains(t))
}

/// For checking the age rule in production with harmless pictures (no
/// explicit or child imagery): `DETECTOR_ESCALATION_AGE=99` counts any face
/// as young, `DETECTOR_ESCALATION_SCORE=0.001` counts nearly any picture as
/// explicit. Leave both unset normally.
pub fn escalation_age_override() -> Option<f32> {
    env_f32("DETECTOR_ESCALATION_AGE", 1.0, 120.0)
}

pub fn escalation_score_override() -> Option<f32> {
    env_f32("DETECTOR_ESCALATION_SCORE", 0.0, 1.0)
}

/// The rule that escalates a picture to child safety: explicit (nudity
/// score ≥ `score`) and a face estimated young (youngest ≤ `age`). Site-wide
/// (the site owner's setting); it applies in every context.
#[derive(Debug, Clone)]
pub struct AgeRule {
    pub enabled: bool,
    /// What's in effect (an override from the environment, or the setting).
    pub age: f32,
    pub score: f32,
    /// The owner's settings.
    pub age_setting: i32,
    pub score_setting: f32,
}

impl AgeRule {
    pub fn from_settings(enabled: bool, age: i32, score: f32) -> Self {
        Self {
            enabled,
            age: escalation_age_override().unwrap_or(age as f32),
            score: escalation_score_override().unwrap_or(score),
            age_setting: age,
            score_setting: score,
        }
    }

    pub fn matches(&self, score: Option<f32>, min_age: Option<f32>) -> bool {
        self.enabled
            && score.is_some_and(|s| s.is_finite() && s >= self.score)
            && min_age.is_some_and(|a| a.is_finite() && a <= self.age)
    }

    /// The report's preview line (no content).
    pub fn line(&self) -> String {
        format!(
            "Automatic: explicit content with a face estimated {} or under",
            fmt_age(self.age)
        )
    }
}

pub async fn age_rule(db: &PgPool) -> AgeRule {
    let row: Option<(bool, i32, f32)> = sqlx::query_as(
        "SELECT detector_age_check, detector_age_threshold, detector_escalation_score
         FROM site_settings WHERE id = 1",
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    let (enabled, age, score) = row.unwrap_or((true, 13, 0.40));
    AgeRule::from_settings(enabled, age, score)
}

fn fmt_age(age: f32) -> String {
    format!("{}", age.round() as i64)
}

/// The sidecar's per-face estimates, kept as numbers only (age, confidence),
/// youngest first, at most 10.
fn clean_ages(ages: &[Value]) -> Vec<Value> {
    let mut out: Vec<(f64, f64)> = ages
        .iter()
        .filter_map(|a| {
            let age = a["age"]
                .as_f64()
                .filter(|x| x.is_finite() && *x >= 0.0 && *x <= 150.0)?;
            let conf = a["confidence"]
                .as_f64()
                .filter(|x| x.is_finite())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            Some((age, conf))
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.truncate(10);
    out.into_iter()
        .map(|(age, conf)| {
            json!({
                "age": (age * 10.0).round() / 10.0,
                "confidence": (conf * 1000.0).round() / 1000.0,
            })
        })
        .collect()
}

/// "Detector: nudity 0.82 · faces 2 · youngest estimated ~9 (preliminary
/// automated estimate, not evidence)", or None if faces weren't checked.
pub fn age_line(score: Option<f32>, faces: Option<i32>, min_age: Option<f32>) -> Option<String> {
    let faces = faces?;
    let nudity = score
        .map(|s| format!("nudity {}", fmt_score(s)))
        .unwrap_or_else(|| "nudity not scored".into());
    Some(match (faces, min_age) {
        (0, _) | (_, None) => format!("Detector: {nudity} · no faces found"),
        (n, Some(age)) => format!(
            "Detector: {nudity} · face{} {n} · youngest estimated ~{} ({AGE_NOTE})",
            if n == 1 { "" } else { "s" },
            fmt_age(age)
        ),
    })
}

// ---- Health and the queue's wake-up ----------------------------------------------

#[derive(Default, Clone)]
struct Health {
    last_ok_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    last_error_at: Option<DateTime<Utc>>,
    /// The last call to the sidecar failed (it's down or restarting).
    offline: bool,
    model: Option<String>,
    scans: u64,
    last_ms: Option<i64>,
}

static HEALTH: Mutex<Option<Health>> = Mutex::new(None);
static WAKE: OnceLock<Notify> = OnceLock::new();

fn wake() {
    WAKE.get_or_init(Notify::new).notify_one();
}

fn health_update(f: impl FnOnce(&mut Health)) {
    let mut h = HEALTH.lock().unwrap_or_else(|e| e.into_inner());
    f(h.get_or_insert_with(Health::default));
}

fn health_now() -> Health {
    HEALTH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

// ---- Settings ----------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Settings {
    pub enabled: bool,
    pub sensitivity: String,
    pub action: String,
    pub hold_teens: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            sensitivity: "medium".into(),
            action: "hold".into(),
            hold_teens: true,
        }
    }
}

async fn site_settings(db: &PgPool) -> Settings {
    sqlx::query_as::<_, Settings>(
        "SELECT detector_enabled AS enabled, detector_sensitivity AS sensitivity,
                detector_action AS action, detector_hold_teens AS hold_teens
         FROM site_settings WHERE id = 1",
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// The settings for an outhouse (its own, falling back to the site's), or
/// the site's for DMs, profiles and anything outside an outhouse.
pub async fn settings_for(db: &PgPool, guild_id: Option<i64>) -> Settings {
    let site = site_settings(db).await;
    let Some(guild_id) = guild_id.filter(|g| *g != DM_GUILD) else {
        return site;
    };
    let row: Option<(bool, Option<String>, Option<String>, bool)> = sqlx::query_as(
        "SELECT detector_enabled, detector_sensitivity, detector_action, detector_hold_teens
         FROM guilds WHERE id = $1",
    )
    .bind(guild_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    match row {
        Some((enabled, sensitivity, action, hold_teens)) => Settings {
            enabled: site.enabled && enabled,
            sensitivity: sensitivity.unwrap_or(site.sensitivity),
            action: action.unwrap_or(site.action),
            hold_teens,
        },
        None => site,
    }
}

// ---- Who's involved ------------------------------------------------------------------

async fn is_teen(db: &PgPool, user_id: i64) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND age_bracket = 'teen' AND NOT is_bot)",
    )
    .bind(user_id)
    .fetch_one(db)
    .await
    // Can't tell: treat as a teen (pictures are held, not shown).
    .unwrap_or(true)
}

/// Can a teen see this channel (a DM with a teen in it, or a channel a teen
/// member can view and read)?
pub async fn teen_can_see(db: &PgPool, channel: &Channel) -> bool {
    if channel.guild_id == DM_GUILD {
        return sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM dm_participants p JOIN users u ON u.id = p.user_id
                           WHERE p.channel_id = $1 AND u.age_bracket = 'teen' AND NOT u.is_bot)",
        )
        .bind(channel.id)
        .fetch_one(db)
        .await
        .unwrap_or(true);
    }
    let teens: Vec<i64> = match sqlx::query_scalar(
        "SELECT u.id FROM guild_members m JOIN users u ON u.id = m.user_id
         WHERE m.guild_id = $1 AND u.age_bracket = 'teen' AND NOT u.is_bot",
    )
    .bind(channel.guild_id)
    .fetch_all(db)
    .await
    {
        Ok(t) => t,
        Err(_) => return true,
    };
    for teen in teens {
        if crate::perms::can_view(db, channel.guild_id, channel.id, teen).await
            && crate::perms::content_allowed(db, channel.id, teen).await
        {
            return true;
        }
    }
    false
}

async fn guild_has_teens(db: &PgPool, guild_id: i64) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM guild_members m JOIN users u ON u.id = m.user_id
                       WHERE m.guild_id = $1 AND u.age_bracket = 'teen' AND NOT u.is_bot)",
    )
    .bind(guild_id)
    .fetch_one(db)
    .await
    .unwrap_or(true)
}

// ---- Deciding what people see -----------------------------------------------------------

fn profile_kind(kind: &str) -> bool {
    matches!(
        kind,
        KIND_AVATAR | KIND_BANNER | KIND_EMOJI | KIND_STICKER | KIND_GUILD_ICON | KIND_GUILD_BANNER
    )
}

/// What people see of a picture, given its scan and where it was posted.
#[allow(clippy::too_many_arguments)]
pub fn decide(
    status: &str,
    kind: &str,
    teen: bool,
    adult_only: bool,
    review: Option<&str>,
    action: &str,
    hold_teens: bool,
) -> &'static str {
    if kind == KIND_REPORT {
        return "none";
    }
    let held = if profile_kind(kind) { "hidden" } else { "held" };
    match status {
        "pending" if teen && hold_teens => {
            if profile_kind(kind) {
                "hidden"
            } else {
                "checking"
            }
        }
        // Couldn't be checked: to or from a teen it stays held (fail closed).
        "error" if teen && review != Some("approved") => held,
        "flagged" => match review {
            Some("approved") => "none",
            _ if profile_kind(kind) => "hidden",
            _ if teen => "held",
            _ if adult_only => "none",
            Some("blurred") => "blur",
            _ if action == "blur" => "blur",
            _ => "held",
        },
        _ => "none",
    }
}

/// The holds on these message files (only the ones that aren't shown as normal).
pub async fn holds_for(db: &PgPool, keys: &[String]) -> AppResult<HashMap<String, AttachmentHold>> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT storage_key, hold, teen FROM file_scans
         WHERE storage_key = ANY($1) AND hold IN ('checking', 'held', 'blur', 'hidden')",
    )
    .bind(keys)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(key, hold, teen)| {
            let (state, reveal) = match hold.as_str() {
                "blur" => ("blur", "anyone"),
                "checking" => ("checking", "none"),
                // Held to or from a teen: nobody looks, the owner included.
                _ if teen => ("held", "none"),
                _ => ("held", "sender_owner"),
            };
            (
                key,
                AttachmentHold {
                    state: state.into(),
                    reveal: reveal.into(),
                },
            )
        })
        .collect())
}

/// What `/files`, `/emojis` and `/stickers` may do with a stored file.
pub enum Gate {
    /// Serve it; `settled` = scanned (or never needs scanning), so it can be
    /// cached for good.
    Open { settled: bool },
    /// A message file held for review or still being checked: refused.
    Held,
    /// An avatar, banner, emoji or sticker held back: a placeholder instead.
    Placeholder,
}

pub async fn gate(db: &PgPool, key: &str) -> Gate {
    let row: Result<Option<(String, String, String)>, _> =
        sqlx::query_as("SELECT kind, status, hold FROM file_scans WHERE storage_key = $1")
            .bind(key)
            .fetch_optional(db)
            .await;
    match row {
        Ok(None) => Gate::Open { settled: true },
        Ok(Some((kind, status, hold))) => match hold.as_str() {
            "checking" | "held" | "hidden" if profile_kind(&kind) => Gate::Placeholder,
            "checking" | "held" | "hidden" => Gate::Held,
            _ => Gate::Open {
                settled: status != "pending",
            },
        },
        // The database didn't answer: don't hand out anything we can't check.
        Err(_) => Gate::Held,
    }
}

/// Shown instead of an avatar, banner, emoji or sticker held for review.
pub const PLACEHOLDER_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64"><rect width="64" height="64" rx="12" fill="#26262b"/><path d="M32 14l14 5v10c0 9-6 14-14 17-8-3-14-8-14-17V19z" fill="none" stroke="#f97316" stroke-width="3" stroke-linejoin="round"/><path d="M32 25v8M32 38h.01" stroke="#f97316" stroke-width="3" stroke-linecap="round"/></svg>"##;

// ---- Adding files to the queue ---------------------------------------------------------

struct NewScan<'a> {
    key: &'a str,
    kind: &'a str,
    content_type: &'a str,
    uploader_id: Option<i64>,
    guild_id: Option<i64>,
    channel_id: Option<i64>,
    message_id: Option<i64>,
    teen: bool,
    teen_author: bool,
    adult_only: bool,
}

async fn insert_scan(st: &AppState, s: NewScan<'_>, settings: &Settings) {
    let active = service_url().is_some() && settings.enabled;
    let status = if active { "pending" } else { "skipped" };
    let hold = decide(
        status,
        s.kind,
        s.teen,
        s.adult_only,
        None,
        &settings.action,
        settings.hold_teens,
    );
    let result = sqlx::query(
        "INSERT INTO file_scans (id, storage_key, kind, content_type, uploader_id, guild_id,
                                 channel_id, message_id, teen, teen_author, adult_only, status, hold)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (storage_key) DO NOTHING",
    )
    .bind(st.ids.next_id())
    .bind(s.key)
    .bind(s.kind)
    .bind(s.content_type)
    .bind(s.uploader_id)
    .bind(s.guild_id)
    .bind(s.channel_id)
    .bind(s.message_id)
    .bind(s.teen)
    .bind(s.teen_author)
    .bind(s.adult_only)
    .bind(status)
    .bind(hold)
    .execute(&st.db)
    .await;
    match result {
        Ok(_) if active => wake(),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "couldn't queue a picture for the detector"),
    }
}

/// A message's new files were saved: queue its pictures. Called before the
/// message is sent out, so pictures to or from teens start out held.
pub async fn track_message_files(
    st: &AppState,
    channel: &Channel,
    user: &User,
    message_id: i64,
    files: &[(String, String)],
) {
    let pictures: Vec<&(String, String)> = files.iter().filter(|(_, ct)| scannable(ct)).collect();
    if pictures.is_empty() {
        return;
    }
    let guild = (channel.guild_id != DM_GUILD).then_some(channel.guild_id);
    let settings = settings_for(&st.db, guild).await;
    let teen_author = !user.bot && is_teen(&st.db, user.id).await;
    let teen = teen_author || teen_can_see(&st.db, channel).await;
    let adult_only = crate::perms::is_age_gated(channel) && !teen;
    for (key, content_type) in pictures {
        insert_scan(
            st,
            NewScan {
                key,
                kind: KIND_ATTACHMENT,
                content_type,
                uploader_id: Some(user.id),
                guild_id: guild,
                channel_id: Some(channel.id),
                message_id: Some(message_id),
                teen,
                teen_author,
                adult_only,
            },
            &settings,
        )
        .await;
    }
}

/// An avatar, banner, custom emoji or sticker was saved: queue it.
pub async fn track_upload(
    st: &AppState,
    kind: &'static str,
    key: &str,
    content_type: &str,
    uploader: i64,
    guild_id: Option<i64>,
) {
    if !scannable(content_type) {
        return;
    }
    let settings = settings_for(&st.db, guild_id).await;
    let teen_author = is_teen(&st.db, uploader).await;
    let teen = teen_author
        || match guild_id {
            Some(g) => guild_has_teens(&st.db, g).await,
            None => false,
        };
    insert_scan(
        st,
        NewScan {
            key,
            kind,
            content_type,
            uploader_id: Some(uploader),
            guild_id,
            channel_id: None,
            message_id: None,
            teen,
            teen_author,
            adult_only: false,
        },
        &settings,
    )
    .await;
}

/// A report's review screen shows the detector's result for each file: scan
/// the ones it hasn't seen (quarantined ones too; nothing is shown to
/// anyone and nothing is held because of it).
pub async fn ensure_scanned_for_report(st: &AppState, key: &str, content_type: &str) {
    if service_url().is_none() || !scannable(content_type) {
        return;
    }
    let inserted = sqlx::query(
        "INSERT INTO file_scans (id, storage_key, kind, content_type, apply_hold, status)
         VALUES ($1, $2, 'report', $3, false, 'pending')
         ON CONFLICT (storage_key) DO UPDATE
            SET status = 'pending', apply_hold = false, attempts = 0, next_attempt_at = now()
            WHERE file_scans.status = 'skipped'",
    )
    .bind(st.ids.next_id())
    .bind(key)
    .bind(content_type)
    .execute(&st.db)
    .await;
    if matches!(inserted, Ok(r) if r.rows_affected() > 0) {
        wake();
    }
}

// ---- Forwarding --------------------------------------------------------------------------

/// Forwarding pictures: nothing still being checked or held, and a flagged
/// picture (shown in an adult-only 18+ channel) only to other such channels.
pub async fn check_forward(st: &AppState, keys: &[String], targets: &[&Channel]) -> AppResult<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT status, hold, review FROM file_scans WHERE storage_key = ANY($1) AND kind = 'attachment'",
    )
    .bind(keys)
    .fetch_all(&st.db)
    .await?;
    if rows.iter().any(|(status, hold, _)| {
        status == "pending" || matches!(hold.as_str(), "checking" | "held" | "hidden")
    }) {
        return Err(AppError::Refused(
            "A picture in this message is still being checked or is held for review, so it can't be forwarded yet.".into(),
        ));
    }
    let flagged = rows
        .iter()
        .any(|(status, _, review)| status == "flagged" && review.as_deref() != Some("approved"));
    if flagged {
        for t in targets {
            if !crate::perms::is_age_gated(t) || teen_can_see(&st.db, t).await {
                return Err(AppError::Refused(
                    "A picture in this message can only be shared in adult-only 18+ channels."
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

// ---- Revealing a held picture -------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct RevealRow {
    storage_key: String,
    filename: String,
    content_type: String,
    author_id: i64,
    channel_id: i64,
    safety_hidden: bool,
}

/// `GET /api/attachments/{id}/reveal`: a held or blurred picture, for those
/// allowed to look (held: the sender and the site owner; blurred: anyone
/// who can see the message). Never for teens, never for a picture held in
/// a teen context, never for quarantined files.
pub async fn reveal(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    let row = sqlx::query_as::<_, RevealRow>(
        "SELECT a.storage_key, a.filename, a.content_type, m.author_id, m.channel_id, m.safety_hidden
         FROM attachments a JOIN messages m ON m.id = a.message_id
         WHERE a.id = $1",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    // Quarantine wins over everything (451 for everyone, the owner included).
    if super::safety::is_quarantined(&st.db, &row.storage_key).await {
        return Err(AppError::Removed);
    }
    if row.safety_hidden {
        return Err(AppError::NotFound);
    }
    let owner = is_site_owner(&st.db, user.id).await?;
    if !owner {
        crate::perms::channel_access(&st.db, row.channel_id, user.id).await?;
    }
    let scan: Option<(String, bool)> =
        sqlx::query_as("SELECT hold, teen FROM file_scans WHERE storage_key = $1")
            .bind(&row.storage_key)
            .fetch_optional(&st.db)
            .await?;
    let viewer_teen = is_teen(&st.db, user.id).await;
    let (hold, teen) = scan.unwrap_or(("none".into(), false));
    let allowed = match hold.as_str() {
        "none" => true,
        "blur" => !viewer_teen,
        "held" => !teen && !viewer_teen && (row.author_id == user.id || owner),
        _ => false,
    };
    if !allowed {
        return Err(AppError::Held);
    }
    if hold == "held" && row.author_id != user.id {
        super::safety::log_action(
            &st,
            Some(user.id),
            "view_held",
            None,
            Some(row.author_id),
            format!(
                "Looked at a picture held by the image detector ({})",
                row.filename
            ),
        )
        .await;
    }
    let mut res =
        crate::files::send_file(&st, &row.storage_key, &row.filename, &row.content_type).await?;
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(res)
}

// ---- The worker -----------------------------------------------------------------------------

#[derive(sqlx::FromRow, Clone)]
struct ScanRow {
    id: i64,
    storage_key: String,
    kind: String,
    uploader_id: Option<i64>,
    guild_id: Option<i64>,
    message_id: Option<i64>,
    teen: bool,
    adult_only: bool,
    review: Option<String>,
    apply_hold: bool,
    hold: String,
    attempts: i32,
    age_cleared_at: Option<DateTime<Utc>>,
}

const SCAN_COLUMNS: &str = "id, storage_key, kind, uploader_id, guild_id, message_id, teen,
    adult_only, review, apply_hold, hold, attempts, age_cleared_at";

#[derive(Deserialize)]
struct ScanReply {
    score: f32,
    #[serde(default)]
    labels: Vec<Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    ms: Option<i64>,
    /// Faces found (None: the sidecar doesn't check faces).
    #[serde(default)]
    faces: Option<i32>,
    #[serde(default)]
    ages: Vec<Value>,
    #[serde(default)]
    age_model: Option<String>,
}

enum Outcome {
    Scanned(ScanReply),
    /// This picture can't be scanned (not a picture, unreadable, too big, too slow).
    Unreadable(String),
    /// The sidecar didn't answer properly: try again later.
    Offline(String),
}

/// Starts the background scanner (one picture at a time). Pictures still
/// waiting from before a restart are picked up again.
pub fn spawn_worker(st: AppState) {
    tokio::spawn(async move {
        if service_url().is_none() {
            // No detector set up: release anything left waiting.
            release_all_pending(&st).await;
            tracing::info!("DETECTOR_URL not set: the image detector is off");
            return;
        }
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(45))
            .connect_timeout(Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "image detector: couldn't make an HTTP client");
                return;
            }
        };
        loop {
            let next = sqlx::query_as::<_, ScanRow>(&format!(
                "SELECT {SCAN_COLUMNS} FROM file_scans
                 WHERE status = 'pending' AND next_attempt_at <= now()
                 ORDER BY apply_hold, next_attempt_at, id LIMIT 1"
            ))
            .fetch_optional(&st.db)
            .await;
            match next {
                Ok(Some(row)) => {
                    let offline = process(&st, &client, row).await;
                    if offline {
                        // Don't hammer a sidecar that's down.
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
                Ok(None) => {
                    let notify = WAKE.get_or_init(Notify::new);
                    let _ = tokio::time::timeout(Duration::from_secs(20), notify.notified()).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "image detector: couldn't read the queue");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
        }
    });
}

async fn release_all_pending(st: &AppState) {
    let rows: Vec<(String, String, Option<i64>)> = sqlx::query_as(
        "UPDATE file_scans SET status = 'skipped', hold = 'none'
         WHERE status = 'pending' RETURNING storage_key, kind, guild_id",
    )
    .fetch_all(&st.db)
    .await
    .unwrap_or_default();
    for (key, kind, guild) in rows {
        announce(st, &key, &kind, guild).await;
    }
}

async fn call_sidecar(client: &reqwest::Client, url: &str, data: Vec<u8>) -> Outcome {
    let sent = client
        .post(format!("{url}/scan"))
        .header("Content-Type", "application/octet-stream")
        .body(data)
        .send()
        .await;
    let res = match sent {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return Outcome::Offline("it took too long to answer".into()),
        Err(e) => return Outcome::Offline(format!("couldn't reach it ({e})")),
    };
    let code = res.status().as_u16();
    match code {
        200 => match res.json::<ScanReply>().await {
            Ok(r) if r.score.is_finite() => Outcome::Scanned(r),
            _ => Outcome::Offline("it sent back something unexpected".into()),
        },
        413 | 415 | 422 => {
            let why = res
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v["error"].as_str().map(String::from))
                .unwrap_or_else(|| format!("HTTP {code}"));
            Outcome::Unreadable(why)
        }
        504 => Outcome::Unreadable("the scan took too long".into()),
        _ => Outcome::Offline(format!("HTTP {code}")),
    }
}

/// Scans one picture. True if the sidecar seems to be down.
async fn process(st: &AppState, client: &reqwest::Client, row: ScanRow) -> bool {
    let Some(url) = service_url() else {
        return true;
    };
    let settings = settings_for(&st.db, row.guild_id).await;
    if row.apply_hold && !settings.enabled {
        finish(st, &row, "skipped", None, None, &settings).await;
        return false;
    }
    let safe_key =
        !row.storage_key.is_empty() && row.storage_key.bytes().all(|b| b.is_ascii_alphanumeric());
    let data = if safe_key {
        tokio::fs::read(st.config.upload_dir.join(&row.storage_key))
            .await
            .ok()
    } else {
        None
    };
    let Some(data) = data else {
        // The file's gone (deleted before it was scanned): nothing to hold.
        let _ = sqlx::query(
            "UPDATE file_scans SET status = 'skipped', hold = 'none', error = 'The file is gone.'
             WHERE id = $1",
        )
        .bind(row.id)
        .execute(&st.db)
        .await;
        if row.hold != "none" {
            announce(st, &row.storage_key, &row.kind, row.guild_id).await;
        }
        return false;
    };
    match call_sidecar(client, &url, data).await {
        Outcome::Scanned(reply) => {
            health_update(|h| {
                h.offline = false;
                h.last_ok_at = Some(Utc::now());
                h.model = reply.model.clone().or(h.model.take());
                h.scans += 1;
                h.last_ms = reply.ms;
            });
            let limit = threshold(&settings.sensitivity);
            let status = if reply.score >= limit {
                "flagged"
            } else {
                "clean"
            };
            finish(st, &row, status, Some(&reply), None, &settings).await;
            false
        }
        Outcome::Unreadable(why) => {
            health_update(|h| {
                h.offline = false;
                h.last_ok_at = Some(Utc::now());
            });
            finish(st, &row, "error", None, Some(why), &settings).await;
            false
        }
        Outcome::Offline(why) => {
            tracing::warn!(error = %why, "image detector didn't answer; will try again");
            health_update(|h| {
                h.offline = true;
                h.last_error = Some(why.clone());
                h.last_error_at = Some(Utc::now());
            });
            // Back off: 5 s, 10 s, 20 s … up to 5 minutes between tries.
            let wait = (5i64 << row.attempts.clamp(0, 6)).min(300);
            let _ = sqlx::query(
                "UPDATE file_scans SET attempts = attempts + 1, error = $2,
                        next_attempt_at = now() + make_interval(secs => $3::INT)
                 WHERE id = $1",
            )
            .bind(row.id)
            .bind(format!("The detector didn't answer: {why}"))
            .bind(wait as i32)
            .execute(&st.db)
            .await;
            true
        }
    }
}

async fn finish(
    st: &AppState,
    row: &ScanRow,
    status: &str,
    reply: Option<&ScanReply>,
    error: Option<String>,
    settings: &Settings,
) {
    let new_hold = if row.apply_hold {
        decide(
            status,
            &row.kind,
            row.teen,
            row.adult_only,
            row.review.as_deref(),
            &settings.action,
            settings.hold_teens,
        )
    } else {
        // Scanned for a report only: nothing changes for anyone.
        row.hold.as_str()
    };
    let new_hold = new_hold.to_string();
    let ages = reply.map(|r| clean_ages(&r.ages)).unwrap_or_default();
    let faces = reply
        .and_then(|r| r.faces)
        .map(|n| n.max(ages.len() as i32));
    let min_age = ages
        .first()
        .and_then(|a| a["age"].as_f64())
        .map(|a| a as f32);
    let updated = sqlx::query(
        "UPDATE file_scans SET status = $2, hold = $3, score = $4, labels = $5, model = $6,
                threshold = $7, error = $8, scanned_at = now(), attempts = attempts + 1,
                face_count = $9, min_est_age = $10, ages = $11, age_model = $12
         WHERE id = $1",
    )
    .bind(row.id)
    .bind(status)
    .bind(&new_hold)
    .bind(reply.map(|r| r.score))
    .bind(SqlJson(json!(reply
        .map(|r| r.labels.clone())
        .unwrap_or_default())))
    .bind(reply.and_then(|r| r.model.clone()))
    .bind(threshold(&settings.sensitivity))
    .bind(&error)
    .bind(faces)
    .bind(min_age)
    .bind(SqlJson(json!(ages)))
    .bind(reply.and_then(|r| r.age_model.clone()))
    .execute(&st.db)
    .await;
    if let Err(e) = updated {
        tracing::warn!(error = %e, "image detector: couldn't save a result");
        return;
    }
    if new_hold != row.hold {
        announce(st, &row.storage_key, &row.kind, row.guild_id).await;
    }
    // Explicit with a face estimated clearly young: child safety, whatever
    // else would happen (18+ channels and report-only scans too).
    if row.age_cleared_at.is_none() && reply.is_some() {
        let rule = age_rule(&st.db).await;
        if rule.matches(reply.map(|r| r.score), min_age) {
            if let Err(e) = escalate(st, row.id, &rule).await {
                tracing::warn!(error = %e, "image detector: couldn't escalate to child safety");
            }
            return;
        }
    }
    // Held, hidden or blurred: the site owner gets a detector report.
    let needs_report = row.apply_hold
        && row.review.is_none()
        && matches!(new_hold.as_str(), "held" | "hidden" | "blur")
        && matches!(status, "flagged" | "error");
    if needs_report {
        let score = reply.map(|r| r.score);
        let labels: Vec<Value> = reply.map(|r| r.labels.clone()).unwrap_or_default();
        let summary = summary_line(status, score, &labels, error.as_deref());
        let target = super::safety::DetectorTarget {
            scan_id: row.id,
            storage_key: row.storage_key.clone(),
            kind: row.kind.clone(),
            message_id: row.message_id,
            uploader_id: row.uploader_id,
            guild_id: row.guild_id,
            category: if status == "flagged" {
                "sexual"
            } else {
                "other"
            },
            detector: json!({
                "score": score.map(round2),
                "labels": labels,
                "summary": summary,
                "hold": new_hold,
                "teen": row.teen,
                "faces": faces,
                "min_age": min_age.map(round1),
                "age_note": AGE_NOTE,
            }),
            auto_escalation: None,
        };
        if let Err(e) = super::safety::detector_report(st, target).await {
            tracing::warn!(error = %e, "image detector: couldn't make a report");
        }
    }
}

#[derive(sqlx::FromRow)]
struct EscalateRow {
    storage_key: String,
    kind: String,
    message_id: Option<i64>,
    uploader_id: Option<i64>,
    guild_id: Option<i64>,
    teen: bool,
    hold: String,
    score: Option<f32>,
    labels: SqlJson<Vec<Value>>,
    face_count: Option<i32>,
    min_est_age: Option<f32>,
}

/// The age rule matched this file: every open report about it becomes a
/// child-safety report (quarantine, hidden messages, urgent alert), or, with
/// none open, a child-safety report from the detector is made.
async fn escalate(st: &AppState, scan_id: i64, rule: &AgeRule) -> AppResult<()> {
    let r = sqlx::query_as::<_, EscalateRow>(
        "UPDATE file_scans SET age_escalated_at = COALESCE(age_escalated_at, now()) WHERE id = $1
         RETURNING storage_key, kind, message_id, uploader_id, guild_id, teen, hold, score, labels,
                   face_count, min_est_age",
    )
    .bind(scan_id)
    .fetch_one(&st.db)
    .await?;
    let line = rule.line();
    let info = json!({
        "line": line,
        "at": Utc::now(),
        "score": r.score.map(round2),
        "faces": r.face_count,
        "min_age": r.min_est_age.map(round1),
        "age_threshold": rule.age,
        "escalation_score": round2(rule.score),
        "note": format!("The age is a {AGE_NOTE}: a small model's guess from the face alone, often wrong by years, and it can mistake drawings for faces. It's why this was escalated, not proof of anything."),
    });
    let open: Vec<i64> = sqlx::query_scalar(
        "SELECT r.id FROM reports r
         WHERE r.status <> 'closed'
           AND ((r.target_kind = 'file' AND r.target_id = $1)
                OR r.id IN (SELECT report_id FROM report_files WHERE storage_key = $2))
         ORDER BY r.id",
    )
    .bind(scan_id)
    .bind(&r.storage_key)
    .fetch_all(&st.db)
    .await?;
    if !open.is_empty() {
        for id in open {
            super::safety::auto_child_safety(st, id, &info).await?;
        }
        return Ok(());
    }
    if r.kind == KIND_REPORT {
        return Ok(()); // its report was closed meanwhile
    }
    let target = super::safety::DetectorTarget {
        scan_id,
        storage_key: r.storage_key.clone(),
        kind: r.kind.clone(),
        message_id: r.message_id,
        uploader_id: r.uploader_id,
        guild_id: r.guild_id,
        category: super::safety::CHILD_SAFETY,
        detector: json!({
            "score": r.score.map(round2),
            "labels": r.labels.0,
            "summary": line,
            "hold": r.hold,
            "teen": r.teen,
            "faces": r.face_count,
            "min_age": r.min_est_age.map(round1),
            "age_note": AGE_NOTE,
        }),
        auto_escalation: Some(info),
    };
    super::safety::detector_report(st, target).await
}

/// A report about files was made or opened: show (and act on) their age
/// estimates. Files never scanned, or scanned before faces were checked,
/// are scanned now (the scan escalates when it's done); files already
/// scanned that match the age rule escalate the report straight away.
pub async fn check_report(st: &AppState, report_id: i64) {
    let rule = age_rule(&st.db).await;
    if !rule.enabled {
        return;
    }
    let files: Vec<(String, String)> =
        sqlx::query_as("SELECT storage_key, content_type FROM report_files WHERE report_id = $1")
            .bind(report_id)
            .fetch_all(&st.db)
            .await
            .unwrap_or_default();
    for (key, content_type) in files {
        if !scannable(&content_type) {
            continue;
        }
        let scan: Option<(
            i64,
            String,
            Option<f32>,
            Option<i32>,
            Option<f32>,
            Option<DateTime<Utc>>,
            Option<i64>,
        )> = sqlx::query_as(
            "SELECT id, status, score, face_count, min_est_age, age_cleared_at, guild_id
                 FROM file_scans WHERE storage_key = $1",
        )
        .bind(&key)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
        match scan {
            None => ensure_scanned_for_report(st, &key, &content_type).await,
            Some((_, status, score, _, _, _, _)) if status == "skipped" && score.is_none() => {
                ensure_scanned_for_report(st, &key, &content_type).await
            }
            Some((id, status, _, None, _, None, guild))
                if matches!(status.as_str(), "clean" | "flagged") =>
            {
                rescan_for_ages(st, id, guild).await
            }
            Some((id, _, score, Some(_), min_age, None, _)) if rule.matches(score, min_age) => {
                if let Err(e) = escalate(st, id, &rule).await {
                    tracing::warn!(error = %e, "image detector: couldn't escalate a report");
                }
            }
            _ => {}
        }
    }
}

/// Scanned before faces were checked: scan it again (nothing changes for
/// anyone meanwhile; the same score gives the same hold).
async fn rescan_for_ages(st: &AppState, scan_id: i64, guild_id: Option<i64>) {
    if service_url().is_none() || !settings_for(&st.db, guild_id).await.enabled {
        return;
    }
    let requeued = sqlx::query(
        "UPDATE file_scans SET status = 'pending', attempts = 0, next_attempt_at = now()
         WHERE id = $1 AND face_count IS NULL AND status IN ('clean', 'flagged')",
    )
    .bind(scan_id)
    .execute(&st.db)
    .await;
    if matches!(requeued, Ok(r) if r.rows_affected() > 0) {
        wake();
    }
}

/// The owner dismissed an automatic child-safety escalation as a mistake:
/// no more automatic escalations for these files, and a picture the
/// detector still holds gets its normal detector report back (so it can be
/// approved or removed as usual).
pub(crate) async fn escalation_dismissed(st: &AppState, keys: &[String]) {
    let cleared: Vec<i64> = sqlx::query_scalar(
        "UPDATE file_scans SET age_cleared_at = COALESCE(age_cleared_at, now())
         WHERE storage_key = ANY($1) AND age_escalated_at IS NOT NULL
         RETURNING id",
    )
    .bind(keys)
    .fetch_all(&st.db)
    .await
    .unwrap_or_default();
    for scan_id in cleared {
        let row = sqlx::query_as::<_, ScanRow>(&format!(
            "SELECT {SCAN_COLUMNS} FROM file_scans
             WHERE id = $1 AND apply_hold AND review IS NULL AND hold IN ('held', 'hidden', 'blur')
               AND status IN ('flagged', 'error')"
        ))
        .bind(scan_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
        let Some(row) = row else { continue };
        let r = sqlx::query_as::<_, EscalateRow>(
            "SELECT storage_key, kind, message_id, uploader_id, guild_id, teen, hold, score, labels,
                    face_count, min_est_age
             FROM file_scans WHERE id = $1",
        )
        .bind(scan_id)
        .fetch_one(&st.db)
        .await;
        let Ok(r) = r else { continue };
        let target = super::safety::DetectorTarget {
            scan_id,
            storage_key: row.storage_key.clone(),
            kind: row.kind.clone(),
            message_id: row.message_id,
            uploader_id: row.uploader_id,
            guild_id: row.guild_id,
            category: "sexual",
            detector: json!({
                "score": r.score.map(round2),
                "labels": r.labels.0,
                "summary": summary_line("flagged", r.score, &r.labels.0, None),
                "hold": r.hold,
                "teen": r.teen,
                "faces": r.face_count,
                "min_age": r.min_est_age.map(round1),
                "age_note": AGE_NOTE,
            }),
            auto_escalation: None,
        };
        if let Err(e) = super::safety::detector_report(st, target).await {
            tracing::warn!(error = %e, "image detector: couldn't make a report");
        }
    }
}

/// Tells open clients a picture's hold changed (messages are sent again;
/// emoji and sticker lists are re-fetched).
async fn announce(st: &AppState, key: &str, kind: &str, guild_id: Option<i64>) {
    match kind {
        KIND_ATTACHMENT => {
            let messages: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
                "SELECT DISTINCT m.id, m.channel_id, c.guild_id FROM attachments a
                 JOIN messages m ON m.id = a.message_id JOIN channels c ON c.id = m.channel_id
                 WHERE a.storage_key = $1 AND NOT m.safety_hidden LIMIT 50",
            )
            .bind(key)
            .fetch_all(&st.db)
            .await
            .unwrap_or_default();
            for (id, channel_id, guild) in messages {
                if let Ok(message) = super::messages::fetch_message(&st.db, id).await {
                    st.dispatch(
                        Audience::Channel {
                            guild_id: guild.unwrap_or(DM_GUILD),
                            channel_id,
                        },
                        ServerEvent::MessageUpdate(message),
                    );
                }
            }
        }
        KIND_EMOJI => {
            if let Some(guild_id) = guild_id {
                st.dispatch(
                    Audience::Guild(guild_id),
                    ServerEvent::EmojisChanged { guild_id },
                );
            }
        }
        KIND_STICKER => {
            if let Some(guild_id) = guild_id {
                st.dispatch(
                    Audience::Guild(guild_id),
                    ServerEvent::StickersChanged { guild_id },
                );
            }
        }
        KIND_GUILD_ICON | KIND_GUILD_BANNER => {
            if let Some(guild_id) = guild_id {
                super::guild_info::announce(st, guild_id).await;
            }
        }
        _ => {}
    }
}

// ---- What the owner sees ----------------------------------------------------------------

/// 0.6000000238 → 0.6 (for JSON).
fn round2(x: f32) -> f64 {
    (x as f64 * 1000.0).round() / 1000.0
}

/// 8.4000001 → 8.4 (ages, for JSON).
fn round1(x: f32) -> f64 {
    (x as f64 * 10.0).round() / 10.0
}

fn fmt_score(score: f32) -> String {
    format!("{score:.2}")
}

fn label_list(labels: &[Value]) -> String {
    labels
        .iter()
        .take(3)
        .filter_map(|l| {
            let name = l["label"].as_str()?;
            let score = l["score"].as_f64().unwrap_or(0.0);
            Some(format!(
                "{} {score:.2}",
                name.to_lowercase().replace('_', " ")
            ))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// "Detector: 0.02 — nothing explicit found" and friends.
pub fn summary_line(
    status: &str,
    score: Option<f32>,
    labels: &[Value],
    error: Option<&str>,
) -> String {
    match (status, score) {
        ("pending", _) => "Detector: checking…".into(),
        ("error", _) => format!(
            "Detector: couldn't check this file ({})",
            error.unwrap_or("unreadable")
        ),
        ("skipped", None) => "Detector: not scanned".into(),
        (_, Some(s)) if s < 0.25 => format!("Detector: {} — nothing explicit found", fmt_score(s)),
        (_, Some(s)) => {
            let what = if s >= preset_threshold("medium") {
                "explicit content likely"
            } else {
                "possibly explicit"
            };
            let labels = label_list(labels);
            if labels.is_empty() {
                format!("Detector: {} — {what}", fmt_score(s))
            } else {
                format!("Detector: {} — {what} ({labels})", fmt_score(s))
            }
        }
        _ => "Detector: not scanned".into(),
    }
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: i64,
    kind: String,
    status: String,
    hold: String,
    review: Option<String>,
    score: Option<f32>,
    labels: SqlJson<Vec<Value>>,
    model: Option<String>,
    threshold: Option<f32>,
    error: Option<String>,
    teen: bool,
    teen_author: bool,
    adult_only: bool,
    scanned_at: Option<DateTime<Utc>>,
    face_count: Option<i32>,
    min_est_age: Option<f32>,
    ages: SqlJson<Vec<Value>>,
    age_model: Option<String>,
    age_escalated_at: Option<DateTime<Utc>>,
    age_cleared_at: Option<DateTime<Utc>>,
}

const SUMMARY_COLUMNS: &str = "id, kind, status, hold, review, score, labels, model, threshold,
    error, teen, teen_author, adult_only, scanned_at, face_count, min_est_age, ages, age_model,
    age_escalated_at, age_cleared_at";

fn summary_json(r: &SummaryRow) -> Value {
    let summary = summary_line(&r.status, r.score, &r.labels.0, r.error.as_deref());
    let near_zero = r.score.is_some_and(|s| s < 0.1) && r.status != "pending";
    json!({
        "status": r.status,
        "score": r.score.map(round2),
        "labels": r.labels.0,
        "model": r.model,
        "threshold": r.threshold.map(round2),
        "scanned_at": r.scanned_at,
        "summary": summary,
        "near_zero": near_zero,
        "hold": r.hold,
        "review": r.review,
        "kind": r.kind,
        "teen": r.teen,
        // Faces and ages: a preliminary automated estimate, not evidence.
        "faces": r.face_count,
        "min_age": r.min_est_age.map(round1),
        "ages": r.ages.0,
        "age_model": r.age_model,
        "age_line": if r.status == "pending" { None } else { age_line(r.score, r.face_count, r.min_est_age) },
        "age_note": AGE_NOTE,
        "age_escalated": r.age_escalated_at.is_some(),
        "age_escalated_at": r.age_escalated_at,
        "age_cleared": r.age_cleared_at.is_some(),
    })
}

/// The detector's result for a reported file (metadata only). Starts a
/// scan if it was never scanned.
pub async fn file_summary(st: &AppState, key: &str, content_type: &str) -> Value {
    let row = sqlx::query_as::<_, SummaryRow>(&format!(
        "SELECT {SUMMARY_COLUMNS} FROM file_scans WHERE storage_key = $1"
    ))
    .bind(key)
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten();
    let needs_scan = match &row {
        None => true,
        Some(r) => r.status == "skipped" && r.score.is_none(),
    };
    if needs_scan && scannable(content_type) && service_url().is_some() {
        ensure_scanned_for_report(st, key, content_type).await;
        return json!({ "status": "pending", "summary": "Detector: checking…", "near_zero": false });
    }
    match row {
        Some(r) => summary_json(&r),
        None if !scannable(content_type) => {
            json!({ "status": "skipped", "summary": "Detector: not a picture it can check", "near_zero": false })
        }
        None => {
            json!({ "status": "skipped", "summary": "Detector: not set up on this server", "near_zero": false })
        }
    }
}

/// A detector report's panel: the scan and where the picture was posted.
pub async fn report_panel(db: &PgPool, scan_id: i64) -> Option<Value> {
    let r = sqlx::query_as::<_, SummaryRow>(&format!(
        "SELECT {SUMMARY_COLUMNS} FROM file_scans WHERE id = $1"
    ))
    .bind(scan_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()?;
    let mut v = summary_json(&r);
    v["id"] = json!(r.id.to_string());
    v["teen_author"] = json!(r.teen_author);
    v["adult_only"] = json!(r.adult_only);
    v["viewable"] = json!(!r.teen);
    Some(v)
}

/// May the site owner look at this file in the console? Not if it's held
/// in a teen context.
pub async fn owner_may_view(db: &PgPool, key: &str) -> bool {
    let teen: Option<bool> = sqlx::query_scalar(
        "SELECT teen FROM file_scans WHERE storage_key = $1 AND status IN ('flagged', 'error')",
    )
    .bind(key)
    .fetch_optional(db)
    .await
    .unwrap_or(Some(true));
    !teen.unwrap_or(false)
}

/// The owner's decision on a detector report: approve (release), keep
/// blurred / hidden, or remove the file. Returns what happened (for the log).
pub async fn owner_decision(st: &AppState, scan_id: i64, action: &str) -> AppResult<String> {
    let row = sqlx::query_as::<_, ScanRow>(&format!(
        "SELECT {SCAN_COLUMNS} FROM file_scans WHERE id = $1"
    ))
    .bind(scan_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let status: String = sqlx::query_scalar("SELECT status FROM file_scans WHERE id = $1")
        .bind(scan_id)
        .fetch_one(&st.db)
        .await?;
    let settings = settings_for(&st.db, row.guild_id).await;
    match action {
        "approve_file" | "keep_blurred" => {
            let review = if action == "approve_file" {
                "approved"
            } else {
                "blurred"
            };
            let hold = decide(
                &status,
                &row.kind,
                row.teen,
                row.adult_only,
                Some(review),
                &settings.action,
                settings.hold_teens,
            );
            sqlx::query("UPDATE file_scans SET review = $2, hold = $3 WHERE id = $1")
                .bind(scan_id)
                .bind(review)
                .bind(hold)
                .execute(&st.db)
                .await?;
            if hold != row.hold {
                announce(st, &row.storage_key, &row.kind, row.guild_id).await;
            }
            Ok(match (review, hold) {
                ("approved", _) => "Approved: the picture is shown again".into(),
                (_, "blur") => "Kept blurred (people can click through)".into(),
                (_, "hidden") => "Kept hidden".into(),
                _ => "Kept held (it was sent to or from a teen)".into(),
            })
        }
        "remove_file" => {
            let n = remove_everywhere(st, &row).await?;
            Ok(format!(
                "Removed the picture ({n} place{})",
                if n == 1 { "" } else { "s" }
            ))
        }
        _ => Err(AppError::BadRequest("Unknown action.".into())),
    }
}

/// Takes a flagged picture off everything that uses it (the report keeps the file).
async fn remove_everywhere(st: &AppState, row: &ScanRow) -> AppResult<usize> {
    let key = &row.storage_key;
    match row.kind.as_str() {
        KIND_ATTACHMENT => {
            let uses: Vec<(i64, i64)> =
                sqlx::query_as("SELECT id, message_id FROM attachments WHERE storage_key = $1")
                    .bind(key)
                    .fetch_all(&st.db)
                    .await?;
            for (attachment_id, message_id) in &uses {
                super::safety::remove_attachments(st, *message_id, Some(*attachment_id)).await?;
            }
            Ok(uses.len())
        }
        KIND_AVATAR | KIND_BANNER => {
            let col = if row.kind == KIND_AVATAR {
                "avatar_key"
            } else {
                "banner_key"
            };
            let n = sqlx::query(&format!("UPDATE users SET {col} = NULL WHERE {col} = $1"))
                .bind(key)
                .execute(&st.db)
                .await?
                .rows_affected();
            sqlx::query("DELETE FROM profile_files WHERE storage_key = $1")
                .bind(key)
                .execute(&st.db)
                .await?;
            Ok(n as usize)
        }
        KIND_GUILD_ICON | KIND_GUILD_BANNER => {
            let col = if row.kind == KIND_GUILD_ICON {
                "icon_key"
            } else {
                "banner_key"
            };
            let guilds: Vec<i64> = sqlx::query_scalar(&format!(
                "UPDATE guilds SET {col} = NULL WHERE {col} = $1 RETURNING id"
            ))
            .bind(key)
            .fetch_all(&st.db)
            .await?;
            sqlx::query("DELETE FROM profile_files WHERE storage_key = $1")
                .bind(key)
                .execute(&st.db)
                .await?;
            for g in &guilds {
                super::guild_info::announce(st, *g).await;
            }
            Ok(guilds.len())
        }
        KIND_EMOJI | KIND_STICKER => {
            let table = if row.kind == KIND_EMOJI {
                "emojis"
            } else {
                "stickers"
            };
            let n = sqlx::query(&format!("DELETE FROM {table} WHERE storage_key = $1"))
                .bind(key)
                .execute(&st.db)
                .await?
                .rows_affected();
            announce(st, key, &row.kind, row.guild_id).await;
            Ok(n as usize)
        }
        _ => Ok(0),
    }
}

// ---- Health, settings ---------------------------------------------------------------------

/// The detector's health for Server status and the settings page.
pub async fn health(st: &AppState) -> Value {
    let url = service_url();
    let (queue, held): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'pending'),
                count(*) FILTER (WHERE hold IN ('held', 'hidden'))
         FROM file_scans",
    )
    .fetch_one(&st.db)
    .await
    .unwrap_or((0, 0));
    let mut live: Option<Value> = None;
    if let Some(url) = &url {
        let res = st
            .http
            .get(format!("{url}/health"))
            .timeout(Duration::from_secs(3))
            .send()
            .await;
        if let Ok(r) = res {
            if r.status().is_success() {
                live = r.json::<Value>().await.ok();
            }
        }
    }
    let h = health_now();
    let site = site_settings(&st.db).await;
    json!({
        "configured": url.is_some(),
        "enabled": site.enabled,
        "online": live.is_some(),
        "model": live.as_ref().and_then(|v| v["model"].as_str().map(String::from)).or(h.model),
        "test_mode": live.as_ref().and_then(|v| v["test_mode"].as_bool()).unwrap_or(false),
        // Every model the sidecar runs (nudity, faces, age).
        "models": live.as_ref().map(|v| v["models"].clone()).filter(|m| m.is_object()),
        "age_check": live.as_ref().and_then(|v| v["age"].as_bool()),
        "queue": queue,
        "held": held,
        "scans_since_start": h.scans,
        "last_scan_ms": h.last_ms,
        "last_ok_at": h.last_ok_at,
        "last_error": if h.offline || live.is_none() { h.last_error } else { None },
        "last_error_at": h.last_error_at,
        "threshold_override": threshold_override(),
        "escalation_age_override": escalation_age_override(),
        "escalation_score_override": escalation_score_override(),
    })
}

fn age_json(rule: &AgeRule) -> Value {
    json!({
        "enabled": rule.enabled,
        "threshold": rule.age_setting,
        "escalation_score": round2(rule.score_setting),
        "effective_age": rule.age,
        "effective_score": round2(rule.score),
        "thresholds": AGE_THRESHOLDS,
        "scores": ESCALATION_SCORES.iter().map(|s| round2(*s)).collect::<Vec<_>>(),
        "note": AGE_NOTE,
    })
}

async fn require_owner(st: &AppState, user: &User) -> AppResult<()> {
    if user.bot || !is_site_owner(&st.db, user.id).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn settings_json(s: &Settings) -> Value {
    json!({
        "enabled": s.enabled,
        "sensitivity": s.sensitivity,
        "action": s.action,
        "hold_teens": s.hold_teens,
    })
}

/// `GET /api/safety/detector`: the site-wide settings, health and every
/// outhouse's own settings (site owner only).
pub async fn get_settings(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let site = site_settings(&st.db).await;
    let guilds: Vec<(i64, String, bool, Option<String>, Option<String>, bool)> = sqlx::query_as(
        "SELECT id, name, detector_enabled, detector_sensitivity, detector_action, detector_hold_teens
         FROM guilds ORDER BY lower(name), id",
    )
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!({
        "settings": settings_json(&site),
        "age": age_json(&age_rule(&st.db).await),
        "thresholds": { "low": round2(preset_threshold("low")), "medium": round2(preset_threshold("medium")), "high": round2(preset_threshold("high")) },
        "threshold_override": threshold_override(),
        "health": health(&st).await,
        "outhouses": guilds.iter().map(|(id, name, enabled, sens, action, hold)| json!({
            "id": id.to_string(), "name": name, "enabled": enabled,
            "sensitivity": sens, "action": action, "hold_teens": hold,
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct SettingsUpdate {
    enabled: Option<bool>,
    sensitivity: Option<String>,
    action: Option<String>,
    hold_teens: Option<bool>,
    /// Site-wide only (ignored for an outhouse): the age rule.
    age_check: Option<bool>,
    age_threshold: Option<i32>,
    escalation_score: Option<f32>,
}

fn check_choice(value: &Option<String>, allowed: &[&str], what: &str) -> AppResult<()> {
    match value.as_deref() {
        Some(v) if !v.is_empty() && v != "default" && !allowed.contains(&v) => {
            Err(AppError::BadRequest(format!("Unknown {what}.")))
        }
        _ => Ok(()),
    }
}

const SENSITIVITIES: &[&str] = &["low", "medium", "high"];
const ACTIONS: &[&str] = &["hold", "blur"];

/// `PUT /api/safety/detector` `{enabled?, sensitivity?, action?, hold_teens?}`
pub async fn set_settings(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<SettingsUpdate>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    check_choice(&req.sensitivity, SENSITIVITIES, "sensitivity")?;
    check_choice(&req.action, ACTIONS, "action")?;
    let sens = req
        .sensitivity
        .filter(|s| SENSITIVITIES.contains(&s.as_str()));
    let action = req.action.filter(|s| ACTIONS.contains(&s.as_str()));
    if req
        .age_threshold
        .is_some_and(|a| !AGE_THRESHOLDS.contains(&a))
    {
        return Err(AppError::BadRequest(
            "The age threshold can be 10, 13 or 16.".into(),
        ));
    }
    if req
        .escalation_score
        .is_some_and(|x| !(x.is_finite() && (0.05..=0.95).contains(&x)))
    {
        return Err(AppError::BadRequest(
            "The escalation score can be from 0.05 to 0.95.".into(),
        ));
    }
    let age_before = age_rule(&st.db).await;
    sqlx::query(
        "UPDATE site_settings SET
             detector_enabled = COALESCE($1, detector_enabled),
             detector_sensitivity = COALESCE($2, detector_sensitivity),
             detector_action = COALESCE($3, detector_action),
             detector_hold_teens = COALESCE($4, detector_hold_teens),
             detector_age_check = COALESCE($5, detector_age_check),
             detector_age_threshold = COALESCE($6, detector_age_threshold),
             detector_escalation_score = COALESCE($7, detector_escalation_score)
         WHERE id = 1",
    )
    .bind(req.enabled)
    .bind(&sens)
    .bind(&action)
    .bind(req.hold_teens)
    .bind(req.age_check)
    .bind(req.age_threshold)
    .bind(req.escalation_score)
    .execute(&st.db)
    .await?;
    let s = site_settings(&st.db).await;
    let age = age_rule(&st.db).await;
    if age.enabled != age_before.enabled
        || age.age_setting != age_before.age_setting
        || (age.score_setting - age_before.score_setting).abs() > 0.0001
    {
        super::safety::log_action(
            &st,
            Some(user.id),
            "settings",
            None,
            None,
            format!(
                "Age check {}: explicit pictures (nudity {} or more) with a face estimated {} or under are escalated to child safety",
                if age.enabled { "on" } else { "off" },
                fmt_score(age.score_setting),
                age.age_setting,
            ),
        )
        .await;
    }
    super::safety::log_action(
        &st,
        Some(user.id),
        "settings",
        None,
        None,
        format!(
            "Image detector {}, sensitivity {}, outside 18+ channels: {}, hold DM pictures to/from teens until checked: {}",
            if s.enabled { "on" } else { "off" },
            s.sensitivity,
            if s.action == "blur" { "blur only" } else { "hold for approval" },
            if s.hold_teens { "on" } else { "off" },
        ),
    )
    .await;
    // Saving also retries anything waiting for the detector straight away.
    sqlx::query("UPDATE file_scans SET next_attempt_at = now() WHERE status = 'pending'")
        .execute(&st.db)
        .await?;
    if s.enabled {
        wake();
    }
    let mut out = settings_json(&s);
    out["age"] = age_json(&age);
    Ok(Json(out))
}

/// `GET /api/safety/guilds/{id}/detector`
pub async fn get_guild_settings(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    guild_settings_json(&st, guild_id).await.map(Json)
}

async fn guild_settings_json(st: &AppState, guild_id: i64) -> AppResult<Value> {
    let (name, enabled, sens, action, hold): (String, bool, Option<String>, Option<String>, bool) =
        sqlx::query_as(
            "SELECT name, detector_enabled, detector_sensitivity, detector_action, detector_hold_teens
             FROM guilds WHERE id = $1",
        )
        .bind(guild_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let site = site_settings(&st.db).await;
    Ok(json!({
        "id": guild_id.to_string(),
        "name": name,
        "enabled": enabled,
        "sensitivity": sens,
        "action": action,
        "hold_teens": hold,
        "site": settings_json(&site),
        "effective": settings_json(&settings_for(&st.db, Some(guild_id)).await),
    }))
}

/// `PUT /api/safety/guilds/{id}/detector` `{enabled?, sensitivity?, action?,
/// hold_teens?}` ("default" for sensitivity or action = the site's setting).
pub async fn set_guild_settings(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<SettingsUpdate>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    check_choice(&req.sensitivity, SENSITIVITIES, "sensitivity")?;
    check_choice(&req.action, ACTIONS, "action")?;
    // None = leave it; Some("default") = back to the site's setting.
    let set_sens = req.sensitivity.is_some();
    let set_action = req.action.is_some();
    let sens = req
        .sensitivity
        .filter(|s| SENSITIVITIES.contains(&s.as_str()));
    let action = req.action.filter(|s| ACTIONS.contains(&s.as_str()));
    let name: String = sqlx::query_scalar(
        "UPDATE guilds SET
             detector_enabled = COALESCE($2, detector_enabled),
             detector_sensitivity = CASE WHEN $3 THEN $4 ELSE detector_sensitivity END,
             detector_action = CASE WHEN $5 THEN $6 ELSE detector_action END,
             detector_hold_teens = COALESCE($7, detector_hold_teens)
         WHERE id = $1 RETURNING name",
    )
    .bind(guild_id)
    .bind(req.enabled)
    .bind(set_sens)
    .bind(&sens)
    .bind(set_action)
    .bind(&action)
    .bind(req.hold_teens)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let v = guild_settings_json(&st, guild_id).await?;
    super::safety::log_action(
        &st,
        Some(user.id),
        "settings",
        None,
        None,
        format!(
            "Image detector in {name}: {}, sensitivity {}, outside 18+ channels: {}, hold pictures to/from teens until checked: {}",
            if v["enabled"] == true { "on" } else { "off" },
            v["sensitivity"].as_str().unwrap_or("site default"),
            v["action"].as_str().map(|a| if a == "blur" { "blur only" } else { "hold for approval" }).unwrap_or("site default"),
            if v["hold_teens"] == true { "on" } else { "off" },
        ),
    )
    .await;
    Ok(Json(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions() {
        // Adult-only 18+ channel: shown.
        assert_eq!(
            decide("flagged", "attachment", false, true, None, "hold", true),
            "none"
        );
        // To or from a teen: held, even in "blur only" mode or an 18+ channel.
        assert_eq!(
            decide("flagged", "attachment", true, false, None, "blur", true),
            "held"
        );
        assert_eq!(
            decide("flagged", "attachment", true, true, None, "hold", true),
            "held"
        );
        assert_eq!(
            decide(
                "flagged",
                "attachment",
                true,
                false,
                Some("blurred"),
                "hold",
                true
            ),
            "held"
        );
        // Elsewhere: held for approval, or blurred.
        assert_eq!(
            decide("flagged", "attachment", false, false, None, "hold", true),
            "held"
        );
        assert_eq!(
            decide("flagged", "attachment", false, false, None, "blur", true),
            "blur"
        );
        assert_eq!(
            decide(
                "flagged",
                "attachment",
                false,
                false,
                Some("blurred"),
                "hold",
                true
            ),
            "blur"
        );
        assert_eq!(
            decide(
                "flagged",
                "attachment",
                true,
                false,
                Some("approved"),
                "hold",
                true
            ),
            "none"
        );
        // Avatars, banners, emoji and stickers: hidden until approved.
        assert_eq!(
            decide("flagged", "avatar", false, false, None, "blur", true),
            "hidden"
        );
        assert_eq!(
            decide(
                "flagged",
                "emoji",
                false,
                false,
                Some("blurred"),
                "hold",
                true
            ),
            "hidden"
        );
        assert_eq!(
            decide(
                "flagged",
                "sticker",
                false,
                false,
                Some("approved"),
                "hold",
                true
            ),
            "none"
        );
        // Still checking: held only to or from teens (when that's on).
        assert_eq!(
            decide("pending", "attachment", true, false, None, "hold", true),
            "checking"
        );
        assert_eq!(
            decide("pending", "attachment", true, false, None, "hold", false),
            "none"
        );
        assert_eq!(
            decide("pending", "attachment", false, false, None, "hold", true),
            "none"
        );
        assert_eq!(
            decide("pending", "avatar", true, false, None, "hold", true),
            "hidden"
        );
        // Unreadable: held to or from teens, shown otherwise.
        assert_eq!(
            decide("error", "attachment", true, false, None, "hold", true),
            "held"
        );
        assert_eq!(
            decide("error", "attachment", false, false, None, "hold", true),
            "none"
        );
        assert_eq!(
            decide("clean", "attachment", true, false, None, "hold", true),
            "none"
        );
        assert_eq!(
            decide("skipped", "attachment", true, false, None, "hold", true),
            "none"
        );
        assert_eq!(
            decide("flagged", "report", true, false, None, "hold", true),
            "none"
        );
    }

    #[test]
    fn age_rule_matches() {
        let rule = AgeRule {
            enabled: true,
            age: 13.0,
            score: 0.40,
            age_setting: 13,
            score_setting: 0.40,
        };
        // Explicit and young: escalated.
        assert!(rule.matches(Some(0.97), Some(8.0)));
        assert!(rule.matches(Some(0.40), Some(13.0)));
        // Young without nudity, explicit with an adult, no face: nothing.
        assert!(!rule.matches(Some(0.05), Some(8.0)));
        assert!(!rule.matches(Some(0.97), Some(34.0)));
        assert!(!rule.matches(Some(0.97), None));
        assert!(!rule.matches(None, Some(8.0)));
        assert!(!rule.matches(Some(0.39), Some(5.0)));
        // Switched off: nothing.
        assert!(!AgeRule {
            enabled: false,
            ..rule.clone()
        }
        .matches(Some(0.97), Some(8.0)));
        assert_eq!(
            rule.line(),
            "Automatic: explicit content with a face estimated 13 or under"
        );
        let ages = clean_ages(&[
            json!({"age": 34.21, "confidence": 0.5, "range": "30-39"}),
            json!({"age": 8.04, "confidence": 1.7}),
            json!({"age": "x"}),
            json!({"age": f64::NAN}),
        ]);
        assert_eq!(
            ages,
            vec![
                json!({"age": 8.0, "confidence": 1.0}),
                json!({"age": 34.2, "confidence": 0.5})
            ]
        );
    }

    #[test]
    fn thresholds_and_summaries() {
        assert!(threshold("low") > threshold("medium") && threshold("medium") > threshold("high"));
        assert_eq!(threshold("nonsense"), threshold("medium"));
        assert_eq!(
            summary_line("clean", Some(0.02), &[], None),
            "Detector: 0.02 — nothing explicit found"
        );
        let labels = vec![json!({"label": "FEMALE_BREAST_EXPOSED", "score": 0.91})];
        assert_eq!(
            summary_line("flagged", Some(0.91), &labels, None),
            "Detector: 0.91 — explicit content likely (female breast exposed 0.91)"
        );
        assert_eq!(
            summary_line("pending", None, &[], None),
            "Detector: checking…"
        );
        assert!(summary_line("error", None, &[], Some("not a picture")).contains("not a picture"));
        assert_eq!(
            age_line(Some(0.82), Some(2), Some(8.6)).as_deref(),
            Some("Detector: nudity 0.82 · faces 2 · youngest estimated ~9 (preliminary automated estimate, not evidence)")
        );
        assert_eq!(
            age_line(Some(0.02), Some(0), None).as_deref(),
            Some("Detector: nudity 0.02 · no faces found")
        );
        assert_eq!(age_line(Some(0.5), None, None), None);
        assert!(scannable("image/webp") && !scannable("image/svg+xml") && !scannable("video/mp4"));
    }
}
