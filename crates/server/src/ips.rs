//! IP addresses, kept for safety.
//!
//! Why: police (the AFP's ACCCE, the eSafety Commissioner) ask for addresses
//! and times in child-safety cases, and a suspended or banned person coming
//! back on a new account often comes from the same address.
//!
//! What: one row per (account, address) in `user_ips`, updated in place, and
//! `users.last_ip`. Recorded when an account is made, at every sign-in, and
//! at most once an hour per session for signed-in activity (API calls and
//! the gateway). Bots are skipped. The address is the one
//! `security::client_ip` works out (the same one the rate limits use).
//!
//! Who sees them: only the site owner, in the safety console (each look is
//! written to the safety log). Nobody else, not even the person themselves.
//!
//! How long: `IP_RETENTION_DAYS` (default 90) after an address was last
//! seen. A report's copy (`report_ips`, made when the report is made, when
//! its evidence is exported and just before retention would delete them)
//! stays while the report is open, and for the retention period after it
//! closes (child-safety evidence: until the owner deletes the evidence).
//!
//! Shared addresses are common (households, mobile networks, VPNs), so a
//! match is only ever a quiet hint in the owner's console: nothing is
//! blocked or banned automatically.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::error::AppResult;
use crate::security;
use crate::state::AppState;

pub const DEFAULT_RETENTION_DAYS: i64 = 90;
/// Signed-in activity is noted at most this often per session and address.
const SESSION_EVERY: Duration = Duration::from_secs(3600);
/// Ban-evasion hints raised for one sign-in at most.
const MAX_MATCHES_PER_SIGNIN: i64 = 20;

/// Days an address is kept after it was last seen (`IP_RETENTION_DAYS`).
pub fn retention_days() -> i64 {
    static DAYS: OnceLock<i64> = OnceLock::new();
    *DAYS.get_or_init(|| parse_retention(std::env::var("IP_RETENTION_DAYS").ok().as_deref()))
}

fn parse_retention(value: Option<&str>) -> i64 {
    value
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| (1..=3650).contains(d))
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// How an address was first seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Register,
    Login,
    Session,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Register => "register",
            Kind::Login => "login",
            Kind::Session => "session",
        }
    }
}

/// Notes that `user_id` used `ip` (bots and unknown addresses are skipped).
pub async fn record(db: &PgPool, user_id: i64, ip: IpAddr, kind: Kind) -> AppResult<()> {
    if ip.is_unspecified() {
        return Ok(());
    }
    sqlx::query(
        "WITH u AS (
             UPDATE users SET last_ip = $2::INET, last_ip_at = now()
             WHERE id = $1 AND NOT is_bot RETURNING id)
         INSERT INTO user_ips (user_id, ip, kind)
         SELECT id, $2::INET, $3 FROM u
         ON CONFLICT (user_id, ip) DO UPDATE
             SET last_seen = now(), count = user_ips.count + 1",
    )
    .bind(user_id)
    .bind(ip.to_string())
    .bind(kind.as_str())
    .execute(db)
    .await?;
    Ok(())
}

/// Sessions (and addresses) noted in the last hour.
fn recent_sessions() -> &'static Mutex<HashMap<String, Instant>> {
    static SEEN: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_key(token: &str, ip: IpAddr) -> String {
    let hash = crate::auth::token_hash(token);
    format!("{}|{ip}", &hash[..16])
}

/// True the first time in an hour this session is seen from this address
/// (and remembers it).
fn session_due(token: &str, ip: IpAddr) -> bool {
    let key = session_key(token, ip);
    let mut seen = recent_sessions().lock().expect("ip session lock");
    if seen.len() > 100_000 {
        seen.retain(|_, at| at.elapsed() < SESSION_EVERY);
    }
    match seen.get(&key) {
        Some(at) if at.elapsed() < SESSION_EVERY => false,
        _ => {
            seen.insert(key, Instant::now());
            true
        }
    }
}

/// A new session from sign-in or sign-up: its address is already recorded,
/// so its first requests don't count it again.
pub fn session_started(token: &str, ip: IpAddr) {
    let _ = session_due(token, ip);
}

/// Signed-in activity (an API call or the gateway): noted at most once an
/// hour per session and address, off the request's path.
pub fn note_session(st: &AppState, token: &str, user_id: i64, bot: bool, ip: IpAddr) {
    if bot || ip.is_unspecified() || token.is_empty() || !session_due(token, ip) {
        return;
    }
    let db = st.db.clone();
    tokio::spawn(async move {
        if let Err(e) = record(&db, user_id, ip, Kind::Session).await {
            tracing::warn!(error = %e, "couldn't note a session's address");
        }
    });
}

/// Sign-up and sign-in: record the address, then look for a currently
/// suspended account, or one banned from any outhouse, that used it within
/// the retention period. Each new pair becomes one quiet notice in the
/// owner's console (never repeated, never a push, nothing blocked).
pub async fn signed_in(st: &AppState, user_id: i64, token: &str, ip: IpAddr, kind: Kind) {
    if let Err(e) = record(&st.db, user_id, ip, kind).await {
        tracing::warn!(error = %e, "couldn't record a sign-in address");
        return;
    }
    session_started(token, ip);
    if let Err(e) = check_matches(st, user_id, ip, kind).await {
        tracing::warn!(error = %e, "couldn't check for ban-evasion hints");
    }
}

/// A suspended or banned account seen on the same address.
#[derive(sqlx::FromRow)]
pub struct MatchRow {
    pub id: i64,
    pub last_seen: DateTime<Utc>,
    pub suspended: bool,
    pub suspended_until: Option<DateTime<Utc>>,
    pub banned_in: Vec<String>,
}

/// Raises ban-evasion hints for `user_id` on `ip`. Returns how many.
pub async fn check_matches(
    st: &AppState,
    user_id: i64,
    ip: IpAddr,
    kind: Kind,
) -> AppResult<usize> {
    // Everyone at home (and everything behind Apache) shares these.
    if ip.is_unspecified() || security::is_local_or_proxy(ip, &security::get().trusted_proxies) {
        return Ok(0);
    }
    let rows = sqlx::query_as::<_, MatchRow>(
        "SELECT u.id, o.last_seen,
                COALESCE(u.suspended_until > now(), false) AS suspended,
                CASE WHEN u.suspended_until > now() THEN u.suspended_until END AS suspended_until,
                ARRAY(SELECT g.name FROM bans b JOIN guilds g ON g.id = b.guild_id
                      WHERE b.user_id = u.id ORDER BY lower(g.name)) AS banned_in
         FROM user_ips o JOIN users u ON u.id = o.user_id
         WHERE o.ip = $2::INET AND o.user_id <> $1 AND NOT u.is_bot
           AND o.last_seen > now() - make_interval(days => $3)
           AND (u.suspended_until > now() OR EXISTS(SELECT 1 FROM bans b WHERE b.user_id = u.id))
           AND NOT EXISTS(SELECT 1 FROM ip_matches m WHERE m.user_id = $1 AND m.matched_user_id = u.id)
         ORDER BY o.last_seen DESC
         LIMIT $4",
    )
    .bind(user_id)
    .bind(ip.to_string())
    .bind(retention_days() as i32)
    .bind(MAX_MATCHES_PER_SIGNIN)
    .fetch_all(&st.db)
    .await?;
    let mut raised = 0;
    for m in rows {
        let fresh: Option<i64> = sqlx::query_scalar(
            "INSERT INTO ip_matches (user_id, matched_user_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING RETURNING matched_user_id",
        )
        .bind(user_id)
        .bind(m.id)
        .fetch_optional(&st.db)
        .await?;
        if fresh.is_none() {
            continue;
        }
        let report = crate::api::safety::ip_match_report(st, user_id, &m, ip, kind).await?;
        sqlx::query(
            "UPDATE ip_matches SET report_id = $3 WHERE user_id = $1 AND matched_user_id = $2",
        )
        .bind(user_id)
        .bind(m.id)
        .bind(report)
        .execute(&st.db)
        .await?;
        raised += 1;
    }
    Ok(raised)
}

/// Copies a person's current addresses into a report (new rows are added,
/// existing ones brought up to date).
pub async fn snapshot_for_report(db: &PgPool, report_id: i64, user_id: i64) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO report_ips (report_id, user_id, ip, kind, first_seen, last_seen, count)
         SELECT $1, user_id, ip, kind, first_seen, last_seen, count FROM user_ips WHERE user_id = $2
         ON CONFLICT (report_id, user_id, ip) DO UPDATE
             SET first_seen = LEAST(report_ips.first_seen, EXCLUDED.first_seen),
                 last_seen = GREATEST(report_ips.last_seen, EXCLUDED.last_seen),
                 count = GREATEST(report_ips.count, EXCLUDED.count)",
    )
    .bind(report_id)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

/// What one retention run removed.
#[derive(Debug, Default, serde::Serialize)]
pub struct Purged {
    pub kept_for_reports: u64,
    pub addresses: u64,
    pub report_addresses: u64,
}

/// Deletes addresses not seen for `days`. Ones belonging to the subject of
/// an open report (not an IP-match notice) are copied into the report first.
pub async fn purge(db: &PgPool, days: i64) -> AppResult<Purged> {
    let days = days.max(0) as i32;
    let mut tx = db.begin().await?;
    let kept = sqlx::query(
        "INSERT INTO report_ips (report_id, user_id, ip, kind, first_seen, last_seen, count)
         SELECT r.id, i.user_id, i.ip, i.kind, i.first_seen, i.last_seen, i.count
         FROM user_ips i JOIN reports r ON r.reported_user_id = i.user_id
         WHERE r.status <> 'closed' AND r.source <> 'ip-match'
           AND i.last_seen < now() - make_interval(days => $1)
         ON CONFLICT (report_id, user_id, ip) DO UPDATE
             SET first_seen = LEAST(report_ips.first_seen, EXCLUDED.first_seen),
                 last_seen = GREATEST(report_ips.last_seen, EXCLUDED.last_seen),
                 count = GREATEST(report_ips.count, EXCLUDED.count)",
    )
    .bind(days)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let addresses =
        sqlx::query("DELETE FROM user_ips WHERE last_seen < now() - make_interval(days => $1)")
            .bind(days)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    sqlx::query(
        "UPDATE users SET last_ip = NULL, last_ip_at = NULL
         WHERE last_ip_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(&mut *tx)
    .await?;
    // A report's copy: IP-match notices follow the normal retention; other
    // reports keep theirs while open and for `days` after closing, and
    // child-safety evidence until the owner deletes it.
    let report_addresses = sqlx::query(
        "DELETE FROM report_ips ri USING reports r
         WHERE r.id = ri.report_id
           AND ((r.source = 'ip-match' AND ri.last_seen < now() - make_interval(days => $1))
             OR (r.source <> 'ip-match' AND r.status = 'closed'
                 AND r.closed_at < now() - make_interval(days => $1)
                 AND NOT (r.category = 'child_safety' AND NOT r.evidence_released)))",
    )
    .bind(days)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(Purged {
        kept_for_reports: kept,
        addresses,
        report_addresses,
    })
}

/// Runs the retention every hour (and once at start-up).
pub fn spawn_pruner(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            match purge(&st.db, retention_days()).await {
                Ok(p) if p.addresses + p.report_addresses > 0 => tracing::info!(
                    removed = p.addresses,
                    report_copies_removed = p.report_addresses,
                    kept_for_reports = p.kept_for_reports,
                    "IP retention: removed old addresses"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "IP retention run failed"),
            }
        }
    });
}

/// account id, username, ip, kind, count, first seen (UTC, Sydney), last seen (UTC, Sydney)
type ExportRow = (
    String,
    Option<String>,
    String,
    String,
    i32,
    String,
    String,
    String,
    String,
);

/// A report's addresses for the evidence package: every time in UTC and in
/// Sydney time (with the AEST/AEDT label and offset), clearly labelled.
pub async fn report_ips_for_export(db: &PgPool, report_id: i64) -> AppResult<Vec<Value>> {
    let mut tx = db.begin().await?;
    // Only for this transaction: Postgres does the daylight-saving sums.
    sqlx::query("SET LOCAL TIME ZONE 'Australia/Sydney'")
        .execute(&mut *tx)
        .await?;
    let rows: Vec<ExportRow> = sqlx::query_as(
        "SELECT ri.user_id::TEXT, u.username, host(ri.ip), ri.kind, ri.count,
                to_char(ri.first_seen AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'),
                to_char(ri.first_seen, 'YYYY-MM-DD HH24:MI:SS TZ \"(UTC\"OF\")\"'),
                to_char(ri.last_seen AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'),
                to_char(ri.last_seen, 'YYYY-MM-DD HH24:MI:SS TZ \"(UTC\"OF\")\"')
         FROM report_ips ri LEFT JOIN users u ON u.id = ri.user_id WHERE ri.report_id = $1
         ORDER BY ri.user_id, ri.last_seen DESC",
    )
    .bind(report_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(
            |(user, username, ip, kind, count, f_utc, f_syd, l_utc, l_syd)| {
                json!({
                    "account_id": user,
                    "account_username": username,
                    "ip_address": ip,
                    "first_seen_as": kind,
                    "times_seen": count,
                    "first_seen_utc": f_utc,
                    "first_seen_australia_sydney": f_syd,
                    "last_seen_utc": l_utc,
                    "last_seen_australia_sydney": l_syd,
                })
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_days_from_the_environment() {
        assert_eq!(parse_retention(None), 90);
        assert_eq!(parse_retention(Some("30")), 30);
        assert_eq!(parse_retention(Some(" 7 ")), 7);
        assert_eq!(parse_retention(Some("0")), 90);
        assert_eq!(parse_retention(Some("-5")), 90);
        assert_eq!(parse_retention(Some("forever")), 90);
        assert_eq!(parse_retention(Some("99999")), 90);
    }

    #[test]
    fn sessions_are_noted_once_an_hour_per_address() {
        let a: IpAddr = "203.0.113.10".parse().unwrap();
        let b: IpAddr = "203.0.113.11".parse().unwrap();
        let token = "unit-test-token-ips";
        assert!(session_due(token, a));
        assert!(!session_due(token, a));
        // A new address (a phone moving networks) is noted straight away.
        assert!(session_due(token, b));
        assert!(!session_due(token, b));
        // Sign-in already recorded it.
        session_started("unit-test-token-2", a);
        assert!(!session_due("unit-test-token-2", a));
        assert!(session_due("unit-test-token-3", a));
    }

    #[test]
    fn kinds() {
        assert_eq!(Kind::Register.as_str(), "register");
        assert_eq!(Kind::Login.as_str(), "login");
        assert_eq!(Kind::Session.as_str(), "session");
    }
}
