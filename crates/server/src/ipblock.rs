//! Blocked networks: IP addresses the site owner blocked when he suspended
//! someone permanently (see `api::ipblocks` for the console side).
//!
//! Strict, by the owner's choice: while a block is active nobody can sign
//! up, sign in, use the API or connect to the gateway from it. Existing
//! sessions of accounts seen on it are signed out when it's made. The only
//! ways past it are the site owner's own account (so he can't lock himself
//! out) and accounts he marks "Allow from blocked networks".
//!
//! IPv4 blocks are one address (/32); IPv6 blocks are the address's /64
//! (one household or phone usually has a whole /64). Private, LAN,
//! loopback, link-local, CGNAT-internal (100.64.0.0/10) and trusted-proxy
//! addresses are never blocked.
//!
//! Every refused attempt is written to `blocked_attempts` (time, address,
//! account or username tried); they go in the blocked person's evidence
//! package.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::AppResult;
use crate::security::{self, Cidr};
use crate::state::AppState;

/// What someone on a blocked network is told when signing up.
pub const SIGNUP_REFUSED: &str =
    "New accounts can't be created from your network right now. If you think this is a mistake, contact the server owner.";
/// ...and when signing in or using Outhouse.
pub const USE_REFUSED: &str =
    "Outhouse can't be used from your network right now. If you think this is a mistake, contact the server owner.";

/// A refused request or gateway connection is written down at most this
/// often per account and address (sign-ups and sign-ins: every time).
const ATTEMPT_EVERY: Duration = Duration::from_secs(600);

#[derive(Clone)]
struct Active {
    id: i64,
    net: Cidr,
    expires: Option<DateTime<Utc>>,
}

fn active() -> &'static RwLock<Vec<Active>> {
    static BLOCKS: OnceLock<RwLock<Vec<Active>>> = OnceLock::new();
    BLOCKS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Reads the active blocks into memory (after every change, and every
/// minute so expiries and other processes' changes are picked up).
pub async fn reload(db: &PgPool) -> AppResult<usize> {
    let rows: Vec<(i64, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, ip::TEXT, expires_at FROM ip_blocks
         WHERE removed_at IS NULL AND (expires_at IS NULL OR expires_at > now())",
    )
    .fetch_all(db)
    .await?;
    let list: Vec<Active> = rows
        .into_iter()
        .filter_map(|(id, net, expires)| {
            Some(Active {
                id,
                net: Cidr::parse(&net)?,
                expires,
            })
        })
        .collect();
    let n = list.len();
    *active().write().expect("ip block lock") = list;
    Ok(n)
}

pub fn spawn_refresher(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = reload(&st.db).await {
                tracing::warn!(error = %e, "couldn't load the blocked networks");
            }
        }
    });
}

/// The active block covering `ip`, if any.
pub fn blocked(ip: IpAddr) -> Option<i64> {
    if ip.is_unspecified() {
        return None;
    }
    let now = Utc::now();
    active()
        .read()
        .expect("ip block lock")
        .iter()
        .find(|b| b.expires.is_none_or(|e| e > now) && b.net.contains(ip))
        .map(|b| b.id)
}

/// Why an address can't be blocked (None = it can). Shared and internal
/// ranges would lock out everyone at home or behind the proxy.
pub fn unblockable_reason(ip: IpAddr) -> Option<&'static str> {
    let ip = security::canonical(ip);
    if security::is_local_or_proxy(ip, &security::get().trusted_proxies) {
        return Some("a private, LAN or proxy address (never blocked)");
    }
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if o[0] == 100 && (64..128).contains(&o[1]) {
                return Some("a carrier-grade NAT address (100.64.0.0/10, never blocked)");
            }
            if v4.is_multicast() || o[0] == 0 {
                return Some("not a real visitor address");
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_multicast() {
                return Some("not a real visitor address");
            }
        }
    }
    None
}

/// The network a block covers: IPv4 the address (/32), IPv6 its /64.
pub fn network_for(ip: IpAddr) -> String {
    match security::canonical(ip) {
        IpAddr::V4(v4) => format!("{v4}/32"),
        IpAddr::V6(v6) => {
            let net = u128::from(v6) & (u128::MAX << 64);
            format!("{}/64", std::net::Ipv6Addr::from(net))
        }
    }
}

/// May this account get past a block? (The site owner and accounts he
/// exempted.)
pub async fn exempt(db: &PgPool, user_id: i64) -> bool {
    let flagged: bool =
        sqlx::query_scalar("SELECT COALESCE(ip_block_exempt, false) FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);
    flagged
        || crate::api::audit::is_site_owner(db, user_id)
            .await
            .unwrap_or(false)
}

fn attempt_due(key: String) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("attempt lock");
    if seen.len() > 50_000 {
        seen.retain(|_, at| at.elapsed() < ATTEMPT_EVERY);
    }
    match seen.get(&key) {
        Some(at) if at.elapsed() < ATTEMPT_EVERY => false,
        _ => {
            seen.insert(key, Instant::now());
            true
        }
    }
}

/// Writes down a refused attempt (requests and gateway connections at most
/// once every 10 minutes per account and address).
pub async fn record_attempt(
    st: &AppState,
    block_id: i64,
    ip: IpAddr,
    kind: &str,
    user_id: Option<i64>,
    username: &str,
) {
    if matches!(kind, "request" | "gateway")
        && !attempt_due(format!("{kind}|{}|{ip}", user_id.unwrap_or(0)))
    {
        return;
    }
    let username: String = username.chars().take(64).collect();
    let result = sqlx::query(
        "INSERT INTO blocked_attempts (id, block_id, ip, kind, user_id, username)
         VALUES ($1, $2, $3::INET, $4, $5, $6)",
    )
    .bind(st.ids.next_id())
    .bind(block_id)
    .bind(security::canonical(ip).to_string())
    .bind(kind)
    .bind(user_id)
    .bind(&username)
    .execute(&st.db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "couldn't record a blocked attempt");
    }
    tracing::info!(%ip, user = %username, kind, "refused: blocked network");
}

/// A signed-in request or gateway connection from a blocked network by
/// someone who isn't exempt: the session ends (they're signed out) and the
/// attempt is written down. True if it was refused.
pub async fn refuse_session(
    st: &AppState,
    ip: IpAddr,
    user_id: i64,
    username: &str,
    token: &str,
    kind: &str,
) -> bool {
    let Some(block) = blocked(ip) else {
        return false;
    };
    if exempt(&st.db, user_id).await {
        return false;
    }
    if let Err(e) = crate::auth::delete_session(&st.db, token).await {
        tracing::warn!(error = %e, "couldn't end a session from a blocked network");
    }
    record_attempt(st, block, ip, kind, Some(user_id), username).await;
    true
}

/// Attempts are kept a year, or while the blocked person's report keeps its
/// evidence (open, or child-safety evidence not yet released).
pub async fn purge_attempts(db: &PgPool) -> AppResult<u64> {
    Ok(sqlx::query(
        "DELETE FROM blocked_attempts a USING ip_blocks b
         WHERE b.id = a.block_id AND a.created_at < now() - interval '365 days'
           AND NOT EXISTS (SELECT 1 FROM reports r
                           WHERE (r.id = b.report_id OR r.reported_user_id = b.source_user_id)
                             AND r.source <> 'ip-match'
                             AND (r.status <> 'closed'
                                  OR (r.category = 'child_safety' AND NOT r.evidence_released)))",
    )
    .execute(db)
    .await?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn networks_and_unblockable_ranges() {
        assert_eq!(
            network_for("203.0.113.7".parse().unwrap()),
            "203.0.113.7/32"
        );
        assert_eq!(
            network_for("2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap()),
            "2001:db8:1:2::/64"
        );
        assert_eq!(
            network_for("::ffff:203.0.113.7".parse().unwrap()),
            "203.0.113.7/32"
        );
        for ip in [
            "10.1.2.3",
            "192.168.1.32",
            "172.16.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.1",
            "::1",
            "fe80::1",
            "fd00::1",
        ] {
            assert!(unblockable_reason(ip.parse().unwrap()).is_some(), "{ip}");
        }
        for ip in ["203.0.113.7", "100.128.0.1", "2001:db8::1", "8.8.8.8"] {
            assert!(unblockable_reason(ip.parse().unwrap()).is_none(), "{ip}");
        }
    }

    #[test]
    fn attempts_are_throttled_per_key() {
        assert!(attempt_due("unit|1|203.0.113.1".into()));
        assert!(!attempt_due("unit|1|203.0.113.1".into()));
        assert!(attempt_due("unit|2|203.0.113.1".into()));
    }
}
