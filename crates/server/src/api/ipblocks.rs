//! Blocked networks in the safety console (site owner only). See
//! `crate::ipblock` for what a block does.
//!
//! A block is made from a permanently suspended person's IP addresses (their
//! recent `user_ips`, and their reports' copies in `report_ips` within the
//! retention period). For a person with a child-safety report the console
//! ticks "Also block their IP addresses" by default.

use std::collections::BTreeMap;
use std::net::IpAddr;

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::audit::is_site_owner;
use super::safety::{log_action, require_owner};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::ipblock;
use crate::security::{Cidr, ClientIp};
use crate::state::{AppState, Audience};

/// The one-line explanation shown with blocks.
pub const BLOCK_NOTE: &str = "Everyone on a blocked network is locked out: their household, and possibly a whole mobile network, school or workplace that shares the address. That's your choice under your safety policy. Changing networks or using a VPN can still get around it. Your own account always works, and you can allow someone you know is innocent (People → Allow from blocked networks).";

/// One network a person used, and whether it can be blocked.
struct Candidate {
    network: String,
    ips: Vec<String>,
    last_seen: DateTime<Utc>,
    skip: Option<String>,
    own: bool,
    blocked: bool,
}

async fn candidates(st: &AppState, user_id: i64, owner_ip: IpAddr) -> AppResult<Vec<Candidate>> {
    let rows: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT host(ip), max(last_seen) FROM (
             SELECT ip, last_seen FROM user_ips WHERE user_id = $1
             UNION ALL
             SELECT ip, last_seen FROM report_ips
             WHERE user_id = $1 AND last_seen > now() - make_interval(days => $2)
         ) x GROUP BY host(ip) ORDER BY 2 DESC LIMIT 200",
    )
    .bind(user_id)
    .bind(crate::ips::retention_days() as i32)
    .fetch_all(&st.db)
    .await?;
    // The owners' own addresses (now and recently): blocking them would
    // lock out their household.
    let owners = super::safety::site_owner_ids(&st.db).await?;
    let owner_ips: Vec<String> =
        sqlx::query_scalar("SELECT host(ip) FROM user_ips WHERE user_id = ANY($1)")
            .bind(&owners)
            .fetch_all(&st.db)
            .await?;
    let mut owner_addrs: Vec<IpAddr> = owner_ips.iter().filter_map(|i| i.parse().ok()).collect();
    owner_addrs.push(owner_ip);
    let active: Vec<String> = sqlx::query_scalar(
        "SELECT ip::TEXT FROM ip_blocks WHERE removed_at IS NULL
           AND (expires_at IS NULL OR expires_at > now())",
    )
    .fetch_all(&st.db)
    .await?;
    let mut by_net: BTreeMap<String, Candidate> = BTreeMap::new();
    for (ip, seen) in rows {
        let Ok(addr) = ip.parse::<IpAddr>() else {
            continue;
        };
        let network = ipblock::network_for(addr);
        let entry = by_net.entry(network.clone()).or_insert_with(|| {
            let net = Cidr::parse(&network);
            let skip = ipblock::unblockable_reason(addr).map(str::to_string);
            let own = skip.is_none()
                && net
                    .as_ref()
                    .is_some_and(|n| owner_addrs.iter().any(|o| n.contains(*o)));
            Candidate {
                network: network.clone(),
                ips: Vec::new(),
                last_seen: seen,
                skip,
                own,
                blocked: active.contains(&network),
            }
        });
        entry.ips.push(ip);
        entry.last_seen = entry.last_seen.max(seen);
    }
    let mut list: Vec<Candidate> = by_net.into_values().collect();
    list.sort_by_key(|c| std::cmp::Reverse(c.last_seen));
    Ok(list)
}

/// Has this person a child-safety report that wasn't dismissed as a mistake?
async fn has_child_safety_report(db: &PgPool, user_id: i64) -> AppResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM reports
         WHERE reported_user_id = $1 AND category = 'child_safety' AND source <> 'ip-match'
           AND NOT (status = 'closed' AND COALESCE(resolution, '') LIKE 'Dismissed%')
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?)
}

/// Is this account suspended for good (not "until reviewed")?
async fn permanently_suspended(db: &PgPool, user_id: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1
                       AND suspended_until >= '9999-01-01' AND NOT suspension_pending)",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

/// `GET /api/safety/users/{id}/ip-block-preview`: the networks a block
/// would cover (each look is written to the safety log, like IP addresses).
pub async fn preview(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    ClientIp(my_ip): ClientIp,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let list = candidates(&st, id, my_ip).await?;
    let child = has_child_safety_report(&st.db, id).await?;
    log_action(
        &st,
        Some(user.id),
        "view_ips",
        None,
        Some(id),
        format!("Looked at @{username}'s networks to block"),
    )
    .await;
    Ok(Json(json!({
        "user_id": id.to_string(),
        "child_safety": child.is_some(),
        "child_safety_report_id": child.map(|r| r.to_string()),
        "permanent": permanently_suspended(&st.db, id).await?,
        "site_owner": is_site_owner(&st.db, id).await?,
        "note": BLOCK_NOTE,
        "networks": list.iter().map(|c| json!({
            "network": c.network,
            "ips": c.ips,
            "last_seen": c.last_seen,
            "skip": c.skip,
            "own": c.own,
            "already_blocked": c.blocked,
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct NewBlocks {
    /// Networks to block (from the preview); none = every one that can be.
    #[serde(default)]
    networks: Option<Vec<String>>,
    /// Days until it ends; none = until the owner removes it.
    #[serde(default)]
    days: Option<i64>,
    #[serde(default)]
    reason: Option<String>,
    /// Block networks the owner uses too (he confirmed it).
    #[serde(default)]
    include_own: bool,
    #[serde(default, with = "outhouse_common::id::option_as_string")]
    report_id: Option<i64>,
}

/// `POST /api/safety/users/{id}/ip-blocks`: blocks a permanently suspended
/// person's networks, and signs out every account seen on them (except the
/// site owner and accounts he allowed).
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    ClientIp(my_ip): ClientIp,
    Path(id): Path<i64>,
    Json(req): Json<NewBlocks>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    if is_site_owner(&st.db, id).await? {
        return Err(AppError::BadRequest(
            "A site owner's networks can't be blocked.".into(),
        ));
    }
    if !permanently_suspended(&st.db, id).await? {
        return Err(AppError::BadRequest(
            "Make their suspension permanent first.".into(),
        ));
    }
    let days = req.days.filter(|d| *d > 0).map(|d| d.min(3650) as i32);
    let reason: String = req
        .reason
        .unwrap_or_default()
        .trim()
        .chars()
        .take(300)
        .collect();
    let report = match req.report_id {
        Some(r) => Some(r),
        None => has_child_safety_report(&st.db, id).await?,
    };
    let list = candidates(&st, id, my_ip).await?;
    let mut made = Vec::new();
    let mut skipped = Vec::new();
    for c in &list {
        if let Some(wanted) = &req.networks {
            if !wanted.iter().any(|w| w.trim() == c.network) {
                continue;
            }
        }
        if let Some(why) = &c.skip {
            skipped.push(format!("{} ({why})", c.network));
            continue;
        }
        if c.own && !req.include_own {
            skipped.push(format!("{} (one of your own addresses)", c.network));
            continue;
        }
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO ip_blocks (id, ip, source_user_id, report_id, reason, expires_at, created_by)
             VALUES ($1, $2::CIDR, $3, $4, $5,
                     CASE WHEN $6::INT IS NULL THEN NULL ELSE now() + make_interval(days => $6::INT) END, $7)
             ON CONFLICT (ip) WHERE removed_at IS NULL DO NOTHING
             RETURNING id",
        )
        .bind(st.ids.next_id())
        .bind(&c.network)
        .bind(id)
        .bind(report)
        .bind(&reason)
        .bind(days)
        .bind(user.id)
        .fetch_optional(&st.db)
        .await?;
        if inserted.is_some() {
            made.push(c.network.clone());
        } else {
            skipped.push(format!("{} (already blocked)", c.network));
        }
    }
    ipblock::reload(&st.db).await?;
    let signed_out = sign_out_blocked(&st, &made).await?;
    if !made.is_empty() {
        log_action(
            &st,
            Some(user.id),
            "ip_block",
            report,
            Some(id),
            format!(
                "Blocked {} network(s) {}: {}. {} account(s) seen on them signed out.{}{}",
                made.len(),
                match days {
                    Some(d) => format!("for {d} days"),
                    None => "until removed".into(),
                },
                made.join(", "),
                signed_out,
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" Reason: {reason}")
                },
                if skipped.is_empty() {
                    String::new()
                } else {
                    format!(" Skipped: {}", skipped.join("; "))
                }
            ),
        )
        .await;
    }
    Ok(Json(json!({
        "blocked": made,
        "skipped": skipped,
        "signed_out": signed_out,
    })))
}

/// Ends every session of accounts seen on these networks (the site owner and
/// allowed accounts excepted) and closes their connections.
async fn sign_out_blocked(st: &AppState, networks: &[String]) -> AppResult<usize> {
    if networks.is_empty() {
        return Ok(0);
    }
    let users: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT u.id FROM user_ips i JOIN users u ON u.id = i.user_id
         WHERE i.ip <<= ANY($1::CIDR[]) AND NOT u.ip_block_exempt AND NOT u.is_bot",
    )
    .bind(networks)
    .fetch_all(&st.db)
    .await?;
    let mut n = 0;
    for uid in users {
        if is_site_owner(&st.db, uid).await? {
            continue;
        }
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(uid)
            .execute(&st.db)
            .await?;
        crate::api::voice::clear_voice(st, uid, None);
        st.dispatch(
            Audience::User(uid),
            ServerEvent::InvalidSession {
                reason: ipblock::USE_REFUSED.into(),
            },
        );
        n += 1;
    }
    Ok(n)
}

#[derive(sqlx::FromRow)]
struct BlockRow {
    id: i64,
    network: String,
    source_user_id: Option<i64>,
    username: Option<String>,
    display_name: Option<String>,
    report_id: Option<i64>,
    reason: String,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    created_by_name: Option<String>,
    attempts: i64,
    last_attempt: Option<DateTime<Utc>>,
    active: bool,
}

#[derive(sqlx::FromRow)]
struct AttemptRow {
    id: i64,
    block_id: i64,
    network: String,
    ip: String,
    kind: String,
    user_id: Option<i64>,
    username: String,
    account_name: Option<String>,
    created_at: DateTime<Utc>,
}

const ATTEMPT_SELECT: &str = "SELECT a.id, a.block_id, b.ip::TEXT AS network, host(a.ip) AS ip,
        a.kind, a.user_id, a.username, u.display_name AS account_name, a.created_at
    FROM blocked_attempts a JOIN ip_blocks b ON b.id = a.block_id
    LEFT JOIN users u ON u.id = a.user_id";

fn attempt_json(a: &AttemptRow) -> Value {
    json!({
        "id": a.id.to_string(),
        "block_id": a.block_id.to_string(),
        "network": a.network,
        "ip": a.ip,
        "kind": a.kind,
        "user_id": a.user_id.map(|u| u.to_string()),
        "username": a.username,
        "account_name": a.account_name,
        "created_at": a.created_at,
    })
}

/// Refused attempts that concern this person: theirs, and any on networks
/// blocked because of them.
pub async fn attempts_for_user(db: &PgPool, user_id: i64) -> AppResult<Vec<Value>> {
    let sql = format!(
        "{ATTEMPT_SELECT} WHERE a.user_id = $1 OR b.source_user_id = $1
         ORDER BY a.created_at DESC LIMIT 100"
    );
    let rows = sqlx::query_as::<_, AttemptRow>(&sql)
        .bind(user_id)
        .fetch_all(db)
        .await?;
    Ok(rows.iter().map(attempt_json).collect())
}

/// `GET /api/safety/ip-blocks`: blocks (active and expired, not removed)
/// and the latest refused attempts.
pub async fn list(State(st): State<AppState>, AuthUser(user): AuthUser) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let blocks = sqlx::query_as::<_, BlockRow>(
        "SELECT b.id, b.ip::TEXT AS network, b.source_user_id, u.username, u.display_name,
                b.report_id, b.reason, b.created_at, b.expires_at, c.display_name AS created_by_name,
                (SELECT count(*) FROM blocked_attempts a WHERE a.block_id = b.id) AS attempts,
                (SELECT max(created_at) FROM blocked_attempts a WHERE a.block_id = b.id) AS last_attempt,
                (b.expires_at IS NULL OR b.expires_at > now()) AS active
         FROM ip_blocks b
         LEFT JOIN users u ON u.id = b.source_user_id
         LEFT JOIN users c ON c.id = b.created_by
         WHERE b.removed_at IS NULL
         ORDER BY active DESC, b.created_at DESC LIMIT 500",
    )
    .fetch_all(&st.db)
    .await?;
    let sql = format!("{ATTEMPT_SELECT} ORDER BY a.created_at DESC LIMIT 100");
    let attempts = sqlx::query_as::<_, AttemptRow>(&sql)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(json!({
        "note": BLOCK_NOTE,
        "blocks": blocks.iter().map(|b| json!({
            "id": b.id.to_string(),
            "network": b.network,
            "account": b.source_user_id.map(|id| json!({
                "id": id.to_string(), "username": b.username, "display_name": b.display_name,
            })),
            "report_id": b.report_id.map(|r| r.to_string()),
            "reason": b.reason,
            "created_at": b.created_at,
            "created_by_name": b.created_by_name,
            "expires_at": b.expires_at,
            "active": b.active,
            "attempts": b.attempts,
            "last_attempt": b.last_attempt,
        })).collect::<Vec<_>>(),
        "attempts": attempts.iter().map(attempt_json).collect::<Vec<_>>(),
    })))
}

async fn block_label(db: &PgPool, id: i64) -> AppResult<(String, Option<i64>, Option<i64>)> {
    sqlx::query_as(
        "SELECT ip::TEXT, source_user_id, report_id FROM ip_blocks WHERE id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)
}

/// `POST /api/safety/ip-blocks/{id}/remove`
pub async fn remove(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let (network, source, report) = block_label(&st.db, id).await?;
    sqlx::query("UPDATE ip_blocks SET removed_at = now(), removed_by = $2 WHERE id = $1")
        .bind(id)
        .bind(user.id)
        .execute(&st.db)
        .await?;
    ipblock::reload(&st.db).await?;
    log_action(
        &st,
        Some(user.id),
        "ip_unblock",
        report,
        source,
        format!("Removed the block on {network} (its record and attempts are kept)"),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct Extend {
    /// Days from now; none = until removed.
    #[serde(default)]
    days: Option<i64>,
}

/// `POST /api/safety/ip-blocks/{id}/extend` `{days}` (none = until removed)
pub async fn extend(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<Extend>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let (network, source, report) = block_label(&st.db, id).await?;
    let days = req.days.filter(|d| *d > 0).map(|d| d.min(3650) as i32);
    sqlx::query(
        "UPDATE ip_blocks SET expires_at = CASE WHEN $2::INT IS NULL THEN NULL
                                                ELSE now() + make_interval(days => $2::INT) END
         WHERE id = $1",
    )
    .bind(id)
    .bind(days)
    .execute(&st.db)
    .await?;
    ipblock::reload(&st.db).await?;
    let signed_out = sign_out_blocked(&st, std::slice::from_ref(&network)).await?;
    log_action(
        &st,
        Some(user.id),
        "ip_block_extend",
        report,
        source,
        match days {
            Some(d) => format!("Block on {network} now ends in {d} days"),
            None => format!("Block on {network} now lasts until removed"),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true, "signed_out": signed_out })))
}

/// `POST /api/safety/ip-blocks/reload`: re-reads the blocks now. Only with
/// `OUTHOUSE_TEST_HOOKS=1` (the smoke test changes expiries in SQL).
pub async fn reload_now(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    if std::env::var("OUTHOUSE_TEST_HOOKS").ok().as_deref() != Some("1") {
        return Err(AppError::NotFound);
    }
    require_owner(&st, &user).await?;
    let n = ipblock::reload(&st.db).await?;
    Ok(Json(json!({ "active": n })))
}

/// For the evidence package: the blocks made because of this person (or
/// this report) and every refused attempt on them, in UTC and Sydney time.
pub async fn evidence(db: &PgPool, report_id: i64, user_id: Option<i64>) -> AppResult<Value> {
    let mut tx = db.begin().await?;
    sqlx::query("SET LOCAL TIME ZONE 'Australia/Sydney'")
        .execute(&mut *tx)
        .await?;
    let utc = "'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'";
    let syd = "'YYYY-MM-DD HH24:MI:SS TZ \"(UTC\"OF\")\"'";
    #[allow(clippy::type_complexity)]
    let blocks: Vec<(String, String, String, Option<String>, Option<String>, String)> =
        sqlx::query_as(&format!(
            "SELECT ip::TEXT, to_char(created_at AT TIME ZONE 'UTC', {utc}), to_char(created_at, {syd}),
                    to_char(expires_at AT TIME ZONE 'UTC', {utc}),
                    to_char(removed_at AT TIME ZONE 'UTC', {utc}), reason
             FROM ip_blocks WHERE report_id = $1 OR source_user_id = $2 ORDER BY created_at"
        ))
        .bind(report_id)
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
    #[allow(clippy::type_complexity)]
    let attempts: Vec<(
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        String,
    )> = sqlx::query_as(&format!(
        "SELECT to_char(a.created_at AT TIME ZONE 'UTC', {utc}), to_char(a.created_at, {syd}),
                    host(a.ip), a.kind, a.user_id::TEXT, a.username, b.ip::TEXT
             FROM blocked_attempts a JOIN ip_blocks b ON b.id = a.block_id
             WHERE b.report_id = $1 OR b.source_user_id = $2
             ORDER BY a.created_at"
    ))
    .bind(report_id)
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(json!({
        "note": "Networks the site owner blocked after permanently suspending this account (IPv4: that address; IPv6: its /64), and every attempt refused from them: signing up, signing in, or using Outhouse (requests and live connections are noted at most once every 10 minutes per account). Shared addresses are common, so an attempt isn't proof of who made it. Times are UTC and Sydney time (Australia/Sydney).",
        "blocks": blocks.iter().map(|(net, c_utc, c_syd, exp, removed, reason)| json!({
            "network": net,
            "created_utc": c_utc,
            "created_australia_sydney": c_syd,
            "expires_utc": exp,
            "removed_utc": removed,
            "reason": reason,
        })).collect::<Vec<_>>(),
        "refused_attempts": attempts.iter().map(|(t_utc, t_syd, ip, kind, uid, username, net)| json!({
            "time_utc": t_utc,
            "time_australia_sydney": t_syd,
            "ip_address": ip,
            "blocked_network": net,
            "attempt": kind,
            "account_id": uid,
            "username_tried": username,
        })).collect::<Vec<_>>(),
    }))
}
