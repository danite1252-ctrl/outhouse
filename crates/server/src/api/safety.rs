//! Safety: reports to the site owner, the owner's review console, account
//! suspensions, age groups and teen protections.
//!
//! Anyone can report a message (channels, DMs, threads), a file, a person,
//! a sticker, a custom emoji or an outhouse. The server keeps a snapshot of
//! what was reported (with surrounding messages and file hashes) so deleting
//! it doesn't destroy the evidence, and the files stay on disk until the
//! report is closed. Child-safety reports go to the top of the list, their
//! files are quarantined at once (nobody can download them, not even the
//! site owner) and the message is hidden pending review.
//!
//! Reporters stay anonymous: nothing here ever tells the person reported who
//! reported them. Only the site owner (`api::audit::is_site_owner`) sees the
//! console.

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{User, DM_GUILD};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::types::Json as SqlJson;
use sqlx::PgPool;

use super::audit::is_site_owner;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

/// What people can report something as (key, label). Child safety first.
pub const CATEGORIES: &[(&str, &str)] = &[
    ("child_safety", "Child safety"),
    ("sexual", "Sexual or explicit content"),
    ("harassment", "Harassment or bullying"),
    ("hate", "Hate or threats of violence"),
    ("self_harm", "Self-harm"),
    ("spam", "Spam or scams"),
    ("illegal", "Illegal activity"),
    ("impersonation", "Impersonation"),
    ("other", "Other"),
];

pub const CHILD_SAFETY: &str = "child_safety";
const MAX_DETAILS: usize = 1000;
const MAX_NOTE: usize = 2000;
/// Context kept around a reported message: this many before and after.
const CONTEXT_EACH_SIDE: i64 = 10;
/// Reports one person can send per hour (`REPORTS_PER_HOUR`).
const DEFAULT_REPORTS_PER_HOUR: i64 = 20;
/// "Permanently" is stored as this far-off date.
const FOREVER: &str = "9999-12-31T00:00:00Z";
pub const CANT_MESSAGE: &str = "This person can't receive messages from you.";
/// What someone suspended with "Suspend now" is told (and nothing more).
pub const PENDING_REASON: &str = "Suspended pending review of a report";

/// Quiet ban-evasion hints (see `ips.rs`): not something people can report.
pub const IP_MATCH: &str = "ip_match";
/// Reports with this source are ban-evasion hints from `ips.rs`.
pub const IP_MATCH_SOURCE: &str = "ip-match";

pub(crate) fn category_label(key: &str) -> &'static str {
    if key == IP_MATCH {
        return "Same IP as a suspended or banned account";
    }
    CATEGORIES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, l)| *l)
        .unwrap_or("Other")
}

/// 0 = child safety, 1 = serious, 2 = everything else, 3 = IP-match hints.
pub fn priority(category: &str) -> i16 {
    match category {
        CHILD_SAFETY => 0,
        "sexual" | "hate" | "self_harm" | "illegal" => 1,
        IP_MATCH => 3,
        _ => 2,
    }
}

// ---- Site settings ------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SafetySettings {
    pub teen_contact_limits: bool,
    pub teen_block_adult_channels: bool,
    pub require_birth_date: bool,
}

impl Default for SafetySettings {
    fn default() -> Self {
        Self {
            teen_contact_limits: true,
            teen_block_adult_channels: true,
            require_birth_date: true,
        }
    }
}

/// The owner's safety settings (all on if they can't be read).
pub async fn settings(db: &PgPool) -> SafetySettings {
    sqlx::query_as::<_, SafetySettings>(
        "SELECT teen_contact_limits, teen_block_adult_channels, require_birth_date
         FROM site_settings WHERE id = 1",
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

// ---- Age groups ---------------------------------------------------------------

/// The age group for a date of birth (`YYYY-MM-DD`) on `today`: "teen"
/// (13–17) or "adult"; under 13s are refused. Only the group is stored.
pub fn bracket_for_birth_date(raw: &str, today: NaiveDate) -> AppResult<&'static str> {
    let dob = NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").map_err(|_| {
        AppError::BadRequest("Enter your date of birth (day, month and year).".into())
    })?;
    if dob > today || dob.year() < today.year() - 120 {
        return Err(AppError::BadRequest(
            "That date of birth doesn't look right.".into(),
        ));
    }
    let mut age = today.year() - dob.year();
    if (today.month(), today.day()) < (dob.month(), dob.day()) {
        age -= 1;
    }
    if age < 13 {
        return Err(AppError::Refused(
            "Sorry, you need to be 13 or older to use Outhouse.".into(),
        ));
    }
    Ok(if age < 18 { "teen" } else { "adult" })
}

/// The age group for a new account (sign-up asks for a date of birth unless
/// the owner switched that off; then accounts count as adults).
pub async fn signup_bracket(db: &PgPool, birth_date: Option<&str>) -> AppResult<&'static str> {
    match birth_date.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => bracket_for_birth_date(d, Utc::now().date_naive()),
        None if settings(db).await.require_birth_date => Err(AppError::BadRequest(
            "Enter your date of birth. It's only used to work out your age group; the date isn't kept."
                .into(),
        )),
        None => Ok("adult"),
    }
}

#[derive(sqlx::FromRow)]
struct AgeRow {
    id: i64,
    age_bracket: String,
    is_bot: bool,
}

async fn ages(db: &PgPool, ids: &[i64]) -> AppResult<Vec<AgeRow>> {
    Ok(
        sqlx::query_as::<_, AgeRow>("SELECT id, age_bracket, is_bot FROM users WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(db)
            .await?,
    )
}

async fn shares_outhouse(db: &PgPool, a: i64, b: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM guild_members x
                       JOIN guild_members y ON x.guild_id = y.guild_id
                       WHERE x.user_id = $1 AND y.user_id = $2)",
    )
    .bind(a)
    .bind(b)
    .fetch_one(db)
    .await?)
}

async fn shares_family_outhouse(db: &PgPool, a: i64, b: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM guild_members x
                       JOIN guild_members y ON x.guild_id = y.guild_id
                       JOIN guilds g ON g.id = x.guild_id
                       WHERE g.family AND x.user_id = $1 AND y.user_id = $2)",
    )
    .bind(a)
    .bind(b)
    .fetch_one(db)
    .await?)
}

async fn friends_of_friends(db: &PgPool, a: i64, b: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "WITH mine AS (
             SELECT CASE WHEN user_a = $1 THEN user_b ELSE user_a END AS f
             FROM friendships WHERE user_a = $1 OR user_b = $1),
         theirs AS (
             SELECT CASE WHEN user_a = $2 THEN user_b ELSE user_a END AS f
             FROM friendships WHERE user_a = $2 OR user_b = $2)
         SELECT EXISTS(SELECT 1 FROM mine JOIN theirs USING (f))",
    )
    .bind(a)
    .bind(b)
    .fetch_one(db)
    .await?)
}

/// May `other` reach the teen `teen` (friend request, or a DM between two
/// teens)? Only people they share an outhouse with who are friends of
/// friends or in a family outhouse with them.
async fn teen_may_hear_from(db: &PgPool, teen: i64, other: i64) -> AppResult<bool> {
    Ok(shares_outhouse(db, teen, other).await?
        && (friends_of_friends(db, teen, other).await?
            || shares_family_outhouse(db, teen, other).await?))
}

/// Why `me` can't start a DM with `other` under the teen rules (None = fine).
/// Adults can't start DMs with teens they aren't friends with (nor teens
/// with adults); two teens need a shared outhouse and a friend in common or
/// a family outhouse. Bots and the server's own helpers aren't affected.
pub async fn teen_dm_refusal(db: &PgPool, me: i64, other: i64) -> AppResult<Option<String>> {
    if me == other || !settings(db).await.teen_contact_limits {
        return Ok(None);
    }
    let rows = ages(db, &[me, other]).await?;
    if rows.len() < 2 || rows.iter().any(|r| r.is_bot) {
        return Ok(None);
    }
    let teen = |id: i64| rows.iter().any(|r| r.id == id && r.age_bracket == "teen");
    let (me_teen, other_teen) = (teen(me), teen(other));
    if !me_teen && !other_teen {
        return Ok(None);
    }
    if super::friends::are_friends(db, me, other).await? {
        return Ok(None);
    }
    if me_teen && other_teen && teen_may_hear_from(db, other, me).await? {
        return Ok(None);
    }
    Ok(Some(if other_teen {
        CANT_MESSAGE.to_string()
    } else {
        "You can only start DMs with adults you're friends with. Send them a friend request first."
            .to_string()
    }))
}

pub async fn check_teen_dm(db: &PgPool, me: i64, other: i64) -> AppResult<()> {
    match teen_dm_refusal(db, me, other).await? {
        Some(reason) => Err(AppError::Refused(reason)),
        None => Ok(()),
    }
}

/// Group DMs: everyone in it must be allowed to message every teen in it.
pub async fn check_group_teens(db: &PgPool, people: &[i64]) -> AppResult<()> {
    if !settings(db).await.teen_contact_limits {
        return Ok(());
    }
    let rows = ages(db, people).await?;
    let teens: Vec<i64> = rows
        .iter()
        .filter(|r| r.age_bracket == "teen" && !r.is_bot)
        .map(|r| r.id)
        .collect();
    for &teen in &teens {
        for &other in people {
            if other != teen {
                check_teen_dm(db, other, teen).await?;
            }
        }
    }
    Ok(())
}

/// Friend requests to a teen: only from people they share an outhouse with
/// who are friends of friends or in a family outhouse with them.
pub async fn check_teen_friend_request(db: &PgPool, me: i64, other: i64) -> AppResult<()> {
    if !settings(db).await.teen_contact_limits {
        return Ok(());
    }
    let rows = ages(db, &[other]).await?;
    let is_teen = rows.iter().any(|r| r.age_bracket == "teen" && !r.is_bot);
    if !is_teen || teen_may_hear_from(db, other, me).await? {
        return Ok(());
    }
    Err(AppError::Refused(
        "They aren't taking friend requests from you right now.".into(),
    ))
}

/// Is this user a teen who mustn't see age-restricted channels?
pub async fn teen_kept_from_adult_channels(db: &PgPool, user_id: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users u, site_settings s
                       WHERE u.id = $1 AND s.id = 1 AND u.age_bracket = 'teen'
                         AND NOT u.is_bot AND s.teen_block_adult_channels)",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

#[derive(Serialize)]
pub struct ContactInfo {
    pub can_message: bool,
    pub reason: Option<String>,
    /// It would go as a message request (they're not your friend and share
    /// no outhouse with you).
    pub request: bool,
}

/// `GET /api/users/{id}/contact`: can I message them right now (blocks,
/// teen rules, privacy, suspensions, message request limits), and would it
/// be a message request?
pub async fn contact(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(other): Path<i64>,
) -> AppResult<Json<ContactInfo>> {
    let (reason, request) = super::requests::contact_status(&st, user.id, other).await?;
    Ok(Json(ContactInfo {
        can_message: reason.is_none(),
        reason,
        request,
    }))
}

// ---- Suspensions --------------------------------------------------------------

fn until_label(until: DateTime<Utc>) -> String {
    if until.year() >= 9999 {
        "permanently".into()
    } else {
        format!("until {}", until.format("%-d %B %Y, %H:%M UTC"))
    }
}

/// Why this account can't sign in (None = not suspended).
pub async fn suspension_message(db: &PgPool, user_id: i64) -> AppResult<Option<String>> {
    let row: Option<(DateTime<Utc>, Option<String>, bool)> = sqlx::query_as(
        "SELECT suspended_until, suspended_reason, suspension_pending FROM users
         WHERE id = $1 AND suspended_until > now()",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(until, reason, pending)| {
        // "Suspend now" (child-safety reports): only the neutral line, never
        // the category or who reported them.
        if pending {
            return format!("This account has been {}.", PENDING_REASON.to_lowercase());
        }
        let reason = reason
            .filter(|r| !r.trim().is_empty())
            .map(|r| format!(" Reason: {r}"))
            .unwrap_or_default();
        format!(
            "This account has been suspended {}.{reason}",
            until_label(until)
        )
    }))
}

// ---- Files ----------------------------------------------------------------------

/// Quarantined files are never served (451), whoever asks.
pub async fn is_quarantined(db: &PgPool, key: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM quarantined_files WHERE storage_key = $1)")
        .bind(key)
        .fetch_one(db)
        .await
        .unwrap_or(true)
}

/// Must this file stay on disk? (An open report refers to it, or it's
/// child-safety evidence the owner hasn't released.)
pub async fn file_retained(db: &PgPool, key: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM report_files f JOIN reports r ON r.id = f.report_id
                       WHERE f.storage_key = $1
                         AND (r.status <> 'closed'
                              OR (r.category = 'child_safety' AND NOT r.evidence_released)))
             OR EXISTS(SELECT 1 FROM quarantined_files WHERE storage_key = $1)",
    )
    .bind(key)
    .fetch_one(db)
    .await
    .unwrap_or(true)
}

async fn sha256_of(st: &AppState, key: &str) -> Option<String> {
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let data = tokio::fs::read(st.config.upload_dir.join(key)).await.ok()?;
    let digest = tokio::task::spawn_blocking(move || Sha256::digest(&data))
        .await
        .ok()?;
    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}

// ---- The safety log ---------------------------------------------------------------

async fn log(
    st: &AppState,
    actor: Option<i64>,
    action: &str,
    report_id: Option<i64>,
    target_user: Option<i64>,
    details: impl Into<String>,
) {
    let details: String = details.into().chars().take(MAX_NOTE).collect();
    let result = sqlx::query(
        "INSERT INTO safety_log (id, actor_id, action, report_id, target_user_id, details)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(st.ids.next_id())
    .bind(actor)
    .bind(action)
    .bind(report_id)
    .bind(target_user)
    .bind(&details)
    .execute(&st.db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, action, "couldn't write the safety log");
    }
}

/// The safety log, for other modules (the image detector).
pub(crate) async fn log_action(
    st: &AppState,
    actor: Option<i64>,
    action: &str,
    report_id: Option<i64>,
    target_user: Option<i64>,
    details: impl Into<String>,
) {
    log(st, actor, action, report_id, target_user, details).await
}

/// A safety log entry about an outhouse (shown on it in the Outhouses directory).
pub(crate) async fn log_for_guild(
    st: &AppState,
    actor: Option<i64>,
    action: &str,
    guild_id: i64,
    target_user: Option<i64>,
    details: impl Into<String>,
) {
    let details: String = details.into().chars().take(MAX_NOTE).collect();
    let result = sqlx::query(
        "INSERT INTO safety_log (id, actor_id, action, target_user_id, details, guild_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(st.ids.next_id())
    .bind(actor)
    .bind(action)
    .bind(target_user)
    .bind(&details)
    .bind(guild_id)
    .execute(&st.db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, action, "couldn't write the safety log");
    }
}

/// The safety log entries about one outhouse, newest first.
pub(crate) async fn guild_log(st: &AppState, guild_id: i64, limit: i64) -> AppResult<Vec<Value>> {
    let rows = sqlx::query_as::<_, LogRow>(&format!(
        "{LOG_SELECT} WHERE l.guild_id = $1 ORDER BY l.created_at DESC, l.id DESC LIMIT $2"
    ))
    .bind(guild_id)
    .bind(limit)
    .fetch_all(&st.db)
    .await?;
    Ok(rows.iter().map(log_json).collect())
}

/// "Also suspend the owner" when shutting an outhouse down: the same as
/// "Suspend now" on a report (until reviewed).
pub(crate) async fn suspend_pending_from_outhouses(
    st: &AppState,
    me: i64,
    target: i64,
) -> AppResult<()> {
    let req = ActionRequest {
        action: "suspend_pending".into(),
        text: None,
        guild_id: None,
        minutes: None,
        days: None,
        hide_messages: false,
        attachment_id: None,
        bracket: None,
        exempt: None,
    };
    user_action(st, me, target, None, None, &req).await
}

/// A neutral note from Outhouse Safety (shutting an outhouse down).
pub(crate) async fn safety_note(st: &AppState, user_id: i64, content: String) -> AppResult<()> {
    safety_dm(st, user_id, content).await
}

// ---- Owners and alerts ----------------------------------------------------------

/// Everyone who counts as the site owner (see `is_site_owner`).
pub async fn site_owner_ids(db: &PgPool) -> AppResult<Vec<i64>> {
    if let Ok(list) = std::env::var("SITE_OWNER") {
        let list: Vec<String> = list
            .split(',')
            .map(|x| x.trim().to_lowercase())
            .filter(|x| !x.is_empty())
            .collect();
        if !list.is_empty() {
            return Ok(sqlx::query_scalar(
                "SELECT id FROM users
                 WHERE NOT is_bot AND (id::TEXT = ANY($1) OR lower(username) = ANY($1))",
            )
            .bind(&list)
            .fetch_all(db)
            .await?);
        }
    }
    Ok(
        sqlx::query_scalar("SELECT id FROM users WHERE NOT is_bot ORDER BY created_at, id LIMIT 1")
            .fetch_all(db)
            .await?,
    )
}

pub(crate) async fn require_owner(st: &AppState, user: &User) -> AppResult<()> {
    if user.bot || !is_site_owner(&st.db, user.id).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Counts {
    /// Reports nobody has looked at yet.
    pub open: i64,
    /// Being looked at.
    pub in_review: i64,
    /// Child-safety reports not closed yet.
    pub urgent: i64,
    /// Ban-evasion hints (same IP as a suspended or banned account) not
    /// closed yet. Quiet: not in `open`, so they don't light up the badge.
    pub ip_matches: i64,
}

async fn counts(db: &PgPool) -> AppResult<Counts> {
    Ok(sqlx::query_as::<_, Counts>(
        "SELECT count(*) FILTER (WHERE status = 'open' AND source <> 'ip-match') AS open,
                count(*) FILTER (WHERE status = 'review' AND source <> 'ip-match') AS in_review,
                count(*) FILTER (WHERE status <> 'closed' AND priority = 0) AS urgent,
                count(*) FILTER (WHERE status <> 'closed' AND source = 'ip-match') AS ip_matches
         FROM reports",
    )
    .fetch_one(db)
    .await?)
}

/// Updates the owners' badge, and for a child-safety report (`alert` = its
/// id) sends a push notification and the server's ntfy / webhook alert (no
/// details: just that there's something to review, and a nudge to suspend
/// the account at once if in doubt). The push opens that report.
fn notify_owners(st: &AppState, alert: Option<i64>) {
    let st = st.clone();
    tokio::spawn(async move {
        let (Ok(c), Ok(owners)) = (counts(&st.db).await, site_owner_ids(&st.db).await) else {
            return;
        };
        for &owner in &owners {
            st.dispatch(
                Audience::User(owner),
                ServerEvent::ReportsChanged {
                    open: c.open,
                    urgent: c.urgent,
                    alert: alert.is_some(),
                    report_id: alert,
                },
            );
        }
        let Some(report_id) = alert else {
            return;
        };
        let payload = json!({
            "title": "Urgent safety report",
            "body": format!("A child-safety report needs your review. {SUSPEND_NUDGE}"),
            "kind": "safety",
            "guild_id": "0",
            "channel_id": "reports",
            "report_id": report_id.to_string(),
            "tag": "safety-report",
            "url": format!("/app?reports=1&report={report_id}"),
        });
        for &owner in &owners {
            if let Err(e) = crate::push::send_to_user(&st, owner, &payload).await {
                tracing::warn!(error = %e, "safety push failed");
            }
        }
        external_alert(
            &st,
            "Outhouse: urgent safety report",
            &format!("A child-safety report was made and needs review. {SUSPEND_NUDGE} Open Outhouse → User settings → Reports. (No details are sent in this alert.)"),
        )
        .await;
    });
}

/// The nudge in child-safety alerts (police advice: suspend straight away).
pub const SUSPEND_NUDGE: &str = "Review and suspend the account now if in doubt.";

/// The deploy kit's alert settings (`ALERT_NTFY_URL`, `ALERT_WEBHOOK_URL`),
/// if the server was given them.
async fn external_alert(st: &AppState, title: &str, message: &str) {
    let get = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| v.starts_with("https://") || v.starts_with("http://"))
    };
    if let Some(url) = get("ALERT_NTFY_URL") {
        let sent = st
            .http
            .post(&url)
            .header("Title", title)
            .header("Priority", "high")
            .header("Tags", "rotating_light")
            .body(message.to_string())
            .send()
            .await;
        if let Err(e) = sent {
            tracing::warn!(error = %e, "couldn't send the safety alert to ntfy");
        }
    }
    if let Some(url) = get("ALERT_WEBHOOK_URL") {
        let sent = st
            .http
            .post(&url)
            .json(&json!({ "username": "Outhouse safety", "content": format!("**{title}**\n{message}") }))
            .send()
            .await;
        if let Err(e) = sent {
            tracing::warn!(error = %e, "couldn't send the safety alert to the webhook");
        }
    }
}

/// The "Outhouse Safety" account warnings and digests come from.
async fn safety_bot(st: &AppState) -> AppResult<i64> {
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT safety_bot_id FROM site_settings WHERE id = 1")
            .fetch_optional(&st.db)
            .await?
            .flatten();
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    auth::create_bot_user(&mut *tx, id, "Outhouse Safety").await?;
    sqlx::query(
        "UPDATE site_settings SET safety_bot_id = $1 WHERE id = 1 AND safety_bot_id IS NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let winner: Option<i64> =
        sqlx::query_scalar("SELECT safety_bot_id FROM site_settings WHERE id = 1")
            .fetch_one(&st.db)
            .await?;
    Ok(winner.unwrap_or(id))
}

/// A DM from Outhouse Safety.
async fn safety_dm(st: &AppState, user_id: i64, content: String) -> AppResult<()> {
    let bot = safety_bot(st).await?;
    let dm = super::dms::open_direct(st, bot, user_id).await?;
    super::messages::post_bot_message(st, DM_GUILD, dm, bot, None, None, content, Vec::new())
        .await?;
    Ok(())
}

// ---- Making a report ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct NewReport {
    /// message, attachment, user, sticker, emoji or guild
    target_kind: String,
    #[serde(with = "outhouse_common::id::as_string")]
    target_id: i64,
    category: String,
    #[serde(default)]
    details: Option<String>,
}

#[derive(Serialize)]
pub struct ReportReceipt {
    pub ok: bool,
    /// Someone (maybe you) already reported it; yours was added to that report.
    pub duplicate: bool,
}

struct FileMeta {
    key: String,
    filename: String,
    content_type: String,
    size: i64,
}

/// What a report is about, worked out when it's made.
struct Target {
    reported_user_id: Option<i64>,
    guild_id: Option<i64>,
    channel_id: Option<i64>,
    message_id: Option<i64>,
    snapshot: Value,
    files: Vec<FileMeta>,
}

#[derive(sqlx::FromRow)]
struct SnapMessage {
    id: i64,
    channel_id: i64,
    thread_id: Option<i64>,
    author_id: i64,
    author_username: String,
    author_name: String,
    author_bot: bool,
    content: String,
    created_at: DateTime<Utc>,
    edited_at: Option<DateTime<Utc>>,
    sticker: Option<SqlJson<Value>>,
    forward: Option<SqlJson<Value>>,
    guild_id: Option<i64>,
    guild_name: Option<String>,
    channel_name: String,
}

#[derive(sqlx::FromRow)]
struct SnapAttachment {
    id: i64,
    storage_key: String,
    filename: String,
    content_type: String,
    size: i64,
}

#[derive(sqlx::FromRow)]
struct ContextRow {
    id: i64,
    author_id: i64,
    author_name: String,
    content: String,
    created_at: DateTime<Utc>,
    files: i64,
}

fn short(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

async fn message_target(
    st: &AppState,
    reporter: Option<i64>,
    message_id: i64,
    focus_attachment: Option<i64>,
) -> AppResult<Target> {
    let m = sqlx::query_as::<_, SnapMessage>(
        "SELECT m.id, m.channel_id, m.thread_id, m.author_id, u.username AS author_username,
                COALESCE(m.override_name, u.display_name) AS author_name, u.is_bot AS author_bot,
                m.content, m.created_at, m.edited_at, m.sticker, m.forward,
                c.guild_id, g.name AS guild_name, c.name AS channel_name
         FROM messages m
         JOIN users u ON u.id = m.author_id
         JOIN channels c ON c.id = m.channel_id
         LEFT JOIN guilds g ON g.id = c.guild_id
         WHERE m.id = $1",
    )
    .bind(message_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    // You can only report what you can see (DMs: you're in it). The image
    // detector (no reporter) sees everything.
    if let Some(reporter) = reporter {
        crate::perms::channel_access(&st.db, m.channel_id, reporter).await?;
    }
    let attachments = sqlx::query_as::<_, SnapAttachment>(
        "SELECT id, storage_key, filename, content_type, size FROM attachments
         WHERE message_id = $1 ORDER BY id",
    )
    .bind(message_id)
    .fetch_all(&st.db)
    .await?;
    let thread_name: Option<String> = match m.thread_id {
        Some(t) => {
            sqlx::query_scalar("SELECT name FROM threads WHERE id = $1")
                .bind(t)
                .fetch_optional(&st.db)
                .await?
        }
        None => None,
    };
    let participants: Vec<String> = if m.guild_id.is_none() {
        sqlx::query_scalar(
            "SELECT u.display_name || ' (@' || u.username || ')' FROM dm_participants p
             JOIN users u ON u.id = p.user_id WHERE p.channel_id = $1 ORDER BY p.joined_at",
        )
        .bind(m.channel_id)
        .fetch_all(&st.db)
        .await?
    } else {
        Vec::new()
    };
    let context_sql = "SELECT m.id, m.author_id, COALESCE(m.override_name, u.display_name) AS author_name,
                m.content, m.created_at,
                (SELECT count(*) FROM attachments a WHERE a.message_id = m.id) AS files
         FROM messages m JOIN users u ON u.id = m.author_id
         WHERE m.channel_id = $1 AND m.thread_id IS NOT DISTINCT FROM $2::BIGINT AND NOT m.safety_hidden";
    let sql = format!(
        "({context_sql} AND m.id < $3 ORDER BY m.id DESC LIMIT $4)
         UNION ALL
         ({context_sql} AND m.id > $3 ORDER BY m.id ASC LIMIT $4)"
    );
    let mut context = sqlx::query_as::<_, ContextRow>(&sql)
        .bind(m.channel_id)
        .bind(m.thread_id)
        .bind(m.id)
        .bind(CONTEXT_EACH_SIDE)
        .fetch_all(&st.db)
        .await?;
    context.sort_by_key(|c| c.id);
    let context: Vec<Value> = context
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(),
                "author_id": c.author_id.to_string(),
                "author_name": c.author_name,
                "content": short(&c.content, 2000),
                "created_at": c.created_at,
                "files": c.files,
                "before": c.id < m.id,
            })
        })
        .collect();
    let snapshot = json!({
        "kind": "message",
        "message": {
            "id": m.id.to_string(),
            "content": m.content,
            "created_at": m.created_at,
            "edited_at": m.edited_at,
            "sticker": m.sticker.as_ref().map(|s| &s.0),
            "forward": m.forward.as_ref().map(|f| &f.0),
            "thread_id": m.thread_id.map(|t| t.to_string()),
            "author": {
                "id": m.author_id.to_string(),
                "username": m.author_username,
                "display_name": m.author_name,
                "bot": m.author_bot,
            },
            "attachments": attachments.iter().map(|a| json!({
                "id": a.id.to_string(),
                "key": a.storage_key,
                "filename": a.filename,
                "content_type": a.content_type,
                "size": a.size,
            })).collect::<Vec<_>>(),
        },
        "focus_attachment_id": focus_attachment.map(|a| a.to_string()),
        "place": {
            "guild_id": m.guild_id.map(|g| g.to_string()),
            "guild_name": m.guild_name,
            "channel_id": m.channel_id.to_string(),
            "channel_name": m.channel_name,
            "thread_name": thread_name,
            "dm": m.guild_id.is_none(),
            "participants": participants,
        },
        "context": context,
        "edits": [],
    });
    Ok(Target {
        reported_user_id: Some(m.author_id),
        guild_id: m.guild_id,
        channel_id: Some(m.channel_id),
        message_id: Some(m.id),
        snapshot,
        files: attachments
            .into_iter()
            .map(|a| FileMeta {
                key: a.storage_key,
                filename: a.filename,
                content_type: a.content_type,
                size: a.size,
            })
            .collect(),
    })
}

#[derive(sqlx::FromRow)]
struct SnapUser {
    id: i64,
    username: String,
    display_name: String,
    about_me: String,
    avatar_key: Option<String>,
    banner_key: Option<String>,
    is_bot: bool,
    created_at: DateTime<Utc>,
}

async fn user_target(st: &AppState, reporter: Option<i64>, user_id: i64) -> AppResult<Target> {
    if Some(user_id) == reporter {
        return Err(AppError::BadRequest("You can't report yourself.".into()));
    }
    let u = sqlx::query_as::<_, SnapUser>(
        "SELECT id, username, display_name, about_me, avatar_key, banner_key, is_bot, created_at
         FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let keys: Vec<String> = [u.avatar_key.clone(), u.banner_key.clone()]
        .into_iter()
        .flatten()
        .collect();
    let files: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT storage_key, filename, content_type FROM profile_files WHERE storage_key = ANY($1)",
    )
    .bind(&keys)
    .fetch_all(&st.db)
    .await?;
    let size_of = |key: &str| {
        std::fs::metadata(st.config.upload_dir.join(key))
            .map(|m| m.len() as i64)
            .unwrap_or(0)
    };
    Ok(Target {
        reported_user_id: Some(u.id),
        guild_id: None,
        channel_id: None,
        message_id: None,
        snapshot: json!({
            "kind": "user",
            "user": {
                "id": u.id.to_string(),
                "username": u.username,
                "display_name": u.display_name,
                "about_me": u.about_me,
                "bot": u.is_bot,
                "created_at": u.created_at,
                "avatar_key": u.avatar_key,
                "banner_key": u.banner_key,
            },
        }),
        files: files
            .into_iter()
            .map(|(key, filename, content_type)| FileMeta {
                size: size_of(&key),
                key,
                filename,
                content_type,
            })
            .collect(),
    })
}

#[derive(sqlx::FromRow)]
struct SnapArt {
    id: i64,
    guild_id: i64,
    guild_name: String,
    name: String,
    storage_key: String,
    content_type: String,
    creator_id: Option<i64>,
    created_at: DateTime<Utc>,
}

/// Stickers and custom emoji (both are a picture in an outhouse).
async fn art_target(st: &AppState, kind: &str, id: i64) -> AppResult<Target> {
    let table = if kind == "sticker" {
        "stickers"
    } else {
        "emojis"
    };
    let sql = format!(
        "SELECT s.id, s.guild_id, g.name AS guild_name, s.name, s.storage_key, s.content_type,
                s.creator_id, s.created_at
         FROM {table} s JOIN guilds g ON g.id = s.guild_id WHERE s.id = $1"
    );
    let a = sqlx::query_as::<_, SnapArt>(&sql)
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let size = std::fs::metadata(st.config.upload_dir.join(&a.storage_key))
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    Ok(Target {
        reported_user_id: a.creator_id,
        guild_id: Some(a.guild_id),
        channel_id: None,
        message_id: None,
        snapshot: json!({
            "kind": kind,
            "art": {
                "id": a.id.to_string(),
                "name": a.name,
                "key": a.storage_key,
                "content_type": a.content_type,
                "creator_id": a.creator_id.map(|c| c.to_string()),
                "created_at": a.created_at,
            },
            "place": { "guild_id": a.guild_id.to_string(), "guild_name": a.guild_name },
        }),
        files: vec![FileMeta {
            filename: format!("{}-{}", kind, a.name),
            key: a.storage_key,
            content_type: a.content_type,
            size,
        }],
    })
}

async fn guild_target(st: &AppState, reporter: i64, guild_id: i64) -> AppResult<Target> {
    crate::perms::member_ctx(&st.db, guild_id, reporter).await?;
    let (name, owner_id, created_at): (String, i64, DateTime<Utc>) =
        sqlx::query_as("SELECT name, owner_id, created_at FROM guilds WHERE id = $1")
            .bind(guild_id)
            .fetch_one(&st.db)
            .await?;
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM guild_members WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    let channels: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM channels WHERE guild_id = $1 ORDER BY position, id LIMIT 50",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Target {
        reported_user_id: Some(owner_id),
        guild_id: Some(guild_id),
        channel_id: None,
        message_id: None,
        snapshot: json!({
            "kind": "guild",
            "guild": {
                "id": guild_id.to_string(),
                "name": name,
                "owner_id": owner_id.to_string(),
                "created_at": created_at,
                "members": members,
                "channels": channels,
            },
            "place": { "guild_id": guild_id.to_string(), "guild_name": name },
        }),
        files: Vec::new(),
    })
}

/// `POST /api/reports` `{target_kind, target_id, category, details?}`.
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<NewReport>,
) -> AppResult<(StatusCode, Json<ReportReceipt>)> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    let category = req.category.trim();
    if !CATEGORIES.iter().any(|(k, _)| *k == category) {
        return Err(AppError::BadRequest("Pick what's wrong.".into()));
    }
    let details = req.details.unwrap_or_default().trim().to_string();
    if details.chars().count() > MAX_DETAILS {
        return Err(AppError::BadRequest(format!(
            "Details can be up to {MAX_DETAILS} characters."
        )));
    }
    let kind = req.target_kind.trim();
    if !matches!(
        kind,
        "message" | "attachment" | "user" | "sticker" | "emoji" | "guild"
    ) {
        return Err(AppError::BadRequest("You can't report that.".into()));
    }
    // A cap per person so reports can't be used to flood the owner (or
    // hide lots of messages with false child-safety reports).
    let per_hour = std::env::var("REPORTS_PER_HOUR")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_REPORTS_PER_HOUR);
    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM report_reporters
         WHERE reporter_id = $1 AND created_at > now() - interval '1 hour'",
    )
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if recent >= per_hour {
        return Err(AppError::TooManyRequests(
            "You've sent a lot of reports in the last hour. Try again a bit later. If someone is in danger right now, call 000."
                .into(),
        ));
    }

    // Already reported (and not closed yet)? Add to that report.
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM reports WHERE target_kind = $1 AND target_id = $2 AND status <> 'closed'",
    )
    .bind(kind)
    .bind(req.target_id)
    .fetch_optional(&st.db)
    .await?;
    // Check access every time (so a duplicate can't be used to probe).
    let target = match kind {
        "message" => message_target(&st, Some(user.id), req.target_id, None).await?,
        "attachment" => {
            let message_id: i64 =
                sqlx::query_scalar("SELECT message_id FROM attachments WHERE id = $1")
                    .bind(req.target_id)
                    .fetch_optional(&st.db)
                    .await?
                    .ok_or(AppError::NotFound)?;
            message_target(&st, Some(user.id), message_id, Some(req.target_id)).await?
        }
        "user" => user_target(&st, Some(user.id), req.target_id).await?,
        "sticker" | "emoji" => art_target(&st, kind, req.target_id).await?,
        _ => guild_target(&st, user.id, req.target_id).await?,
    };
    if target.reported_user_id == Some(user.id) && kind != "guild" {
        return Err(AppError::BadRequest(
            "You can't report your own things. Delete it instead.".into(),
        ));
    }
    if let Some(id) = existing {
        add_reporter(&st, id, user.id, category, &details).await?;
        return Ok((
            StatusCode::OK,
            Json(ReportReceipt {
                ok: true,
                duplicate: true,
            }),
        ));
    }

    let id = st.ids.next_id();
    let inserted: Option<i64> = sqlx::query_scalar(
        "INSERT INTO reports (id, target_kind, target_id, category, priority, reported_user_id,
                              guild_id, channel_id, message_id, snapshot)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (target_kind, target_id) WHERE status <> 'closed' DO NOTHING
         RETURNING id",
    )
    .bind(id)
    .bind(kind)
    .bind(req.target_id)
    .bind(category)
    .bind(priority(category))
    .bind(target.reported_user_id)
    .bind(target.guild_id)
    .bind(target.channel_id)
    .bind(target.message_id)
    .bind(SqlJson(target.snapshot.clone()))
    .fetch_optional(&st.db)
    .await?;
    if inserted.is_none() {
        // Someone else reported it at the same moment.
        let other: i64 = sqlx::query_scalar(
            "SELECT id FROM reports WHERE target_kind = $1 AND target_id = $2 AND status <> 'closed'",
        )
        .bind(kind)
        .bind(req.target_id)
        .fetch_one(&st.db)
        .await?;
        add_reporter(&st, other, user.id, category, &details).await?;
        return Ok((
            StatusCode::OK,
            Json(ReportReceipt {
                ok: true,
                duplicate: true,
            }),
        ));
    }
    sqlx::query(
        "INSERT INTO report_reporters (report_id, reporter_id, category, details)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(user.id)
    .bind(category)
    .bind(&details)
    .execute(&st.db)
    .await?;
    for f in &target.files {
        let hash = sha256_of(&st, &f.key).await;
        sqlx::query(
            "INSERT INTO report_files (report_id, storage_key, filename, content_type, size, sha256)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(&f.key)
        .bind(&f.filename)
        .bind(&f.content_type)
        .bind(f.size)
        .bind(hash)
        .execute(&st.db)
        .await?;
    }
    // The reported person's IP addresses stay with the report while it's
    // open, whatever the retention (see ips.rs).
    if let Some(subject) = target.reported_user_id {
        crate::ips::snapshot_for_report(&st.db, id, subject).await?;
    }
    let urgent = category == CHILD_SAFETY;
    if urgent {
        quarantine(&st, id).await?;
    }
    notify_owners(&st, urgent.then_some(id));
    // The image detector's age estimates: explicit with a face estimated
    // clearly young makes it a child-safety report, whatever was picked.
    super::detector::check_report(&st, id).await;
    Ok((
        StatusCode::CREATED,
        Json(ReportReceipt {
            ok: true,
            duplicate: false,
        }),
    ))
}

/// Another person reported the same thing: count them (once), and move the
/// report up if they picked something more serious.
async fn add_reporter(
    st: &AppState,
    report_id: i64,
    reporter: i64,
    category: &str,
    details: &str,
) -> AppResult<()> {
    let added = sqlx::query(
        "INSERT INTO report_reporters (report_id, reporter_id, category, details)
         VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(report_id)
    .bind(reporter)
    .bind(category)
    .bind(details)
    .execute(&st.db)
    .await?
    .rows_affected();
    if added == 0 {
        return Ok(());
    }
    let (before, after): (String, String) = sqlx::query_as(
        "UPDATE reports r SET report_count = report_count + 1, updated_at = now(),
                category = CASE WHEN $2 < r.priority THEN $3 ELSE r.category END,
                priority = LEAST(r.priority, $2)
         FROM (SELECT category FROM reports WHERE id = $1) old
         WHERE r.id = $1
         RETURNING old.category, r.category",
    )
    .bind(report_id)
    .bind(priority(category))
    .bind(category)
    .fetch_one(&st.db)
    .await?;
    let escalated = after == CHILD_SAFETY && before != CHILD_SAFETY;
    if escalated {
        quarantine(st, report_id).await?;
    }
    notify_owners(st, escalated.then_some(report_id));
    Ok(())
}

/// Child safety: the report's files can't be downloaded by anyone, and the
/// message (and any copies sharing those files) is hidden pending review.
async fn quarantine(st: &AppState, report_id: i64) -> AppResult<()> {
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT storage_key FROM report_files WHERE report_id = $1")
            .bind(report_id)
            .fetch_all(&st.db)
            .await?;
    sqlx::query(
        "INSERT INTO quarantined_files (storage_key, report_id)
         SELECT k, $2 FROM unnest($1::TEXT[]) AS k ON CONFLICT DO NOTHING",
    )
    .bind(&keys)
    .bind(report_id)
    .execute(&st.db)
    .await?;
    let message_id: Option<i64> =
        sqlx::query_scalar("SELECT message_id FROM reports WHERE id = $1")
            .bind(report_id)
            .fetch_one(&st.db)
            .await?;
    let hidden: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "UPDATE messages m SET safety_hidden = true
         FROM channels c
         WHERE c.id = m.channel_id AND NOT m.safety_hidden
           AND (m.id = $1 OR m.id IN (SELECT message_id FROM attachments WHERE storage_key = ANY($2)))
         RETURNING m.id, m.channel_id, c.guild_id",
    )
    .bind(message_id)
    .bind(&keys)
    .fetch_all(&st.db)
    .await?;
    for (id, channel_id, guild_id) in &hidden {
        st.dispatch(
            Audience::Channel {
                guild_id: guild_id.unwrap_or(DM_GUILD),
                channel_id: *channel_id,
            },
            ServerEvent::MessageDelete {
                id: *id,
                channel_id: *channel_id,
            },
        );
    }
    log(
        st,
        None,
        "quarantine",
        Some(report_id),
        None,
        format!(
            "Child-safety report: {} file(s) quarantined, {} message(s) hidden pending review",
            keys.len(),
            hidden.len()
        ),
    )
    .await;
    Ok(())
}

/// A picture the image detector flagged (see `api::detector`).
pub(crate) struct DetectorTarget {
    pub scan_id: i64,
    pub storage_key: String,
    /// attachment, avatar, banner, emoji or sticker
    pub kind: String,
    pub message_id: Option<i64>,
    pub uploader_id: Option<i64>,
    pub guild_id: Option<i64>,
    /// "sexual" (flagged) or "other" (couldn't be checked)
    pub category: &'static str,
    /// Score, labels and the one-line summary, kept in the snapshot.
    pub detector: Value,
    /// The age rule matched (category is child safety): why, for the
    /// snapshot (see `api::detector::AgeRule`).
    pub auto_escalation: Option<Value>,
}

/// The image detector flagged a picture: a report in the owner's console
/// (source "detector"), with the usual snapshot. Child safety only when the
/// age rule matched (explicit, with a face estimated clearly young): then
/// it's quarantined and the owner alerted like any child-safety report.
pub(crate) async fn detector_report(st: &AppState, t: DetectorTarget) -> AppResult<()> {
    let target = match t.kind.as_str() {
        super::detector::KIND_ATTACHMENT => {
            let Some(message_id) = t.message_id else {
                return Ok(());
            };
            let attachment: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM attachments WHERE message_id = $1 AND storage_key = $2 LIMIT 1",
            )
            .bind(message_id)
            .bind(&t.storage_key)
            .fetch_optional(&st.db)
            .await?;
            match message_target(st, None, message_id, attachment).await {
                Ok(target) => target,
                // The message is gone already: nothing left to hold.
                Err(AppError::NotFound) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        super::detector::KIND_AVATAR | super::detector::KIND_BANNER => {
            let Some(uploader) = t.uploader_id else {
                return Ok(());
            };
            user_target(st, None, uploader).await?
        }
        super::detector::KIND_GUILD_ICON | super::detector::KIND_GUILD_BANNER => {
            let Some(uploader) = t.uploader_id else {
                return Ok(());
            };
            match t.guild_id {
                // The outhouse it's on, with the person who put it there.
                Some(guild_id) => match guild_target(st, uploader, guild_id).await {
                    Ok(mut target) => {
                        target.reported_user_id = Some(uploader);
                        target
                    }
                    Err(_) => user_target(st, None, uploader).await?,
                },
                None => user_target(st, None, uploader).await?,
            }
        }
        kind @ (super::detector::KIND_EMOJI | super::detector::KIND_STICKER) => {
            let table = if kind == "emoji" {
                "emojis"
            } else {
                "stickers"
            };
            let id: Option<i64> =
                sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE storage_key = $1"))
                    .bind(&t.storage_key)
                    .fetch_optional(&st.db)
                    .await?;
            let Some(id) = id else {
                return Ok(());
            };
            art_target(st, kind, id).await?
        }
        _ => return Ok(()),
    };
    let mut snapshot = target.snapshot;
    snapshot["detector"] = t.detector.clone();
    snapshot["detector"]["kind"] = json!(t.kind);
    if let Some(info) = &t.auto_escalation {
        snapshot["auto_escalation"] = info.clone();
    }
    let id = st.ids.next_id();
    let inserted: Option<i64> = sqlx::query_scalar(
        "INSERT INTO reports (id, target_kind, target_id, category, priority, reported_user_id,
                              guild_id, channel_id, message_id, snapshot, source, report_count)
         VALUES ($1, 'file', $2, $3, $4, $5, $6, $7, $8, $9, 'detector', 0)
         ON CONFLICT (target_kind, target_id) WHERE status <> 'closed' DO NOTHING
         RETURNING id",
    )
    .bind(id)
    .bind(t.scan_id)
    .bind(t.category)
    .bind(priority(t.category))
    .bind(target.reported_user_id.or(t.uploader_id))
    .bind(target.guild_id.or(t.guild_id))
    .bind(target.channel_id)
    .bind(target.message_id)
    .bind(SqlJson(snapshot))
    .fetch_optional(&st.db)
    .await?;
    if inserted.is_none() {
        return Ok(()); // already reported
    }
    if let Some(subject) = target.reported_user_id.or(t.uploader_id) {
        crate::ips::snapshot_for_report(&st.db, id, subject).await?;
    }
    // Only the flagged file (the report keeps it on disk while it's open).
    if let Some(f) = target.files.iter().find(|f| f.key == t.storage_key) {
        let hash = sha256_of(st, &f.key).await;
        sqlx::query(
            "INSERT INTO report_files (report_id, storage_key, filename, content_type, size, sha256)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(&f.key)
        .bind(&f.filename)
        .bind(&f.content_type)
        .bind(f.size)
        .bind(hash)
        .execute(&st.db)
        .await?;
    }
    if t.category == CHILD_SAFETY {
        log(
            st,
            None,
            "auto_child_safety",
            Some(id),
            target.reported_user_id.or(t.uploader_id),
            format!(
                "{} (the age is a {}). Child-safety report made automatically.",
                t.detector["summary"].as_str().unwrap_or("Automatic"),
                super::detector::AGE_NOTE
            ),
        )
        .await;
        quarantine(st, id).await?;
        notify_owners(st, Some(id));
        return Ok(());
    }
    log(
        st,
        None,
        "detector",
        Some(id),
        target.reported_user_id.or(t.uploader_id),
        t.detector["summary"]
            .as_str()
            .unwrap_or("Flagged by the image detector")
            .to_string(),
    )
    .await;
    notify_owners(st, None);
    Ok(())
}

/// The image detector's age rule matched a file in this open report
/// (explicit, with a face estimated clearly young): it becomes a
/// child-safety report whatever category was picked (quarantine, hidden
/// messages, urgent alert), and the safety log says it was automatic.
/// Nothing happens to a report that's already about child safety.
pub(crate) async fn auto_child_safety(
    st: &AppState,
    report_id: i64,
    info: &Value,
) -> AppResult<()> {
    let row: Option<(String, Option<i64>)> = sqlx::query_as(
        "UPDATE reports r SET category = $2, priority = $3, updated_at = now(),
                snapshot = jsonb_set(r.snapshot, '{auto_escalation}', $4)
         FROM (SELECT id, category FROM reports WHERE id = $1) old
         WHERE r.id = old.id AND r.category <> $2 AND r.status <> 'closed'
         RETURNING old.category, r.reported_user_id",
    )
    .bind(report_id)
    .bind(CHILD_SAFETY)
    .bind(priority(CHILD_SAFETY))
    .bind(SqlJson(info))
    .fetch_optional(&st.db)
    .await?;
    let Some((before, subject)) = row else {
        return Ok(());
    };
    log(
        st,
        None,
        "auto_child_safety",
        Some(report_id),
        subject,
        format!(
            "Automatically re-classified from “{}” to child safety: {} (the age is a {}).",
            category_label(&before),
            info["line"]
                .as_str()
                .unwrap_or("explicit content with a young-looking face"),
            super::detector::AGE_NOTE
        ),
    )
    .await;
    quarantine(st, report_id).await?;
    notify_owners(st, Some(report_id));
    Ok(())
}

/// A ban-evasion hint (see `ips.rs`): `user_id` signed up or signed in from
/// an address that `m` (suspended, or banned from an outhouse) used within
/// the retention period. A quiet, low-priority notice in the console: no
/// push, no alert, nothing done to anyone. The address itself isn't in the
/// snapshot; the report's copy of both accounts' rows for it is in
/// `report_ips` (for the evidence package), under the normal retention.
pub(crate) async fn ip_match_report(
    st: &AppState,
    user_id: i64,
    m: &crate::ips::MatchRow,
    ip: std::net::IpAddr,
    trigger: crate::ips::Kind,
) -> AppResult<i64> {
    let who = |id: i64| async move {
        sqlx::query_as::<_, (String, String, DateTime<Utc>, String)>(
            "SELECT username, display_name, created_at, age_bracket FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&st.db)
        .await
    };
    let (username, display_name, created_at, bracket) = who(user_id).await?;
    let (m_username, m_display_name, m_created_at, m_bracket) = who(m.id).await?;
    let snapshot = json!({
        "kind": "ip-match",
        "trigger": trigger.as_str(),
        "account": {
            "id": user_id.to_string(), "username": username, "display_name": display_name,
            "created_at": created_at, "age_bracket": bracket,
        },
        "matched": {
            "id": m.id.to_string(), "username": m_username, "display_name": m_display_name,
            "created_at": m_created_at, "age_bracket": m_bracket,
            "suspended": m.suspended, "suspended_until": m.suspended_until,
            "suspended_label": m.suspended_until.map(until_label),
            "banned_in": m.banned_in, "last_seen_on_ip": m.last_seen,
        },
        "seen_at": Utc::now(),
        "note": "Shared IP addresses are common (households, mobile networks and VPNs), so this is a hint, not proof.",
    });
    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO reports (id, target_kind, target_id, category, priority, reported_user_id,
                              snapshot, source, report_count)
         VALUES ($1, 'ip-match', $1, $2, $3, $4, $5, $6, 0)",
    )
    .bind(id)
    .bind(IP_MATCH)
    .bind(priority(IP_MATCH))
    .bind(user_id)
    .bind(SqlJson(snapshot))
    .bind(IP_MATCH_SOURCE)
    .execute(&st.db)
    .await?;
    sqlx::query(
        "INSERT INTO report_ips (report_id, user_id, ip, kind, first_seen, last_seen, count)
         SELECT $1, user_id, ip, kind, first_seen, last_seen, count FROM user_ips
         WHERE ip = $2::INET AND user_id IN ($3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(ip.to_string())
    .bind(user_id)
    .bind(m.id)
    .execute(&st.db)
    .await?;
    let why = match (m.suspended, m.banned_in.is_empty()) {
        (true, true) => "suspended".to_string(),
        (true, false) => format!("suspended; banned in {}", m.banned_in.join(", ")),
        (false, _) => format!("banned in {}", m.banned_in.join(", ")),
    };
    log(
        st,
        None,
        "ip_match",
        Some(id),
        Some(user_id),
        format!(
            "{} from the same IP address as @{m_username} ({why}). A hint, not proof.",
            if trigger == crate::ips::Kind::Register {
                "Signed up"
            } else {
                "Signed in"
            }
        ),
    )
    .await;
    notify_owners(st, None);
    Ok(id)
}

/// A reported message was edited: keep what it said before and after.
pub async fn note_edit(st: &AppState, message_id: i64, before: &str, after: &str) {
    let entry = json!([{ "at": Utc::now(), "before": before, "after": after }]);
    let result = sqlx::query(
        "UPDATE reports
         SET snapshot = jsonb_set(snapshot, '{edits}', COALESCE(snapshot->'edits', '[]'::jsonb) || $2),
             updated_at = now()
         WHERE message_id = $1 AND status <> 'closed'",
    )
    .bind(message_id)
    .bind(SqlJson(entry))
    .execute(&st.db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "couldn't record an edit of a reported message");
    }
}

/// A reported message was deleted: note when and by whom.
pub async fn note_deleted(st: &AppState, message_id: i64, by: i64) {
    let result = sqlx::query(
        "UPDATE reports
         SET snapshot = jsonb_set(snapshot, '{deleted}', $2), updated_at = now()
         WHERE message_id = $1 AND status <> 'closed'",
    )
    .bind(message_id)
    .bind(SqlJson(json!({ "at": Utc::now(), "by": by.to_string() })))
    .execute(&st.db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "couldn't note a reported message's deletion");
    }
}

// ---- The owner's console ----------------------------------------------------------------

/// `GET /api/safety/summary`: counts for the owner's badge.
pub async fn summary(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Counts>> {
    require_owner(&st, &user).await?;
    Ok(Json(counts(&st.db).await?))
}

#[derive(sqlx::FromRow)]
struct ReportRow {
    id: i64,
    target_kind: String,
    target_id: i64,
    category: String,
    priority: i16,
    status: String,
    reported_user_id: Option<i64>,
    guild_id: Option<i64>,
    message_id: Option<i64>,
    snapshot: SqlJson<Value>,
    report_count: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
    resolution: Option<String>,
    evidence_released: bool,
    reported_username: Option<String>,
    reported_name: Option<String>,
    closed_by_name: Option<String>,
    source: String,
}

const REPORT_SELECT: &str = "SELECT r.id, r.target_kind, r.target_id, r.category, r.priority,
        r.status, r.reported_user_id, r.guild_id, r.message_id, r.snapshot, r.report_count,
        r.created_at, r.updated_at, r.closed_at, r.resolution, r.evidence_released, r.source,
        u.username AS reported_username, u.display_name AS reported_name,
        cb.display_name AS closed_by_name
    FROM reports r
    LEFT JOIN users u ON u.id = r.reported_user_id
    LEFT JOIN users cb ON cb.id = r.closed_by";

fn place_label(snap: &Value) -> String {
    let p = &snap["place"];
    if p["dm"].as_bool() == Some(true) {
        let people = p["participants"].as_array().map(|a| a.len()).unwrap_or(0);
        return if people > 2 {
            "Group DM".into()
        } else {
            "Direct message".into()
        };
    }
    let guild = p["guild_name"].as_str().unwrap_or("");
    match (p["channel_name"].as_str(), p["thread_name"].as_str()) {
        (Some(c), Some(t)) => format!("{guild} › #{c} › {t}"),
        (Some(c), None) => format!("{guild} › #{c}"),
        _ => guild.to_string(),
    }
}

fn preview(snap: &Value) -> String {
    match snap["kind"].as_str() {
        Some("message") => {
            let m = &snap["message"];
            let text = m["content"].as_str().unwrap_or("").trim();
            let files = m["attachments"].as_array().map(|a| a.len()).unwrap_or(0);
            match (text.is_empty(), files) {
                (false, _) => short(text, 140),
                (true, 0) if !m["sticker"].is_null() => "(sticker)".into(),
                (true, 0) => "(no text)".into(),
                (true, n) => format!("({n} file{})", if n == 1 { "" } else { "s" }),
            }
        }
        Some("user") => format!(
            "Profile of {}",
            snap["user"]["display_name"].as_str().unwrap_or("someone")
        ),
        Some("sticker") => format!("Sticker :{}:", snap["art"]["name"].as_str().unwrap_or("")),
        Some("emoji") => format!("Emoji :{}:", snap["art"]["name"].as_str().unwrap_or("")),
        Some("guild") => format!("Outhouse {}", snap["guild"]["name"].as_str().unwrap_or("")),
        Some("ip-match") => {
            let m = &snap["matched"];
            let mut why = Vec::new();
            if m["suspended"].as_bool() == Some(true) {
                why.push("suspended".to_string());
            }
            if let Some(b) = m["banned_in"].as_array().filter(|b| !b.is_empty()) {
                let names: Vec<&str> = b.iter().filter_map(|n| n.as_str()).collect();
                why.push(format!("banned in {}", names.join(", ")));
            }
            format!(
                "{} from the same IP address as {} (@{}, {}). A hint, not proof.",
                if snap["trigger"] == "register" {
                    "Signed up"
                } else {
                    "Signed in"
                },
                m["display_name"].as_str().unwrap_or("someone"),
                m["username"].as_str().unwrap_or(""),
                why.join("; ")
            )
        }
        _ => String::new(),
    }
}

fn report_json(r: &ReportRow) -> Value {
    json!({
        "id": r.id.to_string(),
        "target_kind": r.target_kind,
        "target_id": r.target_id.to_string(),
        "category": r.category,
        "category_label": category_label(&r.category),
        "child_safety": r.category == CHILD_SAFETY,
        "priority": r.priority,
        "status": r.status,
        "report_count": r.report_count,
        "created_at": r.created_at,
        "updated_at": r.updated_at,
        "closed_at": r.closed_at,
        "closed_by_name": r.closed_by_name,
        "resolution": r.resolution,
        "evidence_released": r.evidence_released,
        "guild_id": r.guild_id.map(|g| g.to_string()),
        "message_id": r.message_id.map(|m| m.to_string()),
        "reported_user": r.reported_user_id.map(|id| json!({
            "id": id.to_string(),
            "username": r.reported_username,
            "display_name": r.reported_name,
        })),
        "place": place_label(&r.snapshot.0),
        "preview": if let Some(line) = r.snapshot.0["auto_escalation"]["line"].as_str() {
            // Automatic child-safety escalation: no content in the preview.
            line.to_string()
        } else if r.source == "detector" {
            let line = r.snapshot.0["detector"]["summary"].as_str().unwrap_or("Flagged by the image detector");
            format!("{line} · {}", preview(&r.snapshot.0))
        } else {
            preview(&r.snapshot.0)
        },
        "source": r.source,
        "auto_escalation": r.snapshot.0["auto_escalation"].is_object(),
    })
}

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

/// `GET /api/safety/reports?status=open|review|closed`: most serious first,
/// then newest.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let status = match q.status.as_deref().unwrap_or("open") {
        s @ ("open" | "review" | "closed") => s.to_string(),
        _ => return Err(AppError::BadRequest("Unknown status.".into())),
    };
    let order = if status == "closed" {
        "r.closed_at DESC NULLS LAST, r.id DESC"
    } else {
        "r.priority, r.created_at DESC"
    };
    let sql = format!("{REPORT_SELECT} WHERE r.status = $1 ORDER BY {order} LIMIT 200");
    let rows = sqlx::query_as::<_, ReportRow>(&sql)
        .bind(&status)
        .fetch_all(&st.db)
        .await?;
    let c = counts(&st.db).await?;
    Ok(Json(json!({
        "reports": rows.iter().map(report_json).collect::<Vec<_>>(),
        "counts": c,
        "categories": CATEGORIES.iter().map(|(k, l)| json!({ "key": k, "label": l })).collect::<Vec<_>>(),
    })))
}

async fn load_report(st: &AppState, id: i64) -> AppResult<ReportRow> {
    let sql = format!("{REPORT_SELECT} WHERE r.id = $1");
    sqlx::query_as::<_, ReportRow>(&sql)
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)
}

#[derive(sqlx::FromRow)]
struct FileRow {
    storage_key: String,
    filename: String,
    content_type: String,
    size: i64,
    sha256: Option<String>,
    quarantined: bool,
}

async fn report_files(st: &AppState, r: &ReportRow) -> AppResult<Vec<Value>> {
    let files = sqlx::query_as::<_, FileRow>(
        "SELECT f.storage_key, f.filename, f.content_type, f.size, f.sha256,
                EXISTS(SELECT 1 FROM quarantined_files q WHERE q.storage_key = f.storage_key) AS quarantined
         FROM report_files f WHERE f.report_id = $1 ORDER BY f.filename",
    )
    .bind(r.id)
    .fetch_all(&st.db)
    .await?;
    let child = r.category == CHILD_SAFETY;
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let on_disk = st.config.upload_dir.join(&f.storage_key).is_file();
        // The image detector's result, as metadata only (it scans files it
        // hasn't seen, quarantined ones included; nothing is shown).
        let detector = if on_disk {
            super::detector::file_summary(st, &f.storage_key, &f.content_type).await
        } else {
            json!({ "status": "skipped", "summary": "Detector: the file is gone", "near_zero": false })
        };
        let teen_hold = !super::detector::owner_may_view(&st.db, &f.storage_key).await;
        out.push(json!({
            "key": f.storage_key,
            "filename": f.filename,
            "content_type": f.content_type,
            "size": f.size,
            "sha256": f.sha256,
            "quarantined": f.quarantined,
            "on_disk": on_disk,
            // Never for child-safety reports (or anything quarantined), nor
            // pictures the detector holds in a teen context.
            "viewable": !child && !f.quarantined && on_disk && !teen_hold,
            "teen_hold": teen_hold,
            "detector": detector,
        }));
    }
    Ok(out)
}

#[derive(sqlx::FromRow)]
struct LogRow {
    id: i64,
    actor_id: Option<i64>,
    actor_name: Option<String>,
    action: String,
    report_id: Option<i64>,
    target_user_id: Option<i64>,
    target_name: Option<String>,
    details: String,
    created_at: DateTime<Utc>,
}

fn log_json(l: &LogRow) -> Value {
    json!({
        "id": l.id.to_string(),
        "actor_id": l.actor_id.map(|a| a.to_string()),
        "actor_name": l.actor_name.clone().unwrap_or_else(|| "Outhouse".into()),
        "action": l.action,
        "report_id": l.report_id.map(|r| r.to_string()),
        "target_user_id": l.target_user_id.map(|t| t.to_string()),
        "target_name": l.target_name,
        "details": l.details,
        "created_at": l.created_at,
    })
}

const LOG_SELECT: &str = "SELECT l.id, l.actor_id, a.display_name AS actor_name, l.action,
        l.report_id, l.target_user_id, t.display_name AS target_name, l.details, l.created_at
    FROM safety_log l
    LEFT JOIN users a ON a.id = l.actor_id
    LEFT JOIN users t ON t.id = l.target_user_id";

#[derive(sqlx::FromRow)]
struct SubjectRow {
    id: i64,
    username: String,
    display_name: String,
    created_at: DateTime<Utc>,
    is_bot: bool,
    age_bracket: String,
    suspended_until: Option<DateTime<Utc>>,
    suspended_reason: Option<String>,
    suspended: bool,
    suspension_pending: bool,
    suspended_at: Option<DateTime<Utc>>,
    suspended_by: Option<i64>,
    suspended_by_name: Option<String>,
    ip_block_exempt: bool,
}

/// Someone's history for the console.
async fn subject_json(st: &AppState, user_id: i64) -> AppResult<Value> {
    let u = sqlx::query_as::<_, SubjectRow>(
        "SELECT u.id, u.username, u.display_name, u.created_at, u.is_bot, u.age_bracket,
                u.suspended_until, u.suspended_reason,
                COALESCE(u.suspended_until > now(), false) AS suspended,
                u.suspension_pending, u.suspended_at, u.suspended_by,
                b.display_name AS suspended_by_name, u.ip_block_exempt
         FROM users u LEFT JOIN users b ON b.id = u.suspended_by WHERE u.id = $1",
    )
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let outhouses: Vec<(i64, String, bool, bool, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT g.id, g.name, g.owner_id = $1, g.family,
                CASE WHEN m.timeout_until > now() THEN m.timeout_until END
         FROM guild_members m JOIN guilds g ON g.id = m.guild_id
         WHERE m.user_id = $1 ORDER BY lower(g.name)",
    )
    .bind(user_id)
    .fetch_all(&st.db)
    .await?;
    let (messages_30d, hidden): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE created_at > now() - interval '30 days'),
                count(*) FILTER (WHERE safety_hidden)
         FROM messages WHERE author_id = $1",
    )
    .bind(user_id)
    .fetch_one(&st.db)
    .await?;
    let reports: Vec<(i64, String, String, DateTime<Utc>, i32)> = sqlx::query_as(
        "SELECT id, category, status, created_at, report_count FROM reports
         WHERE reported_user_id = $1 AND source <> 'ip-match' ORDER BY created_at DESC LIMIT 50",
    )
    .bind(user_id)
    .fetch_all(&st.db)
    .await?;
    let reports_made: i64 =
        sqlx::query_scalar("SELECT count(*) FROM report_reporters WHERE reporter_id = $1")
            .bind(user_id)
            .fetch_one(&st.db)
            .await?;
    let bots: i64 = sqlx::query_scalar("SELECT count(*) FROM bots WHERE owner_id = $1")
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    let sql = format!("{LOG_SELECT} WHERE l.target_user_id = $1 ORDER BY l.id DESC LIMIT 100");
    let log = sqlx::query_as::<_, LogRow>(&sql)
        .bind(user_id)
        .fetch_all(&st.db)
        .await?;
    // Every name they've gone by (the console sees all of it).
    let names = super::profiles::name_history(&st.db, user_id, None, 200).await?;
    let message_requests = super::requests::stats_for(&st.db, user_id).await?;
    let ip_blocks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ip_blocks WHERE source_user_id = $1 AND removed_at IS NULL
           AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(user_id)
    .fetch_one(&st.db)
    .await?;
    let (email, email_verified, email_pending): (Option<String>, bool, Option<String>) =
        sqlx::query_as(
            "SELECT email, email_verified_at IS NOT NULL, email_pending FROM users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    let mut sign_in = super::twofactor::owner_summary(&st.db, user_id).await?;
    sign_in["email"] = json!(email.as_deref().map(super::recovery::mask_email));
    sign_in["email_verified"] = json!(email_verified);
    sign_in["email_pending"] = json!(email_pending.as_deref().map(super::recovery::mask_email));
    Ok(json!({
        "id": u.id.to_string(),
        "username": u.username,
        "display_name": u.display_name,
        "bot": u.is_bot,
        "created_at": u.created_at,
        "account_age_days": (Utc::now() - u.created_at).num_days(),
        "age_bracket": u.age_bracket,
        "site_owner": is_site_owner(&st.db, u.id).await?,
        "suspended": u.suspended,
        "suspended_until": if u.suspended { u.suspended_until } else { None },
        "suspended_label": if !u.suspended { None } else if u.suspension_pending {
            Some("until reviewed".to_string())
        } else {
            u.suspended_until.map(until_label)
        },
        "suspended_reason": if u.suspended { u.suspended_reason } else { None },
        // "Suspend now": until the owner has reviewed it.
        "suspension_pending": u.suspended && u.suspension_pending,
        "suspended_at": if u.suspended { u.suspended_at } else { None },
        "suspended_by": if u.suspended { u.suspended_by.map(|b| b.to_string()) } else { None },
        "suspended_by_name": if u.suspended { u.suspended_by_name } else { None },
        "suspended_permanently": u.suspended && !u.suspension_pending
            && u.suspended_until.is_some_and(|t| t.year() >= 9999),
        // Blocked networks (ipblocks.rs).
        "ip_block_exempt": u.ip_block_exempt,
        "ip_blocks": ip_blocks,
        "blocked_attempts": super::ipblocks::attempts_for_user(&st.db, user_id).await?,
        "outhouses": outhouses.iter().map(|(id, name, owner, family, timeout)| json!({
            "id": id.to_string(), "name": name, "owner": owner, "family": family, "timeout_until": timeout,
        })).collect::<Vec<_>>(),
        "messages_30d": messages_30d,
        "hidden_messages": hidden,
        "bots": bots,
        "reports": reports.iter().map(|(id, category, status, at, count)| json!({
            "id": id.to_string(), "category": category, "category_label": category_label(category),
            "status": status, "created_at": at, "report_count": count,
        })).collect::<Vec<_>>(),
        "reports_made": reports_made,
        "ip_note": ip_note(),
        "log": log.iter().map(log_json).collect::<Vec<_>>(),
        // Sign-in and recovery (People): masked email, two-factor.
        "sign_in": sign_in,
        "names": names,
        "message_requests": message_requests,
    }))
}

/// The line shown with IP addresses in the console.
fn ip_note() -> String {
    format!(
        "IPs are kept {} days (configurable). Shared IPs are common — households, mobile networks and VPNs — so a match is a hint, not proof.",
        crate::ips::retention_days()
    )
}

async fn detail_json(st: &AppState, id: i64) -> AppResult<Value> {
    let r = load_report(st, id).await?;
    let reporters: Vec<(
        i64,
        Option<String>,
        Option<String>,
        String,
        String,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT rr.reporter_id, u.username, u.display_name, rr.category, rr.details, rr.created_at
             FROM report_reporters rr LEFT JOIN users u ON u.id = rr.reporter_id
             WHERE rr.report_id = $1 ORDER BY rr.created_at",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let sql = format!("{LOG_SELECT} WHERE l.report_id = $1 ORDER BY l.id DESC LIMIT 100");
    let log = sqlx::query_as::<_, LogRow>(&sql)
        .bind(id)
        .fetch_all(&st.db)
        .await?;
    let subject = match r.reported_user_id {
        Some(u) => subject_json(st, u).await.ok(),
        None => None,
    };
    let message_live: bool = match r.message_id {
        Some(m) => {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages WHERE id = $1)")
                .bind(m)
                .fetch_one(&st.db)
                .await?
        }
        None => false,
    };
    let message_hidden: bool = match r.message_id {
        Some(m) => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = $1 AND safety_hidden)",
            )
            .bind(m)
            .fetch_one(&st.db)
            .await?
        }
        None => false,
    };
    let detector = if r.source == "detector" && r.target_kind == "file" {
        super::detector::report_panel(&st.db, r.target_id).await
    } else {
        None
    };
    Ok(json!({
        "report": report_json(&r),
        "detector": detector,
        "snapshot": r.snapshot.0,
        "files": report_files(st, &r).await?,
        "message_live": message_live,
        "message_hidden": message_hidden,
        "reporters": reporters.iter().map(|(rid, username, name, category, details, at)| json!({
            "id": rid.to_string(),
            "username": username,
            "display_name": name,
            "category": category,
            "category_label": category_label(category),
            "details": details,
            "created_at": at,
        })).collect::<Vec<_>>(),
        "subject": subject,
        "log": log.iter().map(log_json).collect::<Vec<_>>(),
        // One-time police download links (police.rs); never the tokens.
        "police_links": super::police::links_json(st, id).await?,
    }))
}

/// `GET /api/safety/reports/{id}`
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    // Opening it: the files' age estimates (and the age rule) are checked.
    let open: Option<bool> =
        sqlx::query_scalar("SELECT status <> 'closed' FROM reports WHERE id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?;
    if open == Some(true) {
        super::detector::check_report(&st, id).await;
    }
    Ok(Json(detail_json(&st, id).await?))
}

/// `GET /api/safety/reports/{id}/files/{key}`: a reported file, for the
/// owner, unless it's child-safety evidence (never shown) or quarantined.
pub async fn report_file(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, key)): Path<(i64, String)>,
) -> AppResult<Response> {
    require_owner(&st, &user).await?;
    let r = load_report(&st, id).await?;
    if r.category == CHILD_SAFETY {
        return Err(AppError::Removed);
    }
    let file: Option<(String, String)> = sqlx::query_as(
        "SELECT filename, content_type FROM report_files WHERE report_id = $1 AND storage_key = $2",
    )
    .bind(id)
    .bind(&key)
    .fetch_optional(&st.db)
    .await?;
    let (filename, content_type) = file.ok_or(AppError::NotFound)?;
    // A picture held in a teen context is never shown, not even here.
    if !super::detector::owner_may_view(&st.db, &key).await {
        return Err(AppError::Held);
    }
    crate::files::send_file(&st, &key, &filename, &content_type).await
}

// ---- Actions ----------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct ActionRequest {
    action: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default, with = "outhouse_common::id::option_as_string")]
    guild_id: Option<i64>,
    #[serde(default)]
    minutes: Option<i64>,
    /// Suspensions: how many days (none = permanently).
    #[serde(default)]
    days: Option<i64>,
    #[serde(default)]
    hide_messages: bool,
    #[serde(default, with = "outhouse_common::id::option_as_string")]
    attachment_id: Option<i64>,
    #[serde(default)]
    bracket: Option<String>,
    /// "Allow from blocked networks"
    #[serde(default)]
    exempt: Option<bool>,
}

fn clean_text(text: &Option<String>, max: usize) -> String {
    text.as_deref()
        .unwrap_or("")
        .trim()
        .chars()
        .take(max)
        .collect()
}

/// `POST /api/safety/reports/{id}/actions`
pub async fn report_action(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<ActionRequest>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let r = load_report(&st, id).await?;
    let me = user.id;
    match req.action.as_str() {
        "review" => {
            sqlx::query("UPDATE reports SET status = 'review', updated_at = now() WHERE id = $1 AND status = 'open'")
                .bind(id)
                .execute(&st.db)
                .await?;
            log(
                &st,
                Some(me),
                "review",
                Some(id),
                r.reported_user_id,
                "Marked as in review",
            )
            .await;
        }
        "reopen" => {
            let result = sqlx::query(
                "UPDATE reports SET status = 'review', closed_at = NULL, closed_by = NULL,
                        resolution = NULL, updated_at = now()
                 WHERE id = $1 AND status = 'closed'",
            )
            .bind(id)
            .execute(&st.db)
            .await;
            match result {
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                    return Err(AppError::Conflict(
                        "There's already an open report about this. Look at that one instead."
                            .into(),
                    ))
                }
                other => {
                    other?;
                }
            }
            log(&st, Some(me), "reopen", Some(id), r.reported_user_id, "").await;
        }
        "dismiss" | "close" => {
            if r.status == "closed" {
                return Err(AppError::BadRequest(
                    "That report is already closed.".into(),
                ));
            }
            let dismiss = req.action == "dismiss";
            let note = clean_text(&req.text, 500);
            let resolution = match (dismiss, note.is_empty()) {
                (true, true) => "Dismissed".to_string(),
                (true, false) => format!("Dismissed: {note}"),
                (false, true) => "Closed".to_string(),
                (false, false) => note.clone(),
            };
            sqlx::query(
                "UPDATE reports SET status = 'closed', closed_at = now(), closed_by = $2,
                        resolution = $3, updated_at = now(),
                        evidence_released = evidence_released OR $4
                 WHERE id = $1",
            )
            .bind(id)
            .bind(me)
            .bind(&resolution)
            .bind(dismiss)
            .execute(&st.db)
            .await?;
            if dismiss {
                release_quarantine(&st, &r).await?;
            }
            // Files only kept for this report can go now (child-safety
            // evidence stays until it's deleted on purpose).
            let keys: Vec<String> =
                sqlx::query_scalar("SELECT storage_key FROM report_files WHERE report_id = $1")
                    .bind(id)
                    .fetch_all(&st.db)
                    .await?;
            super::messages::remove_unused_files(&st, keys.clone()).await;
            log(
                &st,
                Some(me),
                if dismiss { "dismiss" } else { "close" },
                Some(id),
                r.reported_user_id,
                resolution,
            )
            .await;
            if dismiss && r.category == CHILD_SAFETY {
                if r.snapshot.0["auto_escalation"].is_object() {
                    log(
                        &st,
                        Some(me),
                        "auto_child_safety_dismissed",
                        Some(id),
                        r.reported_user_id,
                        "Automatic age escalation dismissed as a mistake: the file isn't quarantined any more and won't be escalated automatically again",
                    )
                    .await;
                }
                // Back to the detector's normal handling (a held picture
                // gets its normal detector report again).
                super::detector::escalation_dismissed(&st, &keys).await;
            }
        }
        "delete_message" => {
            let m = r
                .message_id
                .ok_or_else(|| AppError::BadRequest("This report isn't about a message.".into()))?;
            if !delete_message(&st, m).await? {
                return Err(AppError::BadRequest("That message is already gone.".into()));
            }
            log(
                &st,
                Some(me),
                "delete_message",
                Some(id),
                r.reported_user_id,
                "Deleted the reported message (the snapshot is kept)",
            )
            .await;
        }
        "remove_attachment" => {
            let m = r
                .message_id
                .ok_or_else(|| AppError::BadRequest("This report isn't about a message.".into()))?;
            let removed = remove_attachments(&st, m, req.attachment_id).await?;
            if removed == 0 {
                return Err(AppError::BadRequest(
                    "There are no files left to remove.".into(),
                ));
            }
            log(
                &st,
                Some(me),
                "remove_attachment",
                Some(id),
                r.reported_user_id,
                format!("Removed {removed} file(s) from the message"),
            )
            .await;
        }
        "approve_file" | "keep_blurred" | "remove_file" => {
            if r.target_kind != "file" {
                return Err(AppError::BadRequest(
                    "That's only for pictures the image detector flagged.".into(),
                ));
            }
            let what = super::detector::owner_decision(&st, r.target_id, &req.action).await?;
            if r.status != "closed" {
                sqlx::query(
                    "UPDATE reports SET status = 'closed', closed_at = now(), closed_by = $2,
                            resolution = $3, updated_at = now()
                     WHERE id = $1",
                )
                .bind(id)
                .bind(me)
                .bind(&what)
                .execute(&st.db)
                .await?;
                let keys: Vec<String> =
                    sqlx::query_scalar("SELECT storage_key FROM report_files WHERE report_id = $1")
                        .bind(id)
                        .fetch_all(&st.db)
                        .await?;
                super::messages::remove_unused_files(&st, keys).await;
            }
            log(
                &st,
                Some(me),
                &req.action,
                Some(id),
                r.reported_user_id,
                what,
            )
            .await;
        }
        "delete_evidence" => {
            if r.status != "closed" {
                return Err(AppError::BadRequest(
                    "Close the report before deleting its evidence.".into(),
                ));
            }
            let n = delete_evidence(&st, id).await?;
            log(&st, Some(me), "delete_evidence", Some(id), r.reported_user_id, format!("Deleted {n} evidence file(s) from the server (names and SHA-256 hashes are kept)")).await;
        }
        _ => {
            let target = r.reported_user_id.ok_or_else(|| {
                AppError::BadRequest(
                    "Nobody to do that to: this report isn't about a person.".into(),
                )
            })?;
            let guild = req.guild_id.or(r.guild_id);
            user_action(&st, me, target, Some(id), guild, &req).await?;
        }
    }
    // Doing something about an open report puts it in review.
    if !matches!(
        req.action.as_str(),
        "dismiss" | "close" | "reopen" | "note" | "approve_file" | "keep_blurred" | "remove_file"
    ) {
        sqlx::query("UPDATE reports SET status = 'review', updated_at = now() WHERE id = $1 AND status = 'open'")
            .bind(id)
            .execute(&st.db)
            .await?;
    }
    notify_owners(&st, None);
    Ok(Json(detail_json(&st, id).await?))
}

/// `POST /api/safety/users/{id}/actions`: the same account actions without a report.
pub async fn user_action_route(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(target): Path<i64>,
    Json(req): Json<ActionRequest>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    user_action(&st, user.id, target, None, req.guild_id, &req).await?;
    Ok(Json(subject_json(&st, target).await?))
}

async fn user_action(
    st: &AppState,
    me: i64,
    target: i64,
    report: Option<i64>,
    guild: Option<i64>,
    req: &ActionRequest,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(target)
        .fetch_one(&st.db)
        .await?;
    if !exists {
        return Err(AppError::BadRequest(
            "That account doesn't exist any more.".into(),
        ));
    }
    let protect = |what: &str| -> AppResult<()> {
        if target == me {
            return Err(AppError::BadRequest(format!("You can't {what} yourself.")));
        }
        Ok(())
    };
    match req.action.as_str() {
        "note" => {
            let text = clean_text(&req.text, MAX_NOTE);
            if text.is_empty() {
                return Err(AppError::BadRequest("Write the note first.".into()));
            }
            log(st, Some(me), "note", report, Some(target), text).await;
        }
        "warn" => {
            protect("warn")?;
            let text = clean_text(&req.text, 1500);
            if text.is_empty() {
                return Err(AppError::BadRequest("Write what the warning says.".into()));
            }
            safety_dm(
                st,
                target,
                format!(
                    "⚠️ **A warning from Outhouse Safety**\n{text}\n\nPlease help keep Outhouse safe and friendly for everyone. Breaking the rules again can get your account suspended."
                ),
            )
            .await?;
            log(st, Some(me), "warn", report, Some(target), text).await;
        }
        "timeout" | "kick" | "ban" => {
            protect(req.action.as_str())?;
            let guild_id = guild.ok_or_else(|| AppError::BadRequest("Pick an outhouse.".into()))?;
            let owner_id: i64 = sqlx::query_scalar(
                "SELECT g.owner_id FROM guild_members m JOIN guilds g ON g.id = m.guild_id
                 WHERE m.guild_id = $1 AND m.user_id = $2",
            )
            .bind(guild_id)
            .bind(target)
            .fetch_optional(&st.db)
            .await?
            .ok_or_else(|| AppError::BadRequest("They aren't in that outhouse.".into()))?;
            if owner_id == target {
                return Err(AppError::BadRequest(
                    "They own that outhouse. Suspend their account instead.".into(),
                ));
            }
            let guild_name: String = sqlx::query_scalar("SELECT name FROM guilds WHERE id = $1")
                .bind(guild_id)
                .fetch_one(&st.db)
                .await?;
            let name = crate::audit::user_label(st, guild_id, target).await;
            let reason = clean_text(&req.text, 500);
            match req.action.as_str() {
                "timeout" => {
                    let minutes = req.minutes.unwrap_or(60).clamp(1, 28 * 24 * 60);
                    sqlx::query(
                        "UPDATE guild_members SET timeout_until = now() + make_interval(mins => $3::INT)
                         WHERE guild_id = $1 AND user_id = $2",
                    )
                    .bind(guild_id)
                    .bind(target)
                    .bind(minutes as i32)
                    .execute(&st.db)
                    .await?;
                    st.dispatch(
                        Audience::Guild(guild_id),
                        ServerEvent::GuildRefresh { guild_id },
                    );
                    crate::audit::Entry::new(guild_id, me, "member_timeout")
                        .target("member", target, name)
                        .details(format!("For {minutes} minutes (Outhouse Safety)"))
                        .save(st)
                        .await;
                    log(
                        st,
                        Some(me),
                        "timeout",
                        report,
                        Some(target),
                        format!("Timed out in {guild_name} for {minutes} minutes"),
                    )
                    .await;
                }
                "kick" => {
                    super::guilds::remove_member(st, guild_id, target).await?;
                    crate::audit::Entry::new(guild_id, me, "member_kick")
                        .target("member", target, name)
                        .details("Outhouse Safety")
                        .save(st)
                        .await;
                    log(
                        st,
                        Some(me),
                        "kick",
                        report,
                        Some(target),
                        format!("Kicked from {guild_name}"),
                    )
                    .await;
                }
                _ => {
                    let stored = if reason.is_empty() {
                        "Outhouse Safety".to_string()
                    } else {
                        format!("Outhouse Safety: {reason}")
                    };
                    sqlx::query(
                        "INSERT INTO bans (guild_id, user_id, reason, banned_by) VALUES ($1, $2, $3, $4)
                         ON CONFLICT (guild_id, user_id) DO UPDATE SET reason = EXCLUDED.reason",
                    )
                    .bind(guild_id)
                    .bind(target)
                    .bind(&stored)
                    .bind(me)
                    .execute(&st.db)
                    .await?;
                    super::guilds::remove_member(st, guild_id, target).await?;
                    crate::audit::Entry::new(guild_id, me, "member_ban")
                        .target("member", target, name)
                        .details(format!("Reason: {stored}"))
                        .save(st)
                        .await;
                    log(
                        st,
                        Some(me),
                        "ban",
                        report,
                        Some(target),
                        format!("Banned from {guild_name}. {reason}"),
                    )
                    .await;
                }
            }
        }
        "suspend" => {
            protect("suspend")?;
            if is_site_owner(&st.db, target).await? {
                return Err(AppError::BadRequest(
                    "You can't suspend a site owner.".into(),
                ));
            }
            let days = req.days.filter(|d| *d > 0).map(|d| d.min(3650) as i32);
            let reason = clean_text(&req.text, 500);
            suspend(st, me, target, days, &reason, req.hide_messages, false).await?;
            let how = match days {
                Some(d) => format!("for {d} day{}", if d == 1 { "" } else { "s" }),
                None => "permanently".into(),
            };
            log(
                st,
                Some(me),
                "suspend",
                report,
                Some(target),
                format!(
                    "Suspended {how}{}{}",
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!(". Reason: {reason}")
                    },
                    if req.hide_messages {
                        ". Their messages are hidden."
                    } else {
                        ""
                    }
                ),
            )
            .await;
        }
        // "Suspend now" (child-safety reports): until the owner has
        // reviewed it, nothing extra hidden, and they're only told it's
        // pending review of a report.
        "suspend_pending" => {
            protect("suspend")?;
            if is_site_owner(&st.db, target).await? {
                return Err(AppError::BadRequest(
                    "You can't suspend a site owner.".into(),
                ));
            }
            let already: bool = sqlx::query_scalar(
                "SELECT COALESCE(suspended_until > now(), false) FROM users WHERE id = $1",
            )
            .bind(target)
            .fetch_one(&st.db)
            .await?;
            if already {
                return Err(AppError::BadRequest("They're already suspended.".into()));
            }
            suspend(st, me, target, None, PENDING_REASON, false, true).await?;
            log(
                st,
                Some(me),
                "suspend",
                report,
                Some(target),
                format!("Suspended now, until reviewed. They only see: \"{PENDING_REASON}\"."),
            )
            .await;
        }
        "make_permanent" => {
            let reason = clean_text(&req.text, 500);
            let changed = sqlx::query(
                "UPDATE users SET suspended_until = $2::TIMESTAMPTZ, suspension_pending = false,
                        suspended_reason = NULLIF($3, '')
                 WHERE id = $1 AND suspended_until > now()",
            )
            .bind(target)
            .bind(FOREVER)
            .bind(&reason)
            .execute(&st.db)
            .await?
            .rows_affected();
            if changed == 0 {
                return Err(AppError::BadRequest("They aren't suspended.".into()));
            }
            log(
                st,
                Some(me),
                "suspend",
                report,
                Some(target),
                if reason.is_empty() {
                    "Suspension made permanent".to_string()
                } else {
                    format!("Suspension made permanent. Reason: {reason}")
                },
            )
            .await;
        }
        "unsuspend" => {
            sqlx::query(
                "UPDATE users SET suspended_until = NULL, suspended_reason = NULL,
                        suspended_at = NULL, suspended_by = NULL, suspension_pending = false
                 WHERE id = $1",
            )
            .bind(target)
            .execute(&st.db)
            .await?;
            log(
                st,
                Some(me),
                "unsuspend",
                report,
                Some(target),
                "Suspension lifted (they need to sign in again; bot tokens need resetting)",
            )
            .await;
        }
        "hide_messages" => {
            let n = hide_all_messages(st, target).await?;
            log(
                st,
                Some(me),
                "hide_messages",
                report,
                Some(target),
                format!("Hid {n} message(s) from everyone"),
            )
            .await;
        }
        "unhide_messages" => {
            let n = unhide_all_messages(st, target).await?;
            log(
                st,
                Some(me),
                "unhide_messages",
                report,
                Some(target),
                format!("Showed {n} hidden message(s) again (child-safety evidence stays hidden)"),
            )
            .await;
        }
        "set_ip_exempt" => {
            let exempt = req
                .exempt
                .ok_or_else(|| AppError::BadRequest("Say whether to allow them.".into()))?;
            sqlx::query("UPDATE users SET ip_block_exempt = $2 WHERE id = $1")
                .bind(target)
                .bind(exempt)
                .execute(&st.db)
                .await?;
            log(
                st,
                Some(me),
                "ip_exempt",
                report,
                Some(target),
                if exempt {
                    "Allowed from blocked networks (they can sign in from a blocked address)"
                } else {
                    "No longer allowed from blocked networks"
                },
            )
            .await;
        }
        "set_age" => {
            let bracket = match req.bracket.as_deref() {
                Some(b @ ("teen" | "adult")) => b,
                _ => return Err(AppError::BadRequest("Pick teen or adult.".into())),
            };
            sqlx::query("UPDATE users SET age_bracket = $2 WHERE id = $1")
                .bind(target)
                .bind(bracket)
                .execute(&st.db)
                .await?;
            if bracket == "teen" {
                // Teens start with DMs from friends only.
                sqlx::query("UPDATE users SET dm_privacy = 'friends' WHERE id = $1")
                    .bind(target)
                    .execute(&st.db)
                    .await?;
            }
            log(
                st,
                Some(me),
                "set_age",
                report,
                Some(target),
                format!("Age group set to {bracket}"),
            )
            .await;
        }
        _ => return Err(AppError::BadRequest("Unknown action.".into())),
    }
    Ok(())
}

/// Suspends an account site-wide: no sign-in, every session and bot token
/// they own stops working, open connections close, they leave voice.
#[allow(clippy::too_many_arguments)]
async fn suspend(
    st: &AppState,
    by: i64,
    target: i64,
    days: Option<i32>,
    reason: &str,
    hide: bool,
    pending: bool,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE users SET suspended_until = CASE WHEN $2::INT IS NULL THEN $4::TIMESTAMPTZ
                                                 ELSE now() + make_interval(days => $2::INT) END,
                suspended_reason = NULLIF($3, ''),
                suspended_at = now(), suspended_by = $5, suspension_pending = $6
         WHERE id = $1",
    )
    .bind(target)
    .bind(days)
    .bind(reason)
    .bind(FOREVER)
    .bind(by)
    .bind(pending)
    .execute(&st.db)
    .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(target)
        .execute(&st.db)
        .await?;
    // Their bots' tokens are replaced (reset them after a suspension ends).
    let bots: Vec<i64> = sqlx::query_scalar(
        "UPDATE bots SET token_hash = encode(sha256(gen_random_uuid()::TEXT::BYTEA), 'hex')
         WHERE owner_id = $1 RETURNING user_id",
    )
    .bind(target)
    .fetch_all(&st.db)
    .await?;
    for id in std::iter::once(target).chain(bots.iter().copied()) {
        crate::api::voice::clear_voice(st, id, None);
        st.dispatch(
            Audience::User(id),
            ServerEvent::InvalidSession {
                reason: "This account has been suspended.".into(),
            },
        );
    }
    if hide {
        hide_all_messages(st, target).await?;
    }
    Ok(())
}

async fn hide_all_messages(st: &AppState, target: i64) -> AppResult<usize> {
    let hidden: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "UPDATE messages m SET safety_hidden = true FROM channels c
         WHERE c.id = m.channel_id AND m.author_id = $1 AND NOT m.safety_hidden
         RETURNING m.id, m.channel_id, c.guild_id",
    )
    .bind(target)
    .fetch_all(&st.db)
    .await?;
    for (id, channel_id, guild_id) in hidden.iter().take(500) {
        st.dispatch(
            Audience::Channel {
                guild_id: guild_id.unwrap_or(DM_GUILD),
                channel_id: *channel_id,
            },
            ServerEvent::MessageDelete {
                id: *id,
                channel_id: *channel_id,
            },
        );
    }
    Ok(hidden.len())
}

/// Shows someone's hidden messages again, except ones held for an open or
/// unreleased child-safety report or with quarantined files.
async fn unhide_all_messages(st: &AppState, target: i64) -> AppResult<u64> {
    Ok(sqlx::query(
        "UPDATE messages m SET safety_hidden = false
         WHERE m.author_id = $1 AND m.safety_hidden
           AND NOT EXISTS (SELECT 1 FROM attachments a JOIN quarantined_files q ON q.storage_key = a.storage_key
                           WHERE a.message_id = m.id)
           AND NOT EXISTS (SELECT 1 FROM reports r WHERE r.message_id = m.id AND r.category = 'child_safety'
                           AND (r.status <> 'closed' OR NOT r.evidence_released))",
    )
    .bind(target)
    .execute(&st.db)
    .await?
    .rows_affected())
}

/// A dismissed child-safety report: its files can be served again and the
/// messages it hid come back (unless another report holds them).
async fn release_quarantine(st: &AppState, r: &ReportRow) -> AppResult<()> {
    if r.category != CHILD_SAFETY {
        return Ok(());
    }
    let keys: Vec<String> = sqlx::query_scalar(
        "DELETE FROM quarantined_files WHERE report_id = $1 RETURNING storage_key",
    )
    .bind(r.id)
    .fetch_all(&st.db)
    .await?;
    sqlx::query(
        "UPDATE messages m SET safety_hidden = false
         WHERE (m.id = $1 OR m.id IN (SELECT message_id FROM attachments WHERE storage_key = ANY($2)))
           AND m.safety_hidden
           AND NOT EXISTS (SELECT 1 FROM attachments a JOIN quarantined_files q ON q.storage_key = a.storage_key
                           WHERE a.message_id = m.id)
           AND NOT EXISTS (SELECT 1 FROM users u WHERE u.id = m.author_id AND u.suspended_until > now())",
    )
    .bind(r.message_id)
    .bind(&keys)
    .execute(&st.db)
    .await?;
    Ok(())
}

/// Deletes a message as the site owner. False if it's already gone.
async fn delete_message(st: &AppState, message_id: i64) -> AppResult<bool> {
    let row: Option<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT m.channel_id, m.thread_id, c.guild_id FROM messages m
         JOIN channels c ON c.id = m.channel_id WHERE m.id = $1",
    )
    .bind(message_id)
    .fetch_optional(&st.db)
    .await?;
    let Some((channel_id, thread_id, guild_id)) = row else {
        return Ok(false);
    };
    let guild_id = guild_id.unwrap_or(DM_GUILD);
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT storage_key FROM attachments WHERE message_id = $1")
            .bind(message_id)
            .fetch_all(&st.db)
            .await?;
    sqlx::query("DELETE FROM messages WHERE id = $1")
        .bind(message_id)
        .execute(&st.db)
        .await?;
    super::messages::remove_unused_files(st, keys).await;
    if let Some(thread_id) = thread_id {
        sqlx::query(
            "UPDATE threads SET message_count = GREATEST(message_count - 1, 0),
                    last_message_id = (SELECT max(id) FROM messages WHERE thread_id = $1)
             WHERE id = $1",
        )
        .bind(thread_id)
        .execute(&st.db)
        .await?;
        super::messages::announce_thread(st, guild_id, channel_id, thread_id).await?;
    }
    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id,
        },
        ServerEvent::MessageDelete {
            id: message_id,
            channel_id,
        },
    );
    Ok(true)
}

/// Removes one (or every) file from a message; the report keeps them.
pub(crate) async fn remove_attachments(
    st: &AppState,
    message_id: i64,
    attachment_id: Option<i64>,
) -> AppResult<usize> {
    let keys: Vec<String> = sqlx::query_scalar(
        "DELETE FROM attachments WHERE message_id = $1 AND ($2::BIGINT IS NULL OR id = $2)
         RETURNING storage_key",
    )
    .bind(message_id)
    .bind(attachment_id)
    .fetch_all(&st.db)
    .await?;
    let n = keys.len();
    super::messages::remove_unused_files(st, keys).await;
    if let Ok(message) = super::messages::fetch_message(&st.db, message_id).await {
        let guild_id: Option<i64> =
            sqlx::query_scalar("SELECT guild_id FROM channels WHERE id = $1")
                .bind(message.channel_id)
                .fetch_one(&st.db)
                .await?;
        st.dispatch(
            Audience::Channel {
                guild_id: guild_id.unwrap_or(DM_GUILD),
                channel_id: message.channel_id,
            },
            ServerEvent::MessageUpdate(message),
        );
    }
    Ok(n)
}

/// Deletes a closed report's evidence files from the server (after police
/// have what they need). Names and hashes stay in the report.
async fn delete_evidence(st: &AppState, report_id: i64) -> AppResult<usize> {
    sqlx::query("UPDATE reports SET evidence_released = true WHERE id = $1")
        .bind(report_id)
        .execute(&st.db)
        .await?;
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT storage_key FROM report_files WHERE report_id = $1")
            .bind(report_id)
            .fetch_all(&st.db)
            .await?;
    let mut deleted = 0;
    for key in keys {
        // Still part of another open report: leave it for that one.
        let other: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM report_files f JOIN reports r ON r.id = f.report_id
                           WHERE f.storage_key = $1 AND r.id <> $2
                             AND (r.status <> 'closed' OR (r.category = 'child_safety' AND NOT r.evidence_released)))",
        )
        .bind(&key)
        .bind(report_id)
        .fetch_one(&st.db)
        .await?;
        if other {
            continue;
        }
        // Any message still using it loses the file; the quarantine entry
        // stays so the key can never be served again.
        sqlx::query("DELETE FROM attachments WHERE storage_key = $1")
            .bind(&key)
            .execute(&st.db)
            .await?;
        if key.bytes().all(|b| b.is_ascii_alphanumeric())
            && tokio::fs::remove_file(st.config.upload_dir.join(&key))
                .await
                .is_ok()
        {
            deleted += 1;
        }
    }
    Ok(deleted)
}

// ---- Evidence package --------------------------------------------------------------------

/// `GET /api/safety/reports/{id}/export`: everything about a report as JSON
/// for police (metadata and SHA-256 hashes; never the files themselves).
pub async fn export(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    require_owner(&st, &user).await?;
    let package = evidence_package(
        &st,
        id,
        json!({ "id": user.id.to_string(), "username": user.username }),
        false,
    )
    .await?;
    log(
        &st,
        Some(user.id),
        "export",
        Some(id),
        None,
        "Exported the evidence package (no files)",
    )
    .await;
    let body = serde_json::to_vec_pretty(&package).map_err(|e| AppError::Other(e.into()))?;
    Response::builder()
        .header(CONTENT_TYPE, "application/json")
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"outhouse-report-{id}.json\""),
        )
        .body(axum::body::Body::from(body))
        .map_err(|e| AppError::Other(e.into()))
}

/// The evidence package (the console's "Export evidence package", and
/// `outhouse-server export-evidence` for deploy/evidence-handover.sh).
///
/// `with_files`: the police hand-over package (handover.rs), which has the
/// files themselves next to this JSON.
pub(crate) async fn evidence_package(
    st: &AppState,
    id: i64,
    generated_by: Value,
    with_files: bool,
) -> AppResult<Value> {
    let detail = detail_json(st, id).await?;
    // The whole log for this report, oldest first (the console shows the latest 100).
    let sql = format!("{LOG_SELECT} WHERE l.report_id = $1 ORDER BY l.id LIMIT 5000");
    let full_log = sqlx::query_as::<_, LogRow>(&sql)
        .bind(id)
        .fetch_all(&st.db)
        .await?;
    let files_note = if with_files {
        "The files are in this hand-over package, in files/, each named by its storage key (\"key\" here), copied byte for byte from the Outhouse server: never opened, viewed, decoded or converted. Check each one against its SHA-256 here, and the whole folder with `sha256sum -c MANIFEST.sha256`. \"detector\" is the result of Outhouse's automatic image check (metadata only)."
    } else {
        "The files themselves are not in this package. They are kept unchanged on the Outhouse server (in its upload folder, named by storage_key) and can be handed over on request; check each copy against its SHA-256 here (e.g. `sha256sum <storage_key>`)."
    };
    // Bring the report's copy of the person's addresses up to date first.
    if let Some(subject) = load_report(st, id).await?.reported_user_id {
        crate::ips::snapshot_for_report(&st.db, id, subject).await?;
    }
    let ips = crate::ips::report_ips_for_export(&st.db, id).await?;
    Ok(json!({
        "format": "outhouse-evidence-package/1",
        "generated_at": Utc::now(),
        "generated_by": generated_by,
        "server": std::env::var("PUBLIC_URL").ok(),
        "software": format!("Outhouse {}", super::audit::version()),
        "time_zone": "All times are UTC (ISO 8601), except ip_addresses, which also gives Sydney time.",
        "files_note": files_note,
        "age_estimates_note": "Where a file's \"detector\" has \"faces\", \"min_age\" and \"ages\" (and a report has \"auto_escalation\"), those are Outhouse's preliminary automated age estimates: a small local model's guess of each face's age group from the face alone (no face pictures are kept). They're often wrong by years, can mistake drawings for faces and are NOT evidence of anyone's age. Outhouse uses them only to escalate explicit pictures to a child-safety review.",
        "report": detail["report"],
        "snapshot": detail["snapshot"],
        "files": detail["files"],
        "reporters": detail["reporters"],
        "reported_account": detail["subject"],
        "ip_addresses": {
            "note": format!(
                "IP addresses this account (account_id) was seen using: when first and last seen, how it was first seen (register = account created, login = signed in, session = signed-in activity, noted at most once an hour) and how many times. Every time is given twice: in UTC, and in Sydney time (Australia/Sydney, AEST UTC+10 or AEDT UTC+11 in daylight saving). Addresses are the visitor's address as Outhouse's web server (Apache, then Caddy) saw it. Outhouse keeps addresses {} days after last use; a report keeps its copy while it's open. Shared addresses are common (households, mobile networks, VPNs).",
                crate::ips::retention_days()
            ),
            "addresses": ips,
        },
        "safety_log": full_log.iter().map(log_json).collect::<Vec<_>>(),
        "blocked_networks": super::ipblocks::evidence(
            &st.db,
            id,
            load_report(st, id).await?.reported_user_id,
        )
        .await?,
    }))
}

// ---- People, settings, log, digest ---------------------------------------------------------

#[derive(Deserialize)]
pub struct UserQuery {
    q: Option<String>,
}

/// `GET /api/safety/users?q=`: find someone by name.
pub async fn find_users(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<UserQuery>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let text = q.q.unwrap_or_default().trim().to_lowercase();
    let pattern = format!(
        "%{}%",
        text.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let rows: Vec<(i64, String, String, String, bool, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, username, display_name, age_bracket, COALESCE(suspended_until > now(), false), created_at
         FROM users
         WHERE NOT is_bot AND (lower(username) LIKE $1 OR lower(display_name) LIKE $1)
         ORDER BY created_at DESC LIMIT 50",
    )
    .bind(&pattern)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!(rows
        .iter()
        .map(|(id, username, name, bracket, suspended, created)| json!({
            "id": id.to_string(), "username": username, "display_name": name,
            "age_bracket": bracket, "suspended": suspended, "created_at": created,
        }))
        .collect::<Vec<_>>())))
}

/// `GET /api/safety/users/{id}`
pub async fn get_user(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    Ok(Json(subject_json(&st, id).await?))
}

#[derive(sqlx::FromRow)]
struct IpRow {
    ip: String,
    kind: String,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    count: i32,
}

#[derive(sqlx::FromRow)]
struct SameIpRow {
    id: i64,
    username: String,
    display_name: String,
    age_bracket: String,
    suspended: bool,
    suspended_until: Option<DateTime<Utc>>,
    banned_in: Vec<String>,
    last_seen: DateTime<Utc>,
    shared_ips: Vec<String>,
}

/// `GET /api/safety/users/{id}/ips`: someone's recent IP addresses and the
/// other accounts seen on them (site owner only; every look is written to
/// the safety log).
pub async fn user_ips(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let (username, is_bot): (String, bool) =
        sqlx::query_as("SELECT username, is_bot FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?
            .ok_or(AppError::NotFound)?;
    let ips = sqlx::query_as::<_, IpRow>(
        "SELECT host(ip) AS ip, kind, first_seen, last_seen, count FROM user_ips
         WHERE user_id = $1 ORDER BY last_seen DESC LIMIT 100",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let same = sqlx::query_as::<_, SameIpRow>(
        "SELECT u.id, u.username, u.display_name, u.age_bracket,
                COALESCE(u.suspended_until > now(), false) AS suspended,
                CASE WHEN u.suspended_until > now() THEN u.suspended_until END AS suspended_until,
                ARRAY(SELECT g.name FROM bans b JOIN guilds g ON g.id = b.guild_id
                      WHERE b.user_id = u.id ORDER BY lower(g.name)) AS banned_in,
                max(o.last_seen) AS last_seen,
                array_agg(DISTINCT host(o.ip)) AS shared_ips
         FROM user_ips mine
         JOIN user_ips o ON o.ip = mine.ip AND o.user_id <> mine.user_id
         JOIN users u ON u.id = o.user_id
         WHERE mine.user_id = $1 AND NOT u.is_bot
         GROUP BY u.id
         ORDER BY (COALESCE(u.suspended_until > now(), false)
                   OR EXISTS(SELECT 1 FROM bans b WHERE b.user_id = u.id)) DESC,
                  max(o.last_seen) DESC
         LIMIT 100",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let trusted = &crate::security::get().trusted_proxies;
    let local = |ip: &str| {
        ip.parse::<std::net::IpAddr>()
            .map(|a| crate::security::is_local_or_proxy(a, trusted))
            .unwrap_or(false)
    };
    log(
        &st,
        Some(user.id),
        "view_ips",
        None,
        Some(id),
        format!("Looked at @{username}'s IP addresses"),
    )
    .await;
    Ok(Json(json!({
        "user_id": id.to_string(),
        "bot": is_bot,
        "retention_days": crate::ips::retention_days(),
        "note": ip_note(),
        "ips": ips.iter().map(|r| json!({
            "ip": r.ip, "kind": r.kind, "first_seen": r.first_seen, "last_seen": r.last_seen,
            "count": r.count, "local": local(&r.ip),
        })).collect::<Vec<_>>(),
        "same_ip_accounts": same.iter().map(|a| json!({
            "id": a.id.to_string(),
            "username": a.username,
            "display_name": a.display_name,
            "age_bracket": a.age_bracket,
            "suspended": a.suspended,
            "suspended_label": a.suspended_until.map(until_label),
            "banned_in": a.banned_in,
            "flagged": a.suspended || !a.banned_in.is_empty(),
            "last_seen": a.last_seen,
            "shared_ips": a.shared_ips,
            // Only LAN / proxy addresses in common: even weaker as a hint.
            "local_only": a.shared_ips.iter().all(|ip| local(ip)),
        })).collect::<Vec<_>>(),
    })))
}

/// `POST /api/safety/ips/purge`: runs the IP retention now. Only exists
/// when the server runs with `OUTHOUSE_TEST_HOOKS=1` (the smoke test);
/// otherwise it's a 404 like any unknown path.
pub async fn purge_ips_now(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<crate::ips::Purged>> {
    if std::env::var("OUTHOUSE_TEST_HOOKS").ok().as_deref() != Some("1") {
        return Err(AppError::NotFound);
    }
    require_owner(&st, &user).await?;
    Ok(Json(
        crate::ips::purge(&st.db, crate::ips::retention_days()).await?,
    ))
}

#[derive(Deserialize)]
pub struct LogQuery {
    #[serde(default, with = "outhouse_common::id::option_as_string")]
    before: Option<i64>,
}

/// `GET /api/safety/log?before=`
pub async fn get_log(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<LogQuery>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let sql = format!(
        "{LOG_SELECT} WHERE ($1::BIGINT IS NULL OR l.id < $1) ORDER BY l.id DESC LIMIT 101"
    );
    let mut rows = sqlx::query_as::<_, LogRow>(&sql)
        .bind(q.before)
        .fetch_all(&st.db)
        .await?;
    let more = rows.len() > 100;
    rows.truncate(100);
    Ok(Json(json!({
        "entries": rows.iter().map(log_json).collect::<Vec<_>>(),
        "has_more": more,
    })))
}

/// `GET /api/safety/settings`: teen protections and which outhouses are family.
pub async fn get_settings(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let outhouses: Vec<(i64, String, bool, i64)> = sqlx::query_as(
        "SELECT g.id, g.name, g.family, (SELECT count(*) FROM guild_members m WHERE m.guild_id = g.id)
         FROM guilds g ORDER BY lower(g.name), g.id",
    )
    .fetch_all(&st.db)
    .await?;
    let teens: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE age_bracket = 'teen' AND NOT is_bot")
            .fetch_one(&st.db)
            .await?;
    Ok(Json(json!({
        "settings": settings(&st.db).await,
        "teens": teens,
        "outhouses": outhouses.iter().map(|(id, name, family, members)| json!({
            "id": id.to_string(), "name": name, "family": family, "members": members,
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct SettingsUpdate {
    teen_contact_limits: Option<bool>,
    teen_block_adult_channels: Option<bool>,
    require_birth_date: Option<bool>,
}

/// `PUT /api/safety/settings`
pub async fn set_settings(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<SettingsUpdate>,
) -> AppResult<Json<SafetySettings>> {
    require_owner(&st, &user).await?;
    let s = sqlx::query_as::<_, SafetySettings>(
        "UPDATE site_settings SET
             teen_contact_limits = COALESCE($1, teen_contact_limits),
             teen_block_adult_channels = COALESCE($2, teen_block_adult_channels),
             require_birth_date = COALESCE($3, require_birth_date)
         WHERE id = 1
         RETURNING teen_contact_limits, teen_block_adult_channels, require_birth_date",
    )
    .bind(req.teen_contact_limits)
    .bind(req.teen_block_adult_channels)
    .bind(req.require_birth_date)
    .fetch_one(&st.db)
    .await?;
    let on = |b: bool| if b { "on" } else { "off" };
    log(
        &st,
        Some(user.id),
        "settings",
        None,
        None,
        format!(
            "Teen DM and friend request limits {}, 18+ channels blocked for teens {}, date of birth at sign-up {}",
            on(s.teen_contact_limits),
            on(s.teen_block_adult_channels),
            on(s.require_birth_date)
        ),
    )
    .await;
    Ok(Json(s))
}

#[derive(Deserialize)]
pub struct FamilyUpdate {
    family: bool,
}

/// `PUT /api/safety/guilds/{id}/family` `{"family": true}`
pub async fn set_family(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<FamilyUpdate>,
) -> AppResult<StatusCode> {
    require_owner(&st, &user).await?;
    let name: String =
        sqlx::query_scalar("UPDATE guilds SET family = $2 WHERE id = $1 RETURNING name")
            .bind(guild_id)
            .bind(req.family)
            .fetch_optional(&st.db)
            .await?
            .ok_or(AppError::NotFound)?;
    log(
        &st,
        Some(user.id),
        "family",
        None,
        None,
        format!(
            "{name} {} a family outhouse",
            if req.family { "is now" } else { "is no longer" }
        ),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn digest_json(db: &PgPool) -> AppResult<Value> {
    let accounts: Vec<(i64, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, username, display_name, age_bracket, created_at FROM users
         WHERE NOT is_bot AND created_at > now() - interval '7 days'
         ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(db)
    .await?;
    let by_category: Vec<(String, i64)> = sqlx::query_as(
        "SELECT category, count(*) FROM reports WHERE created_at > now() - interval '7 days'
           AND source <> 'ip-match'
         GROUP BY category ORDER BY min(priority), category",
    )
    .fetch_all(db)
    .await?;
    let ip_matches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reports WHERE source = 'ip-match' AND created_at > now() - interval '7 days'",
    )
    .fetch_one(db)
    .await?;
    let c = counts(db).await?;
    let shut: Vec<String> = sqlx::query_scalar(
        "SELECT details FROM safety_log
         WHERE action = 'outhouse_shut_down' AND created_at > now() - interval '7 days'
         ORDER BY created_at DESC LIMIT 20",
    )
    .fetch_all(db)
    .await?;
    Ok(json!({
        "days": 7,
        "outhouses_shut_down": shut,
        "new_accounts": accounts.iter().map(|(id, username, name, bracket, at)| json!({
            "id": id.to_string(), "username": username, "display_name": name,
            "age_bracket": bracket, "created_at": at,
        })).collect::<Vec<_>>(),
        "reports": by_category.iter().map(|(cat, n)| json!({
            "category": cat, "category_label": category_label(cat), "count": n,
        })).collect::<Vec<_>>(),
        "reports_total": by_category.iter().map(|(_, n)| n).sum::<i64>(),
        "waiting": c.open + c.in_review,
        "urgent": c.urgent,
        "ip_matches": ip_matches,
        "ip_matches_open": c.ip_matches,
    }))
}

/// `GET /api/safety/digest`: the last seven days.
pub async fn digest(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    Ok(Json(digest_json(&st.db).await?))
}

/// Once a week the owner gets the digest as a DM from Outhouse Safety.
pub fn spawn_digest(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Err(e) = send_digest_if_due(&st).await {
                tracing::warn!(error = %e, "weekly safety digest failed");
            }
        }
    });
}

async fn send_digest_if_due(st: &AppState) -> AppResult<()> {
    let last: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT last_digest_at FROM site_settings WHERE id = 1")
            .fetch_optional(&st.db)
            .await?
            .flatten();
    let due = match last {
        // A new server: start counting from now.
        None => false,
        Some(at) => Utc::now() - at >= chrono::Duration::days(7),
    };
    if last.is_some() && !due {
        return Ok(());
    }
    sqlx::query("UPDATE site_settings SET last_digest_at = now() WHERE id = 1")
        .execute(&st.db)
        .await?;
    if !due {
        return Ok(());
    }
    let d = digest_json(&st.db).await?;
    let accounts = d["new_accounts"].as_array().cloned().unwrap_or_default();
    let mut names: Vec<String> = accounts
        .iter()
        .take(15)
        .map(|a| {
            format!(
                "{} (@{}{})",
                a["display_name"].as_str().unwrap_or(""),
                a["username"].as_str().unwrap_or(""),
                if a["age_bracket"] == "teen" {
                    ", teen"
                } else {
                    ""
                }
            )
        })
        .collect();
    if accounts.len() > 15 {
        names.push(format!("and {} more", accounts.len() - 15));
    }
    let reports: Vec<String> = d["reports"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r| {
            format!(
                "{} {}",
                r["count"],
                r["category_label"].as_str().unwrap_or("")
            )
        })
        .collect();
    let content = format!(
        "📋 **Weekly safety digest** (last 7 days)\n• New accounts: {}{}\n• Reports: {}{}\n• Waiting for review: {}{}\n• Same-IP hints (suspended or banned accounts): {}{}\nOpen User settings → Reports to review them.",
        accounts.len(),
        if names.is_empty() { String::new() } else { format!(" — {}", names.join(", ")) },
        d["reports_total"],
        if reports.is_empty() { String::new() } else { format!(" ({})", reports.join(", ")) },
        d["waiting"],
        if d["urgent"].as_i64().unwrap_or(0) > 0 { format!(" (⚠️ {} child safety)", d["urgent"]) } else { String::new() },
        d["ip_matches"],
        match d["outhouses_shut_down"].as_array().map(|a| a.len()).unwrap_or(0) {
            0 => String::new(),
            n => format!("\n• Outhouses shut down: {n}"),
        },
    );
    for owner in site_owner_ids(&st.db).await? {
        if let Err(e) = safety_dm(st, owner, content.clone()).await {
            tracing::warn!(error = %e, "couldn't send the safety digest");
        }
    }
    Ok(())
}

/// Which of these users can't be shown to `viewer` (suspended accounts are
/// hidden from everyone but the site owner).
pub async fn hidden_profiles(db: &PgPool, viewer: i64, ids: &[i64]) -> AppResult<HashSet<i64>> {
    if is_site_owner(db, viewer).await? {
        return Ok(HashSet::new());
    }
    let rows: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE id = ANY($1) AND suspended_until > now()")
            .bind(ids)
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn age_groups_from_birth_dates() {
        let today = day("2026-11-10");
        assert_eq!(
            bracket_for_birth_date("1990-01-01", today).unwrap(),
            "adult"
        );
        assert_eq!(
            bracket_for_birth_date("2008-11-10", today).unwrap(),
            "adult"
        ); // 18 today
        assert_eq!(bracket_for_birth_date("2008-11-11", today).unwrap(), "teen"); // 18 tomorrow
        assert_eq!(bracket_for_birth_date("2013-11-10", today).unwrap(), "teen"); // 13 today
        assert!(matches!(
            bracket_for_birth_date("2013-11-11", today),
            Err(AppError::Refused(_))
        ));
        assert!(matches!(
            bracket_for_birth_date("2027-01-01", today),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            bracket_for_birth_date("not a date", today),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            bracket_for_birth_date("1800-01-01", today),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn child_safety_comes_first() {
        assert_eq!(priority(CHILD_SAFETY), 0);
        assert!(priority("hate") < priority("spam"));
        assert_eq!(CATEGORIES[0].0, CHILD_SAFETY);
        assert_eq!(category_label("spam"), "Spam or scams");
        assert_eq!(category_label("nope"), "Other");
    }

    #[test]
    fn report_previews_and_places() {
        let snap = json!({
            "kind": "message",
            "message": { "content": "hello there", "attachments": [] },
            "place": { "guild_name": "Smoke", "channel_name": "general", "dm": false },
        });
        assert_eq!(preview(&snap), "hello there");
        assert_eq!(place_label(&snap), "Smoke › #general");
        let dm = json!({ "kind": "message", "message": { "content": "", "attachments": [{}, {}] },
                         "place": { "dm": true, "participants": ["a", "b"] } });
        assert_eq!(preview(&dm), "(2 files)");
        assert_eq!(place_label(&dm), "Direct message");
        assert_eq!(
            until_label("9999-12-31T00:00:00Z".parse().unwrap()),
            "permanently"
        );
    }
}
