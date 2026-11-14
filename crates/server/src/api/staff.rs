//! Site staff: people the site owner trusts to help with safety work,
//! separate from outhouse roles.
//!
//! Each staff member has a set of permissions (`PERMS`); a tier (Support,
//! Moderator, Senior moderator) is just a preset, and the owner can tick
//! and untick single permissions ("Custom"). Every check is made here on
//! the server; the web client only hides what isn't allowed.
//!
//! - Being staff needs a confirmed email and two-factor on. If either goes
//!   away their powers pause automatically (the owner is told) until fixed.
//! - Staff powers only work in a session that passed two-factor
//!   (`sessions.mfa`), never in one from before two-factor was on.
//! - Every staff action goes in the safety log with the tier and the
//!   permission used (`STAFF_CTX`, read by `safety::log`); those entries
//!   can't be changed or deleted (a database trigger).
//! - Conflicts of interest: staff can't act on reports about themselves,
//!   reports they made, or outhouses they own or moderate.
//! - Hourly caps (`STAFF_ACTIONS_PER_HOUR`, default 30;
//!   `STAFF_HEAVY_ACTIONS_PER_HOUR`, default 5 for permanent suspensions,
//!   IP blocks, shut-downs and transfers); hitting one alerts the owner.
//! - Two-person rule (owner settings): a permanent suspension with an IP
//!   block, or shutting down an outhouse with 25+ members, waits for
//!   another senior moderator or the owner to approve it (72 hours).
//! - The site owner is never limited by any of this.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::User;
use outhouse_common::permissions as p;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::audit::is_site_owner;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

// ---- Permissions and tiers ------------------------------------------------------

/// (key, label, tier it belongs to). Everything not here is the owner's alone:
/// police hand-over, deleting evidence, permanently deleting outhouses,
/// managing staff, server settings, the image detector's settings, email
/// tests, backups, teen protections and the safety log as a whole.
pub const PERMS: &[(&str, &str, &str)] = &[
    (
        "people",
        "People: account basics (username, age group, outhouses, masked email, two-factor)",
        "support",
    ),
    (
        "account_help",
        "Password-reset links and removing two-factor for someone locked out",
        "support",
    ),
    (
        "report_status",
        "Report status (open or closed, category; no evidence)",
        "support",
    ),
    (
        "reports",
        "Reports inbox with the reported snapshot (±10 messages)",
        "moderator",
    ),
    (
        "hide_messages",
        "Hide or remove reported messages",
        "moderator",
    ),
    (
        "warn",
        "Warn people (a DM from Outhouse Safety)",
        "moderator",
    ),
    (
        "timeout",
        "Time out or kick someone in an outhouse",
        "moderator",
    ),
    (
        "suspend_7d",
        "Suspend for up to 7 days (or ban from an outhouse)",
        "moderator",
    ),
    (
        "outhouse_look",
        "Read-only look inside an outhouse that has an open report",
        "moderator",
    ),
    (
        "detector",
        "Approve or remove image-detector holds (never child safety)",
        "moderator",
    ),
    (
        "same_ip",
        "Same-IP hints: which accounts, never the addresses",
        "moderator",
    ),
    ("suspend_permanent", "Permanent suspensions", "senior"),
    ("ips", "IP addresses and Blocked networks", "senior"),
    ("shut_down", "Shut down and restore outhouses", "senior"),
    (
        "child_safety",
        "Child-safety reports: Suspend now, details and hashes (never files)",
        "senior",
    ),
    ("family", "Mark family outhouses", "senior"),
    ("transfers", "Staff-assisted ownership transfers", "senior"),
];

pub const TIERS: &[(&str, &str)] = &[
    ("support", "Support"),
    ("moderator", "Moderator"),
    ("senior", "Senior moderator"),
];

pub fn tier_rank(tier: &str) -> u8 {
    match tier {
        "support" => 1,
        "moderator" => 2,
        "senior" => 3,
        _ => 0,
    }
}

pub fn tier_label(tier: &str) -> &'static str {
    match tier {
        "support" => "Support",
        "moderator" => "Moderator",
        "senior" => "Senior moderator",
        "owner" => "Site owner",
        _ => "Custom",
    }
}

/// The permissions a tier comes with.
pub fn preset(tier: &str) -> Vec<String> {
    let rank = tier_rank(tier);
    PERMS
        .iter()
        .filter(|(_, _, t)| tier_rank(t) <= rank)
        .map(|(k, _, _)| k.to_string())
        .collect()
}

/// How senior a set of permissions is (for "staff can't act on staff of an
/// equal or higher tier"): the highest tier any of them belongs to.
pub fn rank_of(perms: &[String]) -> u8 {
    perms
        .iter()
        .filter_map(|k| PERMS.iter().find(|(key, _, _)| key == k))
        .map(|(_, _, t)| tier_rank(t))
        .max()
        .unwrap_or(1)
}

fn clean_perms(list: &[String]) -> Vec<String> {
    PERMS
        .iter()
        .filter(|(k, _, _)| list.iter().any(|x| x == k))
        .map(|(k, _, _)| k.to_string())
        .collect()
}

// ---- The log context ----------------------------------------------------------------

tokio::task_local! {
    /// (staff user id, tier, permission used) for this request, so the
    /// safety log records them (see `safety::log`).
    static STAFF_CTX: RefCell<Option<(i64, String, String)>>;
}

/// Middleware: gives every request its own (empty) staff context.
pub async fn scope_ctx(req: Request, next: Next) -> Response {
    STAFF_CTX.scope(RefCell::new(None), next.run(req)).await
}

/// Notes which permission this staff member is using now.
pub fn set_ctx(user_id: i64, tier: &str, perm: &str) {
    let _ = STAFF_CTX.try_with(|c| {
        *c.borrow_mut() = Some((user_id, tier.to_string(), perm.to_string()));
    });
}

/// The (tier, permission) to write in the safety log for `actor`, if the
/// current request is that staff member's.
pub fn log_ctx(actor: Option<i64>) -> Option<(String, String)> {
    let actor = actor?;
    STAFF_CTX
        .try_with(|c| {
            c.borrow()
                .as_ref()
                .filter(|(id, _, _)| *id == actor)
                .map(|(_, t, p)| (t.clone(), p.clone()))
        })
        .ok()
        .flatten()
}

// ---- Who is acting -------------------------------------------------------------------

/// The site owner or a staff member, with what they may do.
#[derive(Debug, Clone)]
pub struct Actor {
    pub user: User,
    pub owner: bool,
    pub tier: String,
    pub perms: Vec<String>,
    pub rank: u8,
}

impl Actor {
    pub fn id(&self) -> i64 {
        self.user.id
    }

    pub fn can(&self, perm: &str) -> bool {
        self.owner || self.perms.iter().any(|p| p == perm)
    }

    /// Refuses (403) unless they have `perm`; notes it for the log.
    pub fn require(&self, perm: &str) -> AppResult<()> {
        if !self.can(perm) {
            return Err(AppError::Forbidden);
        }
        if !self.owner {
            set_ctx(self.user.id, &self.tier, perm);
        }
        Ok(())
    }

    /// The first of these they have (403 if none).
    pub fn require_any(&self, perms: &[&str]) -> AppResult<()> {
        match perms.iter().find(|p| self.can(p)) {
            Some(p) => self.require(p),
            None => Err(AppError::Forbidden),
        }
    }

    pub fn require_owner(&self) -> AppResult<()> {
        if self.owner {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }

    /// The site owner as an actor (background jobs, owner-only paths).
    pub fn owner_of(user: User) -> Self {
        Self {
            user,
            owner: true,
            tier: "owner".into(),
            perms: PERMS.iter().map(|(k, _, _)| k.to_string()).collect(),
            rank: u8::MAX,
        }
    }
}

#[derive(sqlx::FromRow)]
struct StaffRow {
    tier: String,
    perms: Vec<String>,
    lapsed_at: Option<DateTime<Utc>>,
}

async fn staff_row(db: &PgPool, user_id: i64) -> AppResult<Option<StaffRow>> {
    Ok(
        sqlx::query_as::<_, StaffRow>(
            "SELECT tier, perms, lapsed_at FROM staff WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(db)
        .await?,
    )
}

/// Does this session count as signed in with two-factor?
pub async fn session_mfa(db: &PgPool, token: &str) -> AppResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT mfa FROM sessions WHERE token_hash = $1")
            .bind(auth::token_hash(token))
            .fetch_optional(db)
            .await?
            .unwrap_or(false),
    )
}

/// Marks a session as having passed two-factor.
pub async fn mark_session_mfa(db: &PgPool, token: &str) -> AppResult<()> {
    sqlx::query("UPDATE sessions SET mfa = true WHERE token_hash = $1")
        .bind(auth::token_hash(token))
        .execute(db)
        .await?;
    Ok(())
}

pub const NEEDS_MFA_SESSION: &str =
    "Staff powers need a sign-in that used two-factor. Sign out and sign in again with your code.";

/// The owner or an active staff member in a two-factor session; 403 otherwise.
pub async fn actor_for(st: &AppState, user: User, token: &str) -> AppResult<Actor> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    if is_site_owner(&st.db, user.id).await? {
        return Ok(Actor::owner_of(user));
    }
    let Some(row) = staff_row(&st.db, user.id).await? else {
        return Err(AppError::Forbidden);
    };
    if let Some(why) = check_lapse(st, user.id).await? {
        return Err(AppError::Refused(format!(
            "Your staff powers are paused: {why}"
        )));
    }
    if !session_mfa(&st.db, token).await? {
        return Err(AppError::Refused(NEEDS_MFA_SESSION.into()));
    }
    sqlx::query(
        "UPDATE staff SET last_active_at = now() WHERE user_id = $1
           AND (last_active_at IS NULL OR last_active_at < now() - interval '1 minute')",
    )
    .bind(user.id)
    .execute(&st.db)
    .await?;
    let perms = clean_perms(&row.perms);
    let rank = if row.tier == "custom" {
        rank_of(&perms)
    } else {
        tier_rank(&row.tier)
    };
    Ok(Actor {
        user,
        owner: false,
        tier: row.tier,
        perms,
        rank,
    })
}

/// Extracts the site owner or an active staff member (403 otherwise).
pub struct StaffActor(pub Actor);

impl FromRequestParts<AppState> for StaffActor {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        let token = auth::bearer_token(&parts.headers).unwrap_or("").to_string();
        Ok(StaffActor(actor_for(state, user, &token).await?))
    }
}

/// Is this account staff right now (not paused)? Their rank if so.
pub async fn active_rank(db: &PgPool, user_id: i64) -> AppResult<Option<u8>> {
    let row: Option<(String, Vec<String>, bool)> = sqlx::query_as(
        "SELECT s.tier, s.perms, (u.email_verified_at IS NOT NULL AND u.totp_secret IS NOT NULL)
         FROM staff s JOIN users u ON u.id = s.user_id WHERE s.user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.filter(|r| r.2).map(|(tier, perms, _)| {
        if tier == "custom" {
            rank_of(&clean_perms(&perms))
        } else {
            tier_rank(&tier)
        }
    }))
}

/// Staff can't act on the site owner's account, nor on staff of their own
/// tier or higher (the owner can).
pub async fn guard_target(st: &AppState, a: &Actor, target: i64) -> AppResult<()> {
    if a.owner {
        return Ok(());
    }
    if is_site_owner(&st.db, target).await? {
        return Err(AppError::Refused(
            "That's the site owner's account. Staff can't act on it.".into(),
        ));
    }
    if let Some(rank) = active_rank(&st.db, target).await? {
        if rank >= a.rank {
            return Err(AppError::Refused(
                "They're staff at your level or above. The site owner needs to handle this.".into(),
            ));
        }
    }
    Ok(())
}

// ---- Conflicts of interest -------------------------------------------------------------

pub const SOMEONE_ELSE: &str =
    "Someone else needs to handle this (it involves you or an outhouse you own or moderate).";

/// The outhouses this person owns or moderates (Administrator, Manage
/// outhouse, Ban or Kick members).
pub async fn moderated_guilds(db: &PgPool, user_id: i64) -> AppResult<HashSet<i64>> {
    let mut out: HashSet<i64> =
        sqlx::query_scalar::<_, i64>("SELECT id FROM guilds WHERE owner_id = $1")
            .bind(user_id)
            .fetch_all(db)
            .await?
            .into_iter()
            .collect();
    let member_of: Vec<i64> =
        sqlx::query_scalar("SELECT guild_id FROM guild_members WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(db)
            .await?;
    let strong = p::ADMINISTRATOR | p::MANAGE_GUILD | p::BAN_MEMBERS | p::KICK_MEMBERS;
    for g in member_of {
        if out.contains(&g) {
            continue;
        }
        // Shut-down outhouses refuse member_ctx; they were moderating it if
        // their roles say so.
        let base: Option<i64> = sqlx::query_scalar(
            "SELECT COALESCE(bit_or(r.permissions), 0)::BIGINT FROM roles r
             WHERE r.guild_id = $1 AND (r.id = $1 OR r.id IN
                   (SELECT role_id FROM member_roles WHERE guild_id = $1 AND user_id = $2))",
        )
        .bind(g)
        .bind(user_id)
        .fetch_optional(db)
        .await?;
        if (base.unwrap_or(0) as u64) & strong != 0 {
            out.insert(g);
        }
    }
    Ok(out)
}

/// What a staff member needs to know about themselves and a report to see
/// whether it's a conflict.
pub struct ConflictCtx {
    pub owner: bool,
    pub me: i64,
    pub guilds: HashSet<i64>,
    pub reported: HashSet<i64>,
}

impl ConflictCtx {
    pub async fn load(db: &PgPool, a: &Actor) -> AppResult<Self> {
        if a.owner {
            return Ok(Self {
                owner: true,
                me: a.id(),
                guilds: HashSet::new(),
                reported: HashSet::new(),
            });
        }
        let reported: HashSet<i64> = sqlx::query_scalar::<_, i64>(
            "SELECT DISTINCT report_id FROM report_reporters WHERE reporter_id = $1",
        )
        .bind(a.id())
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();
        Ok(Self {
            owner: false,
            me: a.id(),
            guilds: moderated_guilds(db, a.id()).await?,
            reported,
        })
    }

    pub fn report(&self, report_id: i64, reported_user: Option<i64>, guild: Option<i64>) -> bool {
        !self.owner
            && (reported_user == Some(self.me)
                || self.reported.contains(&report_id)
                || guild.is_some_and(|g| self.guilds.contains(&g)))
    }
}

/// Refuses an outhouse the staff member owns or moderates.
pub async fn guard_guild(st: &AppState, a: &Actor, guild_id: i64) -> AppResult<()> {
    if a.owner {
        return Ok(());
    }
    if moderated_guilds(&st.db, a.id()).await?.contains(&guild_id) {
        return Err(AppError::Refused(SOMEONE_ELSE.into()));
    }
    Ok(())
}

// ---- Hourly caps ---------------------------------------------------------------------------

fn env_cap(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

pub fn cap(heavy: bool) -> i64 {
    if heavy {
        env_cap("STAFF_HEAVY_ACTIONS_PER_HOUR", 5)
    } else {
        env_cap("STAFF_ACTIONS_PER_HOUR", 30)
    }
}

static CAP_ALERTS: Mutex<Option<HashMap<(i64, bool), Instant>>> = Mutex::new(None);

/// Counts one capped action (warn, time out, suspend, hide; or a "heavy"
/// one: permanent suspension, IP block, shut-down, transfer). At the cap
/// it's refused and the owner is alerted (once an hour).
pub async fn charge(st: &AppState, a: &Actor, heavy: bool) -> AppResult<()> {
    if a.owner {
        return Ok(());
    }
    let limit = cap(heavy);
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM staff_actions
         WHERE user_id = $1 AND heavy = $2 AND at > now() - interval '1 hour'",
    )
    .bind(a.id())
    .bind(heavy)
    .fetch_one(&st.db)
    .await?;
    if used >= limit {
        let first = {
            let mut guard = CAP_ALERTS.lock().expect("cap alerts lock");
            let map = guard.get_or_insert_with(HashMap::new);
            let fresh = map
                .get(&(a.id(), heavy))
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3600));
            if fresh {
                map.insert((a.id(), heavy), Instant::now());
            }
            fresh
        };
        if first {
            super::safety::log_action(
                st,
                Some(a.id()),
                "staff_cap",
                None,
                None,
                format!(
                    "Reached the hourly limit of {limit} {} actions; further ones are refused for now",
                    if heavy { "big" } else { "moderation" }
                ),
            )
            .await;
            alert_owner(
                st,
                "Staff action limit reached",
                "A staff member reached their hourly action limit, so further actions are blocked for now. Open User settings → Staff activity to see what happened.",
            )
            .await;
        }
        return Err(AppError::TooManyRequests(format!(
            "You've reached the hourly limit ({limit}) for {}. The site owner has been told. Try again later.",
            if heavy {
                "permanent suspensions, IP blocks, shut-downs and transfers"
            } else {
                "warnings, timeouts, suspensions and hiding messages"
            }
        )));
    }
    sqlx::query("INSERT INTO staff_actions (user_id, heavy) VALUES ($1, $2)")
        .bind(a.id())
        .bind(heavy)
        .execute(&st.db)
        .await?;
    Ok(())
}

/// A push notification and the deploy kit's ntfy / webhook alert to the
/// site owner (no content: just that something needs a look).
pub async fn alert_owner(st: &AppState, title: &str, body: &str) {
    let payload = json!({
        "title": title,
        "body": body,
        "kind": "safety",
        "guild_id": "0",
        "channel_id": "staff",
        "tag": "staff-alert",
        "url": "/app?reports=1",
    });
    if let Ok(owners) = super::safety::site_owner_ids(&st.db).await {
        for owner in owners {
            if let Err(e) = crate::push::send_to_user(st, owner, &payload).await {
                tracing::warn!(error = %e, "staff alert push failed");
            }
        }
    }
    super::safety::external_alert(st, &format!("Outhouse: {title}"), body).await;
}

/// A DM from Outhouse Safety to every site owner.
async fn tell_owner(st: &AppState, text: String) {
    if let Ok(owners) = super::safety::site_owner_ids(&st.db).await {
        for owner in owners {
            if let Err(e) = super::safety::safety_note(st, owner, text.clone()).await {
                tracing::warn!(error = %e, "couldn't tell the owner about staff");
            }
        }
    }
}

// ---- Paused powers (two-factor off, email not confirmed) -----------------------------------

/// Why a staff member can't use their powers right now (None = fine). The
/// first time it's noticed their powers pause and the owner is told; when
/// it's fixed they're back on (and the owner is told).
pub async fn check_lapse(st: &AppState, user_id: i64) -> AppResult<Option<String>> {
    let row: Option<(bool, bool, Option<DateTime<Utc>>, String)> = sqlx::query_as(
        "SELECT u.email_verified_at IS NOT NULL, u.totp_secret IS NOT NULL, s.lapsed_at, u.username
         FROM staff s JOIN users u ON u.id = s.user_id WHERE s.user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?;
    let Some((email, twofa, lapsed, username)) = row else {
        return Ok(None);
    };
    let why = match (email, twofa) {
        (true, true) => None,
        (false, true) => Some("their email address isn't confirmed".to_string()),
        (true, false) => Some("two-factor sign-in is off".to_string()),
        (false, false) => {
            Some("two-factor sign-in is off and their email isn't confirmed".to_string())
        }
    };
    match (&why, lapsed) {
        (Some(reason), None) => {
            let n = sqlx::query(
                "UPDATE staff SET lapsed_at = now(), lapsed_reason = $2 WHERE user_id = $1 AND lapsed_at IS NULL",
            )
            .bind(user_id)
            .bind(reason)
            .execute(&st.db)
            .await?
            .rows_affected();
            if n == 1 {
                super::safety::log_action(
                    st,
                    None,
                    "staff_paused",
                    None,
                    Some(user_id),
                    format!("Staff powers paused automatically: {reason}"),
                )
                .await;
                tell_owner(
                    st,
                    format!("🛡 **Staff powers paused**\n@{username}'s staff powers are paused because {reason}. They come back on their own once it's fixed. (User settings → Staff)"),
                )
                .await;
                alert_owner(st, "Staff powers paused", "A staff member's powers were paused automatically (two-factor or email). Open User settings → Staff.").await;
            }
        }
        (None, Some(_)) => {
            let n = sqlx::query(
                "UPDATE staff SET lapsed_at = NULL, lapsed_reason = NULL WHERE user_id = $1 AND lapsed_at IS NOT NULL",
            )
            .bind(user_id)
            .execute(&st.db)
            .await?
            .rows_affected();
            if n == 1 {
                super::safety::log_action(
                    st,
                    None,
                    "staff_resumed",
                    None,
                    Some(user_id),
                    "Staff powers back on (confirmed email and two-factor)",
                )
                .await;
                tell_owner(
                    st,
                    format!("🛡 @{username}'s staff powers are back on (confirmed email and two-factor)."),
                )
                .await;
            }
        }
        _ => {}
    }
    Ok(why.map(|r| r.replace("their ", "your ")))
}

/// After two-factor or an email changed: pause (or restore) their powers now.
pub async fn recheck(st: &AppState, user_id: i64) {
    if let Err(e) = check_lapse(st, user_id).await {
        tracing::warn!(error = %e, "couldn't recheck a staff member");
    }
    st.dispatch(Audience::User(user_id), ServerEvent::StaffChanged);
}

// ---- Site settings --------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StaffSettings {
    pub two_person_permanent_ip: bool,
    pub two_person_shut_down: bool,
    pub two_person_members: i32,
    pub staff_badge: bool,
}

pub async fn settings(db: &PgPool) -> StaffSettings {
    sqlx::query_as::<_, StaffSettings>(
        "SELECT two_person_permanent_ip, two_person_shut_down, two_person_members, staff_badge
         FROM site_settings WHERE id = 1",
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .unwrap_or(StaffSettings {
        two_person_permanent_ip: true,
        two_person_shut_down: true,
        two_person_members: 25,
        staff_badge: false,
    })
}

fn settings_json(s: &StaffSettings) -> Value {
    json!({
        "two_person_permanent_ip": s.two_person_permanent_ip,
        "two_person_shut_down": s.two_person_shut_down,
        "two_person_members": s.two_person_members,
        "staff_badge": s.staff_badge,
    })
}

/// Show a small "Staff" badge on this person's profile? (Owner setting, off by default.)
pub async fn public_badge(db: &PgPool, user_id: i64) -> bool {
    if !settings(db).await.staff_badge {
        return false;
    }
    matches!(active_rank(db, user_id).await, Ok(Some(_)))
}

// ---- What the app needs to know about me ------------------------------------------------------

/// For `GET /api/users/@me/site-owner`: my staff powers (None if not staff).
pub async fn my_staff_json(st: &AppState, user_id: i64, token: &str) -> AppResult<Option<Value>> {
    let Some(row) = staff_row(&st.db, user_id).await? else {
        return Ok(None);
    };
    let paused = check_lapse(st, user_id).await?;
    let mfa = session_mfa(&st.db, token).await?;
    let perms = clean_perms(&row.perms);
    let s = settings(&st.db).await;
    Ok(Some(json!({
        "tier": row.tier,
        "tier_label": tier_label(&row.tier),
        "perms": perms,
        "active": paused.is_none() && mfa,
        "paused_reason": paused,
        "mfa_session": mfa,
        "mfa_note": if mfa { Value::Null } else { json!(NEEDS_MFA_SESSION) },
        "two_person": settings_json(&s),
        "lapsed_at": row.lapsed_at,
    })))
}

// ---- The owner's Staff page -----------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct ListRow {
    user_id: i64,
    username: String,
    display_name: String,
    tier: String,
    perms: Vec<String>,
    added_at: DateTime<Utc>,
    last_active_at: Option<DateTime<Utc>>,
    lapsed_at: Option<DateTime<Utc>>,
    lapsed_reason: Option<String>,
    email_verified: bool,
    two_factor: bool,
    actions_week: i64,
}

fn catalogue() -> Value {
    json!({
        "perms": PERMS.iter().map(|(k, l, t)| json!({ "key": k, "label": l, "tier": t, "tier_label": tier_label(t) })).collect::<Vec<_>>(),
        "tiers": TIERS.iter().map(|(k, l)| json!({ "key": k, "label": l, "perms": preset(k) })).collect::<Vec<_>>(),
        "caps": { "normal": cap(false), "heavy": cap(true) },
    })
}

/// `GET /api/safety/staff`: everyone on staff, the settings and the catalogue.
pub async fn list(StaffActor(a): StaffActor, State(st): State<AppState>) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM staff")
        .fetch_all(&st.db)
        .await?;
    for id in ids {
        check_lapse(&st, id).await?;
    }
    let rows = sqlx::query_as::<_, ListRow>(
        "SELECT s.user_id, u.username, u.display_name, s.tier, s.perms, s.added_at,
                s.last_active_at, s.lapsed_at, s.lapsed_reason,
                u.email_verified_at IS NOT NULL AS email_verified, u.totp_secret IS NOT NULL AS two_factor,
                (SELECT count(*) FROM safety_log l WHERE l.actor_id = s.user_id AND l.staff_tier IS NOT NULL
                   AND l.created_at > now() - interval '7 days') AS actions_week
         FROM staff s JOIN users u ON u.id = s.user_id
         ORDER BY lower(u.username)",
    )
    .fetch_all(&st.db)
    .await?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM staff_requests WHERE status = 'pending' AND expires_at > now()",
    )
    .fetch_one(&st.db)
    .await?;
    Ok(Json(json!({
        "staff": rows.iter().map(|r| json!({
            "id": r.user_id.to_string(),
            "username": r.username,
            "display_name": r.display_name,
            "tier": r.tier,
            "tier_label": tier_label(&r.tier),
            "perms": clean_perms(&r.perms),
            "added_at": r.added_at,
            "last_active_at": r.last_active_at,
            "paused": r.lapsed_at.is_some(),
            "paused_reason": r.lapsed_reason,
            "email_verified": r.email_verified,
            "two_factor": r.two_factor,
            "actions_week": r.actions_week,
        })).collect::<Vec<_>>(),
        "settings": settings_json(&settings(&st.db).await),
        "catalogue": catalogue(),
        "pending_requests": pending,
    })))
}

#[derive(Deserialize)]
pub struct CandidateQuery {
    #[serde(default)]
    username: String,
}

async fn find_candidate(
    db: &PgPool,
    username: &str,
) -> AppResult<(i64, String, String, bool, bool, bool, bool)> {
    let name = username.trim().trim_start_matches('@').to_lowercase();
    sqlx::query_as::<_, (i64, String, String, bool, bool, bool, bool)>(
        "SELECT id, username, display_name, email_verified_at IS NOT NULL, totp_secret IS NOT NULL,
                is_bot, COALESCE(suspended_until > now(), false)
         FROM users WHERE lower(username) = $1",
    )
    .bind(&name)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| AppError::BadRequest("Nobody has that username.".into()))
}

/// `GET /api/safety/staff/candidate?username=`: can they be staff?
pub async fn candidate(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Query(q): Query<CandidateQuery>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let (id, username, name, email, twofa, bot, suspended) =
        find_candidate(&st.db, &q.username).await?;
    let already: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM staff WHERE user_id = $1)")
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    let owner = is_site_owner(&st.db, id).await?;
    let problem = refusal(email, twofa, bot, suspended, owner, already);
    Ok(Json(json!({
        "id": id.to_string(),
        "username": username,
        "display_name": name,
        "email_verified": email,
        "two_factor": twofa,
        "bot": bot,
        "suspended": suspended,
        "site_owner": owner,
        "already_staff": already,
        "ok": problem.is_none(),
        "problem": problem,
    })))
}

fn refusal(
    email: bool,
    twofa: bool,
    bot: bool,
    suspended: bool,
    owner: bool,
    already: bool,
) -> Option<String> {
    if bot {
        return Some("Bots can't be staff.".into());
    }
    if owner {
        return Some("That's the site owner: they can already do everything.".into());
    }
    if already {
        return Some("They're already staff.".into());
    }
    if suspended {
        return Some("Their account is suspended.".into());
    }
    match (email, twofa) {
        (true, true) => None,
        (false, true) => Some("They need to confirm their email address first.".into()),
        (true, false) => Some("They need to turn on two-factor sign-in first.".into()),
        (false, false) => {
            Some("They need to confirm their email and turn on two-factor sign-in first.".into())
        }
    }
}

#[derive(Deserialize)]
pub struct StaffBody {
    #[serde(default)]
    username: String,
    #[serde(default)]
    tier: String,
    /// Only for tier "custom".
    #[serde(default)]
    perms: Vec<String>,
}

fn tier_and_perms(body: &StaffBody) -> AppResult<(String, Vec<String>)> {
    match body.tier.as_str() {
        t @ ("support" | "moderator" | "senior") => Ok((t.to_string(), preset(t))),
        "custom" => {
            let perms = clean_perms(&body.perms);
            if perms.is_empty() {
                return Err(AppError::BadRequest("Tick at least one permission.".into()));
            }
            Ok(("custom".into(), perms))
        }
        _ => Err(AppError::BadRequest(
            "Pick Support, Moderator, Senior moderator or Custom.".into(),
        )),
    }
}

fn perm_summary(tier: &str, perms: &[String]) -> String {
    if tier == "custom" {
        format!("Custom: {}", perms.join(", "))
    } else {
        tier_label(tier).to_string()
    }
}

/// `POST /api/safety/staff` `{username, tier, perms?}`
pub async fn add(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Json(body): Json<StaffBody>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let (id, username, name, email, twofa, bot, suspended) =
        find_candidate(&st.db, &body.username).await?;
    let already: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM staff WHERE user_id = $1)")
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    if let Some(why) = refusal(
        email,
        twofa,
        bot,
        suspended,
        is_site_owner(&st.db, id).await?,
        already,
    ) {
        return Err(AppError::BadRequest(why));
    }
    let (tier, perms) = tier_and_perms(&body)?;
    sqlx::query("INSERT INTO staff (user_id, tier, perms, added_by) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(&tier)
        .bind(&perms)
        .bind(a.id())
        .execute(&st.db)
        .await?;
    super::safety::log_action(
        &st,
        Some(a.id()),
        "staff_added",
        None,
        Some(id),
        format!("Made @{username} staff ({})", perm_summary(&tier, &perms)),
    )
    .await;
    super::recovery::notify(
        &st.db,
        id,
        "You're now Outhouse staff",
        vec![
            format!("Hi {name},"),
            format!("The site owner made you Outhouse staff ({}). You'll find a STAFF section in User settings.", tier_label(&tier)),
            "Staff powers only work while two-factor sign-in is on and your email is confirmed, and only after signing in with your code. Everything you do as staff is logged.".into(),
        ],
    )
    .await;
    st.dispatch(Audience::User(id), ServerEvent::StaffChanged);
    Ok(Json(json!({ "ok": true, "id": id.to_string() })))
}

/// `PUT /api/safety/staff/{id}` `{tier, perms?}`
pub async fn update(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<StaffBody>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let (tier, perms) = tier_and_perms(&body)?;
    let n = sqlx::query("UPDATE staff SET tier = $2, perms = $3 WHERE user_id = $1")
        .bind(id)
        .bind(&tier)
        .bind(&perms)
        .execute(&st.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    super::safety::log_action(
        &st,
        Some(a.id()),
        "staff_changed",
        None,
        Some(id),
        format!(
            "Staff permissions changed to {}",
            perm_summary(&tier, &perms)
        ),
    )
    .await;
    st.dispatch(Audience::User(id), ServerEvent::StaffChanged);
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/safety/staff/{id}`: no longer staff, straight away (their
/// sessions keep working as a normal account). They get an email.
pub async fn remove(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let gone: Option<String> = sqlx::query_scalar(
        "DELETE FROM staff s USING users u WHERE s.user_id = $1 AND u.id = s.user_id RETURNING u.username",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?;
    let Some(username) = gone else {
        return Err(AppError::NotFound);
    };
    super::safety::log_action(
        &st,
        Some(a.id()),
        "staff_removed",
        None,
        Some(id),
        format!("@{username} is no longer staff"),
    )
    .await;
    super::recovery::notify(
        &st.db,
        id,
        "You're no longer Outhouse staff",
        vec![
            format!("Hi @{username},"),
            "The site owner removed your staff access. Your account works as normal; the STAFF section in User settings is gone.".into(),
        ],
    )
    .await;
    st.dispatch(Audience::User(id), ServerEvent::StaffChanged);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct SettingsBody {
    two_person_permanent_ip: Option<bool>,
    two_person_shut_down: Option<bool>,
    two_person_members: Option<i32>,
    staff_badge: Option<bool>,
}

/// `PUT /api/safety/staff-settings`
pub async fn set_settings(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Json(b): Json<SettingsBody>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    sqlx::query(
        "UPDATE site_settings SET
             two_person_permanent_ip = COALESCE($1, two_person_permanent_ip),
             two_person_shut_down = COALESCE($2, two_person_shut_down),
             two_person_members = COALESCE($3, two_person_members),
             staff_badge = COALESCE($4, staff_badge)
         WHERE id = 1",
    )
    .bind(b.two_person_permanent_ip)
    .bind(b.two_person_shut_down)
    .bind(b.two_person_members.map(|n| n.clamp(1, 100_000)))
    .bind(b.staff_badge)
    .execute(&st.db)
    .await?;
    let s = settings(&st.db).await;
    let on = |b: bool| if b { "on" } else { "off" };
    super::safety::log_action(
        &st,
        Some(a.id()),
        "staff_settings",
        None,
        None,
        format!(
            "Two-person rule: permanent suspension with IP block {}, shutting down outhouses with {}+ members {}. Staff badge on profiles {}",
            on(s.two_person_permanent_ip),
            s.two_person_members,
            on(s.two_person_shut_down),
            on(s.staff_badge)
        ),
    )
    .await;
    Ok(Json(settings_json(&s)))
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    #[serde(default, with = "outhouse_common::id::option_as_string")]
    staff: Option<i64>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default, with = "outhouse_common::id::option_as_string")]
    before: Option<i64>,
}

/// `GET /api/safety/staff/activity?staff=&action=&before=`: what staff did
/// (the owner's "Staff activity" view).
pub async fn activity(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Query(q): Query<ActivityQuery>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let action = q.action.as_deref().map(str::trim).filter(|x| !x.is_empty());
    let rows: Vec<(i64, i64, String, String, String, Option<String>, Option<i64>, Option<i64>, Option<i64>, String, DateTime<Utc>, Option<String>)> =
        sqlx::query_as(
            "SELECT l.id, l.actor_id, COALESCE(a.display_name, ''), COALESCE(a.username, ''), l.action,
                    l.staff_perm, l.report_id, l.target_user_id, l.guild_id, l.details, l.created_at, l.staff_tier
             FROM safety_log l LEFT JOIN users a ON a.id = l.actor_id
             WHERE l.staff_tier IS NOT NULL
               AND ($1::BIGINT IS NULL OR l.actor_id = $1)
               AND ($2::TEXT IS NULL OR l.action = $2)
               AND ($3::BIGINT IS NULL OR l.id < $3)
             ORDER BY l.id DESC LIMIT 101",
        )
        .bind(q.staff)
        .bind(action)
        .bind(q.before)
        .fetch_all(&st.db)
        .await?;
    let more = rows.len() > 100;
    let names: HashMap<i64, String> =
        sqlx::query_as::<_, (i64, String)>("SELECT id, display_name FROM users WHERE id = ANY($1)")
            .bind(rows.iter().filter_map(|r| r.7).collect::<Vec<_>>())
            .fetch_all(&st.db)
            .await?
            .into_iter()
            .collect();
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT action FROM safety_log WHERE staff_tier IS NOT NULL ORDER BY action",
    )
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!({
        "entries": rows.iter().take(100).map(|r| json!({
            "id": r.0.to_string(),
            "actor_id": r.1.to_string(),
            "actor_name": r.2,
            "actor_username": r.3,
            "action": r.4,
            "perm": r.5,
            "tier": r.11,
            "tier_label": r.11.as_deref().map(tier_label),
            "report_id": r.6.map(|x| x.to_string()),
            "target_user_id": r.7.map(|x| x.to_string()),
            "target_name": r.7.and_then(|t| names.get(&t).cloned()),
            "guild_id": r.8.map(|x| x.to_string()),
            "details": r.9,
            "created_at": r.10,
        })).collect::<Vec<_>>(),
        "has_more": more,
        "actions": actions,
    })))
}

/// For the weekly digest: actions per staff member in the last 7 days.
pub async fn week_summary(db: &PgPool) -> AppResult<Vec<Value>> {
    let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT u.username, COALESCE(s.tier, 'removed'),
                count(l.id),
                count(l.id) FILTER (WHERE l.action IN ('suspend', 'ip_block', 'outhouse_shut_down', 'transfer_started'))
         FROM safety_log l JOIN users u ON u.id = l.actor_id
         LEFT JOIN staff s ON s.user_id = l.actor_id
         WHERE l.staff_tier IS NOT NULL AND l.created_at > now() - interval '7 days'
         GROUP BY u.username, s.tier ORDER BY count(l.id) DESC, u.username",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .iter()
        .map(|(u, t, n, big)| json!({ "username": u, "tier": t, "tier_label": tier_label(t), "actions": n, "big_actions": big }))
        .collect())
}

// ---- Two-person rule ---------------------------------------------------------------------

pub const APPROVAL_HOURS: i64 = 72;

/// Does this action by this actor need a second person? (Never for the owner.)
pub async fn needs_approval(st: &AppState, a: &Actor, kind: &str, members: i64) -> bool {
    if a.owner {
        return false;
    }
    let s = settings(&st.db).await;
    match kind {
        "suspend_permanent_ip" => s.two_person_permanent_ip,
        "shut_down" => s.two_person_shut_down && members >= i64::from(s.two_person_members),
        _ => false,
    }
}

/// Files a "Pending approval" request (and tells everyone who could approve it).
#[allow(clippy::too_many_arguments)]
pub async fn request_approval(
    st: &AppState,
    a: &Actor,
    kind: &str,
    target_user: Option<i64>,
    guild_id: Option<i64>,
    report_id: Option<i64>,
    payload: Value,
    summary: String,
) -> AppResult<Value> {
    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO staff_requests (id, kind, requested_by, target_user_id, guild_id, report_id, payload, summary, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(hours => $9))",
    )
    .bind(id)
    .bind(kind)
    .bind(a.id())
    .bind(target_user)
    .bind(guild_id)
    .bind(report_id)
    .bind(sqlx::types::Json(&payload))
    .bind(&summary)
    .bind(APPROVAL_HOURS as i32)
    .execute(&st.db)
    .await?;
    super::safety::log_action(
        st,
        Some(a.id()),
        "approval_requested",
        report_id,
        target_user,
        format!("Asked for a second person's approval: {summary}"),
    )
    .await;
    tell_owner(
        st,
        format!("🛡 **Waiting for approval**\nA staff member asked to {summary}. Another senior moderator or you can approve or decline it within {APPROVAL_HOURS} hours (User settings → Approvals)."),
    )
    .await;
    notify_approvers(st, a.id());
    Ok(json!({ "pending_approval": true, "request_id": id.to_string(), "summary": summary }))
}

fn notify_approvers(st: &AppState, except: i64) {
    let st = st.clone();
    tokio::spawn(async move {
        let mut ids: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM staff")
            .fetch_all(&st.db)
            .await
            .unwrap_or_default();
        ids.extend(
            super::safety::site_owner_ids(&st.db)
                .await
                .unwrap_or_default(),
        );
        for id in ids {
            if id != except {
                st.dispatch(Audience::User(id), ServerEvent::StaffChanged);
            }
        }
    });
}

fn perms_for_kind(kind: &str) -> &'static [&'static str] {
    match kind {
        "suspend_permanent_ip" => &["suspend_permanent", "ips"],
        _ => &["shut_down"],
    }
}

async fn expire_requests(db: &PgPool) -> AppResult<()> {
    sqlx::query(
        "UPDATE staff_requests SET status = 'expired', decided_at = now()
         WHERE status = 'pending' AND expires_at <= now()",
    )
    .execute(db)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct RequestRow {
    id: i64,
    kind: String,
    requested_by: i64,
    requester_name: String,
    target_user_id: Option<i64>,
    target_name: Option<String>,
    guild_id: Option<i64>,
    guild_name: Option<String>,
    report_id: Option<i64>,
    payload: sqlx::types::Json<Value>,
    summary: String,
    status: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    decided_by_name: Option<String>,
    decided_at: Option<DateTime<Utc>>,
    decision_note: String,
    result: String,
}

const REQUEST_SELECT: &str =
    "SELECT r.id, r.kind, r.requested_by, q.display_name AS requester_name,
        r.target_user_id, t.display_name AS target_name, r.guild_id, g.name AS guild_name,
        r.report_id, r.payload, r.summary, r.status, r.created_at, r.expires_at,
        d.display_name AS decided_by_name, r.decided_at, r.decision_note, r.result
    FROM staff_requests r
    JOIN users q ON q.id = r.requested_by
    LEFT JOIN users t ON t.id = r.target_user_id
    LEFT JOIN guilds g ON g.id = r.guild_id
    LEFT JOIN users d ON d.id = r.decided_by";

fn request_json(r: &RequestRow, a: &Actor) -> Value {
    let mine = r.requested_by == a.id();
    json!({
        "id": r.id.to_string(),
        "kind": r.kind,
        "kind_label": if r.kind == "shut_down" { "Shut down an outhouse" } else { "Permanent suspension with IP block" },
        "requested_by": r.requested_by.to_string(),
        "requester_name": r.requester_name,
        "mine": mine,
        "target_user_id": r.target_user_id.map(|x| x.to_string()),
        "target_name": r.target_name,
        "guild_id": r.guild_id.map(|x| x.to_string()),
        "guild_name": r.guild_name,
        "report_id": r.report_id.map(|x| x.to_string()),
        "summary": r.summary,
        "reason": r.payload.0["reason"],
        "status": r.status,
        "created_at": r.created_at,
        "expires_at": r.expires_at,
        "decided_by_name": r.decided_by_name,
        "decided_at": r.decided_at,
        "decision_note": r.decision_note,
        "result": r.result,
        "can_decide": r.status == "pending" && !mine && perms_for_kind(&r.kind).iter().all(|p| a.can(p)),
    })
}

fn can_see_requests(a: &Actor) -> bool {
    a.owner || (a.can("suspend_permanent") && a.can("ips")) || a.can("shut_down")
}

/// `GET /api/safety/staff-requests`: waiting ones first, then the last 50 decided.
pub async fn list_requests(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
) -> AppResult<Json<Value>> {
    if !can_see_requests(&a) {
        return Err(AppError::Forbidden);
    }
    expire_requests(&st.db).await?;
    let pending = sqlx::query_as::<_, RequestRow>(&format!(
        "{REQUEST_SELECT} WHERE r.status = 'pending' ORDER BY r.created_at"
    ))
    .fetch_all(&st.db)
    .await?;
    let done = sqlx::query_as::<_, RequestRow>(&format!(
        "{REQUEST_SELECT} WHERE r.status <> 'pending' ORDER BY COALESCE(r.decided_at, r.created_at) DESC LIMIT 50"
    ))
    .fetch_all(&st.db)
    .await?;
    let visible = |r: &&RequestRow| {
        a.owner || perms_for_kind(&r.kind).iter().all(|p| a.can(p)) || r.requested_by == a.id()
    };
    Ok(Json(json!({
        "pending": pending.iter().filter(visible).map(|r| request_json(r, &a)).collect::<Vec<_>>(),
        "done": done.iter().filter(visible).map(|r| request_json(r, &a)).collect::<Vec<_>>(),
        "hours": APPROVAL_HOURS,
    })))
}

async fn load_request(st: &AppState, id: i64) -> AppResult<RequestRow> {
    sqlx::query_as::<_, RequestRow>(&format!("{REQUEST_SELECT} WHERE r.id = $1"))
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize, Default)]
pub struct DecideBody {
    #[serde(default)]
    note: Option<String>,
}

/// `POST /api/safety/staff-requests/{id}/approve`: another senior
/// moderator (with the same permissions) or the owner carries it out.
pub async fn approve(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<DecideBody>>,
) -> AppResult<Json<Value>> {
    decide(&st, &a, id, true, body.map(|b| b.0).unwrap_or_default()).await
}

/// `POST /api/safety/staff-requests/{id}/decline`
pub async fn decline(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<DecideBody>>,
) -> AppResult<Json<Value>> {
    decide(&st, &a, id, false, body.map(|b| b.0).unwrap_or_default()).await
}

async fn decide(
    st: &AppState,
    a: &Actor,
    id: i64,
    approve: bool,
    body: DecideBody,
) -> AppResult<Json<Value>> {
    expire_requests(&st.db).await?;
    let r = load_request(st, id).await?;
    for perm in perms_for_kind(&r.kind) {
        a.require(perm)?;
    }
    if r.status != "pending" {
        return Err(AppError::BadRequest(format!("It's already {}.", r.status)));
    }
    if r.requested_by == a.id() {
        return Err(AppError::Refused(
            "You can't approve or decline your own request: someone else has to.".into(),
        ));
    }
    if let Some(g) = r.guild_id {
        guard_guild(st, a, g).await?;
    }
    if let Some(t) = r.target_user_id {
        guard_target(st, a, t).await?;
        if t == a.id() {
            return Err(AppError::Refused(SOMEONE_ELSE.into()));
        }
    }
    let note: String = body
        .note
        .unwrap_or_default()
        .trim()
        .chars()
        .take(500)
        .collect();
    let claimed = sqlx::query(
        "UPDATE staff_requests SET status = $2, decided_by = $3, decided_at = now(), decision_note = $4
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .bind(if approve { "approved" } else { "declined" })
    .bind(a.id())
    .bind(&note)
    .execute(&st.db)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Err(AppError::Conflict("Someone else just decided it.".into()));
    }
    super::safety::log_action(
        st,
        Some(a.id()),
        if approve {
            "approval_given"
        } else {
            "approval_declined"
        },
        r.report_id,
        r.target_user_id,
        format!(
            "{} {}'s request to {}{}",
            if approve { "Approved" } else { "Declined" },
            r.requester_name,
            r.summary,
            if note.is_empty() {
                String::new()
            } else {
                format!(". Note: {note}")
            }
        ),
    )
    .await;
    let mut result = String::new();
    if approve {
        // Carried out as the requester's action (their tier and permission).
        let requester = auth::fetch_user(&st.db, r.requested_by).await?;
        let req_token_free = actor_without_session(st, requester).await?;
        set_ctx(
            req_token_free.id(),
            &req_token_free.tier,
            perms_for_kind(&r.kind)[0],
        );
        let outcome = match r.kind.as_str() {
            "suspend_permanent_ip" => {
                super::ipblocks::carry_out(
                    st,
                    &req_token_free,
                    r.target_user_id.unwrap_or(0),
                    &r.payload.0,
                )
                .await
            }
            _ => {
                super::outhouses::carry_out_shut_down(
                    st,
                    &req_token_free,
                    r.guild_id.unwrap_or(0),
                    &r.payload.0,
                )
                .await
            }
        };
        // Back to the approver for anything logged after this.
        if !a.owner {
            set_ctx(a.id(), &a.tier, perms_for_kind(&r.kind)[0]);
        }
        result = match outcome {
            Ok(text) => text,
            Err(e) => {
                sqlx::query("UPDATE staff_requests SET status = 'failed' WHERE id = $1")
                    .bind(id)
                    .execute(&st.db)
                    .await?;
                format!("It couldn't be done: {e}")
            }
        };
        sqlx::query("UPDATE staff_requests SET result = $2 WHERE id = $1")
            .bind(id)
            .bind(&result)
            .execute(&st.db)
            .await?;
    }
    st.dispatch(Audience::User(r.requested_by), ServerEvent::StaffChanged);
    let _ = super::safety::safety_note(
        st,
        r.requested_by,
        format!(
            "🛡 Your request to {} was {} by {}.{}",
            r.summary,
            if approve { "approved" } else { "declined" },
            a.user.display_name,
            if result.is_empty() {
                String::new()
            } else {
                format!(" {result}")
            }
        ),
    )
    .await;
    let r = load_request(st, id).await?;
    Ok(Json(request_json(&r, a)))
}

/// `POST /api/safety/staff-requests/{id}/cancel`: the requester changes their mind.
pub async fn cancel(
    StaffActor(a): StaffActor,
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    let r = load_request(&st, id).await?;
    if r.requested_by != a.id() && !a.owner {
        return Err(AppError::Forbidden);
    }
    let n = sqlx::query(
        "UPDATE staff_requests SET status = 'cancelled', decided_by = $2, decided_at = now()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .bind(a.id())
    .execute(&st.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::BadRequest("It isn't waiting any more.".into()));
    }
    if !a.owner {
        set_ctx(a.id(), &a.tier, perms_for_kind(&r.kind)[0]);
    }
    super::safety::log_action(
        &st,
        Some(a.id()),
        "approval_cancelled",
        r.report_id,
        r.target_user_id,
        format!("Cancelled the request to {}", r.summary),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// The requester as an actor when their request is carried out (their
/// session isn't involved; they must still be active staff).
async fn actor_without_session(st: &AppState, user: User) -> AppResult<Actor> {
    if is_site_owner(&st.db, user.id).await? {
        return Ok(Actor::owner_of(user));
    }
    let row = staff_row(&st.db, user.id)
        .await?
        .ok_or_else(|| AppError::BadRequest("The person who asked isn't staff any more.".into()))?;
    if check_lapse(st, user.id).await?.is_some() {
        return Err(AppError::BadRequest(
            "The person who asked has their staff powers paused.".into(),
        ));
    }
    let perms = clean_perms(&row.perms);
    let rank = if row.tier == "custom" {
        rank_of(&perms)
    } else {
        tier_rank(&row.tier)
    };
    Ok(Actor {
        user,
        owner: false,
        tier: row.tier,
        perms,
        rank,
    })
}

/// An active staff member as an actor, without a session (for badges and
/// alerts); None if they aren't staff or their powers are paused.
pub async fn actor_quiet(st: &AppState, user: User) -> AppResult<Option<Actor>> {
    if staff_row(&st.db, user.id).await?.is_none() {
        return Ok(None);
    }
    Ok(actor_without_session(st, user).await.ok())
}

/// Hourly: expire old requests.
pub fn spawn_sweeper(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        loop {
            tick.tick().await;
            if let Err(e) = expire_requests(&st.db).await {
                tracing::warn!(error = %e, "couldn't expire staff requests");
            }
            let _ = sqlx::query("DELETE FROM staff_actions WHERE at < now() - interval '2 days'")
                .execute(&st.db)
                .await;
        }
    });
}

// ---- Last active (for inactive-owner transfers) ---------------------------------------------

static TOUCHED: Mutex<Option<HashMap<i64, Instant>>> = Mutex::new(None);

/// Notes that someone used Outhouse (at most once an hour per account).
pub fn touch(st: &AppState, user_id: i64) {
    {
        let mut guard = TOUCHED.lock().expect("touch lock");
        let map = guard.get_or_insert_with(HashMap::new);
        if map
            .get(&user_id)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3600))
        {
            return;
        }
        map.insert(user_id, Instant::now());
        if map.len() > 50_000 {
            map.retain(|_, t| t.elapsed() < Duration::from_secs(3600));
        }
    }
    let db = st.db.clone();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query("UPDATE users SET last_active_at = now() WHERE id = $1")
            .bind(user_id)
            .execute(&db)
            .await
        {
            tracing::warn!(error = %e, "couldn't note last activity");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_nest() {
        let s = preset("support");
        let m = preset("moderator");
        let x = preset("senior");
        assert!(s.iter().all(|p| m.contains(p)));
        assert!(m.iter().all(|p| x.contains(p)));
        assert!(s.contains(&"people".to_string()) && !s.contains(&"warn".to_string()));
        assert!(m.contains(&"warn".to_string()) && !m.contains(&"ips".to_string()));
        assert!(x.contains(&"transfers".to_string()));
        assert_eq!(x.len(), PERMS.len());
        assert_eq!(rank_of(&["people".into()]), 1);
        assert_eq!(rank_of(&["people".into(), "ips".into()]), 3);
        assert_eq!(
            clean_perms(&["ips".into(), "nonsense".into()]),
            vec!["ips".to_string()]
        );
    }
}
