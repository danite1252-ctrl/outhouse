//! Handing an outhouse to someone else.
//!
//! 1. **The owner hands it over** (`POST /api/guilds/{id}/transfer`): they
//!    pick a member, type that person's username exactly, their own
//!    password and (with two-factor on) a code. The recipient has 7 days to
//!    accept; the owner can cancel until then; a decline tells the owner.
//! 2. **Staff help** (`POST /api/safety/outhouses/{id}/transfer`, a senior
//!    moderator with "transfers", or the site owner):
//!    - *Inactive owner*: the owner hasn't been active for
//!      `OWNER_INACTIVE_DAYS` (90) days and one of the outhouse's admins
//!      asked for it. The owner is told (email + in-app) and has
//!      `OWNER_OBJECTION_DAYS` (14) days to press "I'm still here". The new
//!      owner confirms their request by accepting, any time from the start
//!      (so staff can't hand an outhouse to someone who didn't ask); it
//!      completes when the window ends *and* they've accepted. If they
//!      haven't by then, they get 7 more days.
//!    - *Owner's request with identity check*: a single-use link (24 hours,
//!      only its hash is kept) goes to the owner's confirmed email; once
//!      they confirm, the new owner accepts as in 1. No confirmed email: use
//!      the inactive route.
//!
//! Never while the outhouse is shut down or has an open child-safety report
//! about it. Only one transfer at a time per outhouse. The recipient must be
//! a member, not a bot, not suspended, not a teen (unless it's a family
//! outhouse, or the site owner allows it for this transfer) and have a
//! confirmed email. Everything goes in the outhouse's audit log and the
//! safety log; both people get an email and a note from Outhouse.

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::permissions as p;
use serde::Deserialize;
use serde_json::{json, Value};

use super::staff::StaffActor;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::mail;
use crate::state::{AppState, Audience};

pub const ACCEPT_DAYS: i64 = 7;
pub const VERIFY_HOURS: i64 = 24;
const LINK_DEAD: &str =
    "This link has expired or was already used. Ask Outhouse staff for a new one.";
const PENDING: &str = "('awaiting_owner', 'objection', 'awaiting_recipient')";

fn env_days(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0)
        .unwrap_or(default)
}

/// `OWNER_INACTIVE_DAYS` (default 90).
pub fn owner_inactive_days() -> i64 {
    env_days("OWNER_INACTIVE_DAYS", 90)
}

/// `OWNER_OBJECTION_DAYS` (default 14).
pub fn objection_days() -> i64 {
    env_days("OWNER_OBJECTION_DAYS", 14)
}

#[derive(sqlx::FromRow, Clone)]
struct Row {
    id: i64,
    guild_id: i64,
    guild_name: String,
    from_user: i64,
    from_username: String,
    from_name: String,
    to_user: i64,
    to_username: String,
    to_name: String,
    route: String,
    status: String,
    keep_admin: bool,
    allow_teen: bool,
    staff_id: Option<i64>,
    reason: String,
    recipient_accepted_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    deadline: DateTime<Utc>,
    objection_until: Option<DateTime<Utc>>,
    owner_confirmed_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    outcome: String,
}

const SELECT: &str = "SELECT t.id, t.guild_id, g.name AS guild_name,
        t.from_user, f.username AS from_username, f.display_name AS from_name,
        t.to_user, r.username AS to_username, r.display_name AS to_name,
        t.route, t.status, t.keep_admin, t.allow_teen, t.staff_id, t.reason,
        t.recipient_accepted_at, t.created_at, t.deadline, t.objection_until,
        t.owner_confirmed_at, t.finished_at, t.outcome
    FROM ownership_transfers t
    JOIN guilds g ON g.id = t.guild_id
    JOIN users f ON f.id = t.from_user
    JOIN users r ON r.id = t.to_user";

fn route_label(route: &str) -> &'static str {
    match route {
        "inactive" => "Staff-assisted: the owner has been inactive",
        "verified" => "Staff-assisted: the owner confirmed by email",
        _ => "The owner handed it over",
    }
}

fn row_json(t: &Row) -> Value {
    json!({
        "id": t.id.to_string(),
        "guild_id": t.guild_id.to_string(),
        "guild_name": t.guild_name,
        "from": { "id": t.from_user.to_string(), "username": t.from_username, "display_name": t.from_name },
        "to": { "id": t.to_user.to_string(), "username": t.to_username, "display_name": t.to_name },
        "route": t.route,
        "route_label": route_label(&t.route),
        "status": t.status,
        "keep_admin": t.keep_admin,
        "allow_teen": t.allow_teen,
        "staff": t.staff_id.is_some(),
        "reason": t.reason,
        "recipient_accepted": t.recipient_accepted_at.is_some(),
        "created_at": t.created_at,
        "deadline": t.deadline,
        "objection_until": t.objection_until,
        "owner_confirmed_at": t.owner_confirmed_at,
        "finished_at": t.finished_at,
        "outcome": t.outcome,
    })
}

async fn load(st: &AppState, id: i64) -> AppResult<Row> {
    sqlx::query_as::<_, Row>(&format!("{SELECT} WHERE t.id = $1"))
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)
}

/// The transfer under way for an outhouse (for the directory), or null.
pub async fn pending_for_guild(st: &AppState, guild_id: i64) -> AppResult<Value> {
    let row = sqlx::query_as::<_, Row>(&format!(
        "{SELECT} WHERE t.guild_id = $1 AND t.status IN {PENDING}"
    ))
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await?;
    Ok(row.as_ref().map(row_json).unwrap_or(Value::Null))
}

// ---- Checks --------------------------------------------------------------------------

struct GuildInfo {
    name: String,
    owner_id: i64,
    family: bool,
}

/// Not while it's shut down or has an open child-safety report about it.
async fn check_guild(st: &AppState, guild_id: i64) -> AppResult<GuildInfo> {
    let (name, owner_id, family, shut): (String, i64, bool, bool) = sqlx::query_as(
        "SELECT name, owner_id, family, shut_down_at IS NOT NULL FROM guilds WHERE id = $1 AND id <> 0",
    )
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    if shut {
        return Err(AppError::BadRequest(
            "It's shut down, so its ownership can't change.".into(),
        ));
    }
    let child: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM reports WHERE guild_id = $1 AND category = 'child_safety'
                       AND status <> 'closed')",
    )
    .bind(guild_id)
    .fetch_one(&st.db)
    .await?;
    if child {
        return Err(AppError::Conflict(
            "There's an open child-safety report about this outhouse, so its ownership can't change until it's closed.".into(),
        ));
    }
    Ok(GuildInfo {
        name,
        owner_id,
        family,
    })
}

/// Can this person own the outhouse? Their (username, display name).
async fn check_recipient(
    st: &AppState,
    guild: &GuildInfo,
    guild_id: i64,
    to: i64,
    allow_teen: bool,
) -> AppResult<(String, String)> {
    let row: Option<(String, String, bool, bool, String, bool, bool)> = sqlx::query_as(
        "SELECT u.username, u.display_name, u.is_bot, COALESCE(u.suspended_until > now(), false),
                u.age_bracket, u.email_verified_at IS NOT NULL,
                EXISTS(SELECT 1 FROM guild_members m WHERE m.guild_id = $2 AND m.user_id = u.id)
         FROM users u WHERE u.id = $1",
    )
    .bind(to)
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await?;
    let Some((username, name, bot, suspended, bracket, email, member)) = row else {
        return Err(AppError::BadRequest("Nobody has that username.".into()));
    };
    if to == guild.owner_id {
        return Err(AppError::BadRequest("They already own it.".into()));
    }
    if !member {
        return Err(AppError::BadRequest(
            "They need to be a member of the outhouse.".into(),
        ));
    }
    if bot {
        return Err(AppError::BadRequest("Bots can't own an outhouse.".into()));
    }
    if suspended {
        return Err(AppError::BadRequest("Their account is suspended.".into()));
    }
    if bracket == "teen" && !guild.family && !allow_teen {
        return Err(AppError::BadRequest(
            "Teen accounts can't own an outhouse unless it's a family outhouse.".into(),
        ));
    }
    if !email {
        return Err(AppError::BadRequest(
            "They need a confirmed email address first (User settings → My account).".into(),
        ));
    }
    Ok((username, name))
}

async fn user_by_name(st: &AppState, username: &str) -> AppResult<i64> {
    let name = username.trim().trim_start_matches('@').to_lowercase();
    sqlx::query_scalar("SELECT id FROM users WHERE lower(username) = $1")
        .bind(&name)
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| AppError::BadRequest("Nobody has that username.".into()))
}

fn insert_error(e: sqlx::Error) -> AppError {
    match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => AppError::Conflict(
            "There's already a transfer waiting for this outhouse. Cancel it first.".into(),
        ),
        other => other.into(),
    }
}

// ---- Telling people ----------------------------------------------------------------------

/// An email (with a button to the app) and a note from Outhouse.
async fn tell(
    st: &AppState,
    user_id: i64,
    subject: &str,
    paragraphs: Vec<String>,
    button: Option<(&str, String)>,
) {
    if let Err(e) = super::safety::safety_note(
        st,
        user_id,
        format!("🔑 **{subject}**\n{}", paragraphs.join("\n")),
    )
    .await
    {
        tracing::warn!(error = %e, "couldn't send a transfer note");
    }
    if mail::configured() {
        if let Some(to) = super::recovery::confirmed_email(&st.db, user_id).await {
            let url = button.as_ref().and_then(|(_, path)| mail::link(path));
            mail::queue(mail::compose(
                &to,
                subject,
                &paragraphs,
                match (&button, &url) {
                    (Some((label, _)), Some(u)) => Some((*label, u.as_str())),
                    _ => None,
                },
                mail::FOOTER,
            ));
        }
    }
    st.dispatch(Audience::User(user_id), ServerEvent::TransfersChanged);
}

fn app_link(id: i64) -> String {
    format!("/app?transfer={id}")
}

fn day(at: DateTime<Utc>) -> String {
    at.format("%-d %B %Y").to_string()
}

async fn ask_recipient(st: &AppState, t: &Row) {
    tell(
        st,
        t.to_user,
        &format!("{} wants you to own {}", if t.staff_id.is_some() { "Outhouse staff" } else { t.from_name.as_str() }, t.guild_name),
        vec![
            format!("Hi {},", t.to_name),
            if t.route == "owner" {
                format!("{} (@{}) wants to hand you ownership of the outhouse {}.", t.from_name, t.from_username, t.guild_name)
            } else {
                format!("Outhouse staff are handing you ownership of the outhouse {} (after your request).", t.guild_name)
            },
            "As the owner you get full control of it: settings, roles, channels and members. Open Outhouse to accept or decline.".into(),
            format!("This offer ends on {}.", day(t.deadline)),
        ],
        Some(("Open Outhouse", app_link(t.id))),
    )
    .await;
}

// ---- Completing ------------------------------------------------------------------------------

/// Hands the outhouse over (re-checking everything first). Returns the
/// finished transfer, or why it couldn't happen (then it's cancelled).
async fn complete(st: &AppState, id: i64, by: Option<i64>) -> AppResult<Row> {
    let t = load(st, id).await?;
    let checks = async {
        let g = check_guild(st, t.guild_id).await?;
        if g.owner_id != t.from_user {
            return Err(AppError::BadRequest(
                "Its owner changed in the meantime.".into(),
            ));
        }
        check_recipient(st, &g, t.guild_id, t.to_user, t.allow_teen).await?;
        Ok::<GuildInfo, AppError>(g)
    }
    .await;
    if let Err(e) = checks {
        sqlx::query(&format!(
            "UPDATE ownership_transfers SET status = 'cancelled', finished_at = now(), outcome = $2
             WHERE id = $1 AND status IN {PENDING}"
        ))
        .bind(id)
        .bind(format!("Couldn't be completed: {e}"))
        .execute(&st.db)
        .await?;
        for u in [t.from_user, t.to_user] {
            st.dispatch(Audience::User(u), ServerEvent::TransfersChanged);
        }
        return Err(e);
    }
    let mut tx = st.db.begin().await?;
    let moved = sqlx::query("UPDATE guilds SET owner_id = $2 WHERE id = $1 AND owner_id = $3")
        .bind(t.guild_id)
        .bind(t.to_user)
        .bind(t.from_user)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if moved == 0 {
        return Err(AppError::Conflict(
            "Its owner changed in the meantime.".into(),
        ));
    }
    let mut admin_note = String::new();
    if t.keep_admin {
        let still_member: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM guild_members WHERE guild_id = $1 AND user_id = $2)",
        )
        .bind(t.guild_id)
        .bind(t.from_user)
        .fetch_one(&mut *tx)
        .await?;
        if still_member {
            let existing: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM roles WHERE guild_id = $1 AND id <> $1 AND (permissions & $2) <> 0
                 ORDER BY (lower(name) = 'administrator') DESC, position DESC LIMIT 1",
            )
            .bind(t.guild_id)
            .bind(p::ADMINISTRATOR as i64)
            .fetch_optional(&mut *tx)
            .await?;
            let role = match existing {
                Some(r) => r,
                None => {
                    let top: i32 = sqlx::query_scalar(
                        "SELECT COALESCE(max(position), 0) FROM roles WHERE guild_id = $1",
                    )
                    .bind(t.guild_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    let rid = st.ids.next_id();
                    sqlx::query(
                        "INSERT INTO roles (id, guild_id, name, color, permissions, deny_permissions, position, hoist)
                         VALUES ($1, $2, 'Administrator', 0, $3, 0, $4, false)",
                    )
                    .bind(rid)
                    .bind(t.guild_id)
                    .bind(p::ADMINISTRATOR as i64)
                    .bind(top + 1)
                    .execute(&mut *tx)
                    .await?;
                    rid
                }
            };
            sqlx::query(
                "INSERT INTO member_roles (guild_id, user_id, role_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(t.guild_id)
            .bind(t.from_user)
            .bind(role)
            .execute(&mut *tx)
            .await?;
            admin_note = format!(" {} keeps an Administrator role.", t.from_name);
        }
    }
    sqlx::query(
        "UPDATE ownership_transfers SET status = 'completed', finished_at = now(), finished_by = $2,
                outcome = 'Completed'
         WHERE id = $1",
    )
    .bind(id)
    .bind(by)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let guild = super::guilds::fetch_guild(&st.db, t.guild_id).await?;
    st.dispatch(Audience::Guild(t.guild_id), ServerEvent::GuildUpdate(guild));
    st.dispatch(
        Audience::Guild(t.guild_id),
        ServerEvent::GuildRefresh {
            guild_id: t.guild_id,
        },
    );
    crate::audit::Entry::new(t.guild_id, by.unwrap_or(t.to_user), "owner_transfer")
        .target("member", t.to_user, t.to_name.clone())
        .details(format!(
            "Ownership moved from {} to {} ({}).{admin_note}",
            t.from_name,
            t.to_name,
            route_label(&t.route)
        ))
        .save(st)
        .await;
    super::safety::log_for_guild(
        st,
        by,
        "transfer_completed",
        t.guild_id,
        Some(t.to_user),
        format!(
            "Ownership of {} moved from @{} to @{} ({}).{admin_note}",
            t.guild_name,
            t.from_username,
            t.to_username,
            route_label(&t.route)
        ),
    )
    .await;
    tell(
        st,
        t.to_user,
        &format!("You now own {}", t.guild_name),
        vec![
            format!("Hi {},", t.to_name),
            format!(
                "You're now the owner of {}. You have full control of it.",
                t.guild_name
            ),
        ],
        Some(("Open Outhouse", "/app".into())),
    )
    .await;
    tell(
        st,
        t.from_user,
        &format!("{} has a new owner", t.guild_name),
        vec![
            format!("Hi {},", t.from_name),
            format!(
                "{} (@{}) is now the owner of {}.{}",
                t.to_name,
                t.to_username,
                t.guild_name,
                if admin_note.is_empty() {
                    String::new()
                } else {
                    " You kept an Administrator role.".into()
                }
            ),
            "If you didn't expect this, contact the site owner.".into(),
        ],
        None,
    )
    .await;
    load(st, id).await
}

// ---- The owner hands it over --------------------------------------------------------------------

#[derive(Deserialize)]
pub struct OwnerStart {
    #[serde(with = "outhouse_common::id::as_string")]
    user_id: i64,
    #[serde(default)]
    confirm_username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    keep_admin: bool,
}

/// `POST /api/guilds/{id}/transfer`
pub async fn owner_start(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<OwnerStart>,
) -> AppResult<Json<Value>> {
    let ctx = crate::perms::member_ctx(&st.db, guild_id, user.id).await?;
    if !ctx.is_owner() || user.bot {
        return Err(AppError::Forbidden);
    }
    let g = check_guild(&st, guild_id).await?;
    let (username, _) = check_recipient(&st, &g, guild_id, req.user_id, false).await?;
    if req.confirm_username.trim().trim_start_matches('@') != username {
        return Err(AppError::BadRequest(format!(
            "Type their username exactly ({username}) to confirm."
        )));
    }
    if crate::security::get().rate_limits
        && !crate::security::limiter().allow(
            &format!("transfer:{}", user.id),
            10,
            crate::security::LOGIN_WINDOW,
        )
    {
        return Err(AppError::TooManyRequests(
            "Too many tries. Wait 15 minutes.".into(),
        ));
    }
    super::twofactor::require_password(&st.db, user.id, &req.password).await?;
    if super::twofactor::is_enabled(&st.db, user.id).await? {
        if req.code.trim().is_empty() {
            return Err(AppError::BadRequest(
                "Enter the code from your authenticator app.".into(),
            ));
        }
        super::twofactor::require_code(&st, user.id, req.code.trim()).await?;
    }
    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO ownership_transfers (id, guild_id, from_user, to_user, route, status, keep_admin, deadline)
         VALUES ($1, $2, $3, $4, 'owner', 'awaiting_recipient', $5, now() + make_interval(days => $6))",
    )
    .bind(id)
    .bind(guild_id)
    .bind(user.id)
    .bind(req.user_id)
    .bind(req.keep_admin)
    .bind(ACCEPT_DAYS as i32)
    .execute(&st.db)
    .await
    .map_err(insert_error)?;
    let t = load(&st, id).await?;
    crate::audit::Entry::new(guild_id, user.id, "owner_transfer_start")
        .target("member", t.to_user, t.to_name.clone())
        .details(format!(
            "Offered ownership to {} (they have {ACCEPT_DAYS} days to accept)",
            t.to_name
        ))
        .save(&st)
        .await;
    super::safety::log_for_guild(
        &st,
        Some(user.id),
        "transfer_started",
        guild_id,
        Some(t.to_user),
        format!(
            "@{} offered ownership of {} to @{}",
            t.from_username, g.name, t.to_username
        ),
    )
    .await;
    ask_recipient(&st, &t).await;
    st.dispatch(Audience::User(user.id), ServerEvent::TransfersChanged);
    Ok(Json(row_json(&t)))
}

/// `GET /api/users/@me/transfers`: offers to me, my own offers waiting, and
/// staff transfers of outhouses I own that I can still stop.
pub async fn mine(State(st): State<AppState>, AuthUser(user): AuthUser) -> AppResult<Json<Value>> {
    let incoming = sqlx::query_as::<_, Row>(&format!(
        "{SELECT} WHERE t.to_user = $1 AND (t.status = 'awaiting_recipient'
              OR (t.status = 'objection' AND t.recipient_accepted_at IS NULL))
         ORDER BY t.created_at"
    ))
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let outgoing = sqlx::query_as::<_, Row>(&format!(
        "{SELECT} WHERE t.from_user = $1 AND t.status IN {PENDING} ORDER BY t.created_at"
    ))
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!({
        "incoming": incoming.iter().map(row_json).collect::<Vec<_>>(),
        // The owner's own offers they can cancel.
        "outgoing": outgoing.iter().filter(|t| t.route == "owner").map(row_json).collect::<Vec<_>>(),
        // Staff transfers of my outhouses: "I'm still here" (inactive route).
        "objections": outgoing.iter().filter(|t| t.route == "inactive").map(row_json).collect::<Vec<_>>(),
        "inactive_days": owner_inactive_days(),
    })))
}

/// `POST /api/transfers/{id}/accept`
pub async fn accept(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    let t = load(&st, id).await?;
    if t.to_user != user.id {
        return Err(AppError::NotFound);
    }
    match t.status.as_str() {
        "awaiting_recipient" => {
            let done = complete(&st, id, Some(user.id)).await?;
            Ok(Json(row_json(&done)))
        }
        "objection" => {
            // Inactive owner: their acceptance confirms the request; it
            // completes when the owner's window ends.
            sqlx::query(
                "UPDATE ownership_transfers SET recipient_accepted_at = now() WHERE id = $1",
            )
            .bind(id)
            .execute(&st.db)
            .await?;
            super::safety::log_for_guild(
                &st,
                Some(user.id),
                "transfer_accepted",
                t.guild_id,
                Some(user.id),
                format!(
                    "@{} accepted ownership of {}; it completes on {} unless the owner objects",
                    t.to_username,
                    t.guild_name,
                    t.objection_until.map(day).unwrap_or_default()
                ),
            )
            .await;
            st.dispatch(Audience::User(user.id), ServerEvent::TransfersChanged);
            Ok(Json(row_json(&load(&st, id).await?)))
        }
        _ => Err(AppError::BadRequest(
            "It isn't waiting for you any more.".into(),
        )),
    }
}

/// `POST /api/transfers/{id}/decline`
pub async fn decline(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    let t = load(&st, id).await?;
    if t.to_user != user.id {
        return Err(AppError::NotFound);
    }
    finish(
        &st,
        &t,
        "declined",
        Some(user.id),
        "Declined by the new owner",
    )
    .await?;
    tell(
        &st,
        t.from_user,
        &format!("{} declined ownership of {}", t.to_name, t.guild_name),
        vec![
            format!("Hi {},", t.from_name),
            format!(
                "{} (@{}) declined to take over {}. Nothing has changed: you're still the owner.",
                t.to_name, t.to_username, t.guild_name
            ),
        ],
        None,
    )
    .await;
    Ok(Json(row_json(&load(&st, id).await?)))
}

/// `POST /api/transfers/{id}/cancel`: the owner (their own offer), or staff.
pub async fn cancel(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: axum::http::HeaderMap,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    let t = load(&st, id).await?;
    let by_owner = t.from_user == user.id && t.route == "owner";
    if !by_owner {
        // Staff with the transfers permission (or the site owner).
        let token = auth::bearer_token(&headers).unwrap_or("").to_string();
        let a = super::staff::actor_for(&st, user.clone(), &token)
            .await
            .map_err(|_| AppError::NotFound)?;
        a.require("transfers")?;
    }
    finish(
        &st,
        &t,
        "cancelled",
        Some(user.id),
        if by_owner {
            "Cancelled by the owner"
        } else {
            "Cancelled by Outhouse staff"
        },
    )
    .await?;
    tell(
        &st,
        t.to_user,
        &format!("The offer to own {} was cancelled", t.guild_name),
        vec![
            format!("Hi {},", t.to_name),
            format!(
                "The transfer of {} to you was cancelled. Nothing has changed.",
                t.guild_name
            ),
        ],
        None,
    )
    .await;
    if !by_owner {
        st.dispatch(Audience::User(t.from_user), ServerEvent::TransfersChanged);
    }
    Ok(Json(row_json(&load(&st, id).await?)))
}

/// `POST /api/transfers/{id}/still-here`: the owner stops a staff transfer
/// made because they seemed inactive.
pub async fn still_here(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    let t = load(&st, id).await?;
    if t.from_user != user.id || t.route != "inactive" {
        return Err(AppError::NotFound);
    }
    finish(
        &st,
        &t,
        "objected",
        Some(user.id),
        "The owner said they're still here",
    )
    .await?;
    tell(
        &st,
        t.to_user,
        &format!("{} keeps its owner", t.guild_name),
        vec![
            format!("Hi {},", t.to_name),
            format!(
                "The owner of {} is still around and stopped the transfer. Nothing has changed.",
                t.guild_name
            ),
        ],
        None,
    )
    .await;
    if let Some(staff) = t.staff_id {
        let _ = super::safety::safety_note(
            &st,
            staff,
            format!(
                "🔑 The owner of {} pressed \"I'm still here\": the transfer to @{} is stopped.",
                t.guild_name, t.to_username
            ),
        )
        .await;
    }
    Ok(Json(row_json(&load(&st, id).await?)))
}

async fn finish(
    st: &AppState,
    t: &Row,
    status: &str,
    by: Option<i64>,
    outcome: &str,
) -> AppResult<()> {
    let n = sqlx::query(&format!(
        "UPDATE ownership_transfers SET status = $2, finished_at = now(), finished_by = $3, outcome = $4,
                owner_token_hash = NULL
         WHERE id = $1 AND status IN {PENDING}"
    ))
    .bind(t.id)
    .bind(status)
    .bind(by)
    .bind(outcome)
    .execute(&st.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::BadRequest("It isn't waiting any more.".into()));
    }
    super::safety::log_for_guild(
        st,
        by,
        &format!("transfer_{status}"),
        t.guild_id,
        Some(t.to_user),
        format!(
            "Transfer of {} to @{}: {outcome}",
            t.guild_name, t.to_username
        ),
    )
    .await;
    if t.route == "owner" {
        crate::audit::Entry::new(t.guild_id, by.unwrap_or(t.from_user), "owner_transfer_end")
            .target("member", t.to_user, t.to_name.clone())
            .details(outcome.to_string())
            .save(st)
            .await;
    }
    for u in [t.from_user, t.to_user] {
        st.dispatch(Audience::User(u), ServerEvent::TransfersChanged);
    }
    Ok(())
}

// ---- Staff-assisted ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct StaffStart {
    #[serde(default)]
    route: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    keep_admin: bool,
    /// Site owner only: let a teen own this (non-family) outhouse.
    #[serde(default)]
    allow_teen: bool,
}

/// `POST /api/safety/outhouses/{id}/transfer` `{route: inactive|verified, username, reason, keep_admin, allow_teen}`
pub async fn staff_start(
    State(st): State<AppState>,
    StaffActor(a): StaffActor,
    Path(guild_id): Path<i64>,
    Json(req): Json<StaffStart>,
) -> AppResult<Json<Value>> {
    a.require("transfers")?;
    super::staff::guard_guild(&st, &a, guild_id).await?;
    let g = check_guild(&st, guild_id).await?;
    super::staff::guard_target(&st, &a, g.owner_id).await?;
    if req.allow_teen && !a.owner {
        return Err(AppError::Forbidden);
    }
    let reason: String = req.reason.trim().chars().take(500).collect();
    if reason.is_empty() {
        return Err(AppError::BadRequest(
            "Write why (who asked, and what they said). It goes in the safety log.".into(),
        ));
    }
    let to = user_by_name(&st, &req.username).await?;
    if to == a.id() && !a.owner {
        return Err(AppError::Refused(super::staff::SOMEONE_ELSE.into()));
    }
    let (to_username, _) = check_recipient(&st, &g, guild_id, to, req.allow_teen).await?;
    let id = st.ids.next_id();
    match req.route.as_str() {
        "inactive" => {
            let days = owner_inactive_days();
            let last: Option<DateTime<Utc>> =
                sqlx::query_scalar("SELECT last_active_at FROM users WHERE id = $1")
                    .bind(g.owner_id)
                    .fetch_one(&st.db)
                    .await?;
            let away = if st.is_online(g.owner_id) {
                0
            } else {
                last.map(|t| (Utc::now() - t).num_days())
                    .unwrap_or(i64::MAX)
            };
            if away < days {
                return Err(AppError::BadRequest(format!(
                    "The owner was active {away} day{} ago. This route needs {days}+ days without activity; use the email check instead.",
                    if away == 1 { "" } else { "s" }
                )));
            }
            let ctx = crate::perms::member_ctx(&st.db, guild_id, to).await?;
            if ctx.guild_perms() & (p::ADMINISTRATOR | p::MANAGE_GUILD) == 0 {
                return Err(AppError::BadRequest(
                    "The new owner must be an admin of the outhouse (Administrator or Manage outhouse).".into(),
                ));
            }
            super::staff::charge(&st, &a, true).await?;
            let window = objection_days();
            sqlx::query(
                "INSERT INTO ownership_transfers (id, guild_id, from_user, to_user, route, status, keep_admin,
                                                  allow_teen, staff_id, reason, objection_until, deadline)
                 VALUES ($1, $2, $3, $4, 'inactive', 'objection', $5, $6, $7, $8,
                         now() + make_interval(days => $9), now() + make_interval(days => $9 + $10))",
            )
            .bind(id)
            .bind(guild_id)
            .bind(g.owner_id)
            .bind(to)
            .bind(req.keep_admin)
            .bind(req.allow_teen)
            .bind(a.id())
            .bind(&reason)
            .bind(window as i32)
            .bind(ACCEPT_DAYS as i32)
            .execute(&st.db)
            .await
            .map_err(insert_error)?;
            let t = load(&st, id).await?;
            super::safety::log_for_guild(
                &st,
                Some(a.id()),
                "transfer_started",
                guild_id,
                Some(to),
                format!(
                    "Started an inactive-owner transfer of {} from @{} (away {away} days) to @{to_username}. The owner has until {} to object. Reason: {reason}",
                    g.name,
                    t.from_username,
                    t.objection_until.map(day).unwrap_or_default()
                ),
            )
            .await;
            tell(
                &st,
                g.owner_id,
                &format!("Is {} still yours?", g.name),
                vec![
                    format!("Hi {},", t.from_name),
                    format!(
                        "You haven't used Outhouse for a while, and an admin of {} asked for ownership of it so it can keep running. Outhouse staff have started handing it to {} (@{}).",
                        g.name, t.to_name, t.to_username
                    ),
                    format!(
                        "If you're still around and want to keep it, sign in and press \"I'm still here\" before {}. Otherwise it moves to them then.",
                        t.objection_until.map(day).unwrap_or_default()
                    ),
                ],
                Some(("I'm still here", app_link(id))),
            )
            .await;
            ask_recipient(&st, &t).await;
            Ok(Json(row_json(&t)))
        }
        "verified" => {
            if super::recovery::confirmed_email(&st.db, g.owner_id)
                .await
                .is_none()
            {
                return Err(AppError::BadRequest(
                    "The owner has no confirmed email, so this route isn't available. Use the inactive-owner route.".into(),
                ));
            }
            if !mail::configured() {
                return Err(AppError::BadRequest(
                    "This server can't send email, so the email check isn't available.".into(),
                ));
            }
            super::staff::charge(&st, &a, true).await?;
            let token = auth::new_token();
            sqlx::query(
                "INSERT INTO ownership_transfers (id, guild_id, from_user, to_user, route, status, keep_admin,
                                                  allow_teen, staff_id, reason, owner_token_hash, deadline)
                 VALUES ($1, $2, $3, $4, 'verified', 'awaiting_owner', $5, $6, $7, $8, $9,
                         now() + make_interval(hours => $10))",
            )
            .bind(id)
            .bind(guild_id)
            .bind(g.owner_id)
            .bind(to)
            .bind(req.keep_admin)
            .bind(req.allow_teen)
            .bind(a.id())
            .bind(&reason)
            .bind(auth::token_hash(&token))
            .bind(VERIFY_HOURS as i32)
            .execute(&st.db)
            .await
            .map_err(insert_error)?;
            let t = load(&st, id).await?;
            super::safety::log_for_guild(
                &st,
                Some(a.id()),
                "transfer_started",
                guild_id,
                Some(to),
                format!(
                    "Sent an ownership check to @{}'s confirmed email: transfer {} to @{to_username}? Reason: {reason}",
                    t.from_username, g.name
                ),
            )
            .await;
            if let (Some(to_addr), Some(url)) = (
                super::recovery::confirmed_email(&st.db, g.owner_id).await,
                mail::link(&format!("/confirm-transfer?token={token}")),
            ) {
                mail::queue(mail::compose(
                    &to_addr,
                    &format!("Confirm the transfer of {}", g.name),
                    &[
                        format!("Hi {},", t.from_name),
                        format!("Outhouse staff were asked to transfer your outhouse {} to {} (@{}).", g.name, t.to_name, t.to_username),
                        format!("Confirm you want to transfer {} to @{} with the button below. The link works once, for {VERIFY_HOURS} hours.", g.name, t.to_username),
                        "If you didn't ask for this, ignore this email: nothing changes, and please tell the site owner.".into(),
                    ],
                    Some(("Confirm the transfer", url.as_str())),
                    mail::FOOTER,
                ));
            }
            Ok(Json(row_json(&t)))
        }
        _ => Err(AppError::BadRequest(
            "Pick a route: inactive owner or email check.".into(),
        )),
    }
}

#[derive(Deserialize)]
pub struct TokenBody {
    #[serde(default)]
    token: String,
}

async fn by_token(st: &AppState, token: &str) -> AppResult<Row> {
    let token = token.trim();
    if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    }
    sqlx::query_as::<_, Row>(&format!(
        "{SELECT} WHERE t.owner_token_hash = $1 AND t.status = 'awaiting_owner' AND t.deadline > now()"
    ))
    .bind(auth::token_hash(token))
    .fetch_optional(&st.db)
    .await?
    .ok_or_else(|| AppError::BadRequest(LINK_DEAD.into()))
}

/// `POST /api/transfers/confirm-owner/check` `{token}`: what the link is for.
pub async fn confirm_check(
    State(st): State<AppState>,
    Json(req): Json<TokenBody>,
) -> AppResult<Json<Value>> {
    let t = by_token(&st, &req.token).await?;
    Ok(Json(json!({
        "guild_name": t.guild_name,
        "from_username": t.from_username,
        "to_username": t.to_username,
        "to_display_name": t.to_name,
        "expires_at": t.deadline,
    })))
}

/// `POST /api/transfers/confirm-owner` `{token}`: the owner confirms (the
/// link works once). The new owner is then asked to accept.
pub async fn confirm_owner(
    State(st): State<AppState>,
    Json(req): Json<TokenBody>,
) -> AppResult<Json<Value>> {
    let t = by_token(&st, &req.token).await?;
    let n = sqlx::query(
        "UPDATE ownership_transfers SET status = 'awaiting_recipient', owner_token_hash = NULL,
                owner_confirmed_at = now(), deadline = now() + make_interval(days => $2)
         WHERE id = $1 AND status = 'awaiting_owner'",
    )
    .bind(t.id)
    .bind(ACCEPT_DAYS as i32)
    .execute(&st.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::BadRequest(LINK_DEAD.into()));
    }
    super::safety::log_for_guild(
        &st,
        None,
        "transfer_owner_confirmed",
        t.guild_id,
        Some(t.to_user),
        format!(
            "@{} confirmed by email: transfer {} to @{}",
            t.from_username, t.guild_name, t.to_username
        ),
    )
    .await;
    let t = load(&st, t.id).await?;
    ask_recipient(&st, &t).await;
    st.dispatch(Audience::User(t.from_user), ServerEvent::TransfersChanged);
    Ok(Json(
        json!({ "ok": true, "guild_name": t.guild_name, "to_username": t.to_username }),
    ))
}

// ---- Background ---------------------------------------------------------------------------------

/// Completes inactive-owner transfers whose window ended (if the new owner
/// accepted), and expires anything left too long.
pub async fn sweep(st: &AppState) -> AppResult<usize> {
    let mut done = 0;
    let ripe: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT id, recipient_accepted_at IS NOT NULL FROM ownership_transfers
         WHERE status = 'objection' AND objection_until <= now()",
    )
    .fetch_all(&st.db)
    .await?;
    for (id, accepted) in ripe {
        if accepted {
            match complete(st, id, None).await {
                Ok(_) => done += 1,
                Err(e) => {
                    tracing::info!(error = %e, transfer = id, "an inactive-owner transfer couldn't complete")
                }
            }
        } else {
            sqlx::query("UPDATE ownership_transfers SET status = 'awaiting_recipient' WHERE id = $1 AND status = 'objection'")
                .bind(id)
                .execute(&st.db)
                .await?;
            let t = load(st, id).await?;
            ask_recipient(st, &t).await;
        }
    }
    let expired = sqlx::query_as::<_, Row>(&format!(
        "{SELECT} WHERE t.status IN ('awaiting_owner', 'awaiting_recipient') AND t.deadline <= now()"
    ))
    .fetch_all(&st.db)
    .await?;
    for t in expired {
        if finish(st, &t, "expired", None, "Nobody answered in time")
            .await
            .is_ok()
        {
            tell(
                st,
                t.from_user,
                &format!("The transfer of {} ran out of time", t.guild_name),
                vec![format!(
                    "The transfer of {} to @{} wasn't accepted in time, so nothing changed.",
                    t.guild_name, t.to_username
                )],
                None,
            )
            .await;
        }
    }
    Ok(done)
}

pub fn spawn_sweeper(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            tick.tick().await;
            if let Err(e) = sweep(&st).await {
                tracing::warn!(error = %e, "ownership transfer sweep failed");
            }
        }
    });
}

/// `POST /api/transfers/sweep`: runs the sweep now. Only with
/// `OUTHOUSE_TEST_HOOKS=1` (the tests move the clock in SQL).
pub async fn sweep_now(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    if std::env::var("OUTHOUSE_TEST_HOOKS").ok().as_deref() != Some("1") {
        return Err(AppError::NotFound);
    }
    super::safety::require_owner(&st, &user).await?;
    Ok(Json(json!({ "completed": sweep(&st).await? })))
}
