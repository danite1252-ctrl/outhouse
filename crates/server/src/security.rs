//! Security bits shared by the whole server:
//!
//! - response headers on everything (Content-Security-Policy, HSTS,
//!   clickjacking and sniffing protection, Permissions-Policy);
//! - working out the real client address behind Apache/Caddy without
//!   trusting a client's own `X-Forwarded-For`;
//! - small in-memory rate limits (sign-in, sign-up, invite look-ups);
//! - the sign-up mode (`REGISTRATION=open|invite|closed`);
//! - cleaning links in embeds so only web links ever reach an `href`.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts, Request};
use axum::http::header::{
    CONTENT_SECURITY_POLICY, REFERRER_POLICY, STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use outhouse_common::models::Embed;
use sha2::{Digest, Sha256};

use crate::state::Config;

/// The script in the Battle.net "linked" page (api/bnet.rs), allowed by hash.
/// A unit test below checks it still matches that page.
pub const BNET_PAGE_SCRIPT: &str =
    "try{window.opener&&window.opener.postMessage({outhouse:\"bnet\"},location.origin)}catch(e){}";

/// How people may make accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registration {
    /// Anyone who can reach the site (the default, as before).
    Open,
    /// Only with a working invite code (the invite page passes it along).
    Invite,
    /// Nobody (except the very first account on a new server).
    Closed,
}

impl Registration {
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("invite" | "invites" | "invite-only" | "invite_only") => Registration::Invite,
            Some("closed" | "off" | "no" | "none" | "false" | "0") => Registration::Closed,
            _ => Registration::Open,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Registration::Open => "open",
            Registration::Invite => "invite",
            Registration::Closed => "closed",
        }
    }
}

/// An IPv4 or IPv6 network, e.g. `10.0.0.0/8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a.parse::<IpAddr>().ok()?, Some(p.parse::<u8>().ok()?)),
            None => (s.parse::<IpAddr>().ok()?, None),
        };
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        (prefix <= max).then_some(Cidr { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = canonical(ip);
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// `::ffff:1.2.3.4` → `1.2.3.4`, so IPv4 rules work on dual-stack sockets.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

/// Proxies trusted to tell us the client's address by default: this machine,
/// home and Docker networks (Apache on the LAN, Caddy in Docker).
const DEFAULT_TRUSTED_PROXIES: &str =
    "127.0.0.0/8,::1/128,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,fc00::/7,fe80::/10";

/// Settings worked out once at start-up.
#[derive(Debug)]
pub struct Security {
    pub trusted_proxies: Vec<Cidr>,
    pub csp: String,
    pub registration: Registration,
    pub rate_limits: bool,
}

static SECURITY: OnceLock<Security> = OnceLock::new();

/// Reads `TRUSTED_PROXIES`, `REGISTRATION` and `RATE_LIMITS`, and builds the
/// Content-Security-Policy (hashing the inline scripts of the landing and
/// invite pages). Call once at start-up.
pub fn init(config: &Config) -> &'static Security {
    let env = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let trusted_proxies = env("TRUSTED_PROXIES")
        .unwrap_or_else(|| DEFAULT_TRUSTED_PROXIES.into())
        .split(',')
        .filter_map(|c| {
            let parsed = Cidr::parse(c);
            if parsed.is_none() && !c.trim().is_empty() {
                tracing::warn!(entry = %c, "TRUSTED_PROXIES: not an address or network, ignored");
            }
            parsed
        })
        .collect();
    let mut hashes = Vec::new();
    for page in ["landing.html", "invite.html"] {
        if let Ok(html) = std::fs::read_to_string(config.web_dir.join(page)) {
            hashes.extend(inline_script_hashes(&html));
        }
    }
    hashes.push(script_hash(BNET_PAGE_SCRIPT));
    let security = Security {
        trusted_proxies,
        csp: build_csp(config, &hashes),
        registration: Registration::parse(env("REGISTRATION").as_deref()),
        rate_limits: !matches!(
            env("RATE_LIMITS")
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("off" | "0" | "false" | "no")
        ),
    };
    tracing::info!(
        registration = security.registration.as_str(),
        rate_limits = security.rate_limits,
        "security settings"
    );
    SECURITY.get_or_init(|| security)
}

/// The settings from [`init`] (defaults if it hasn't run, e.g. in unit tests).
pub fn get() -> &'static Security {
    SECURITY.get_or_init(|| Security {
        trusted_proxies: DEFAULT_TRUSTED_PROXIES
            .split(',')
            .filter_map(Cidr::parse)
            .collect(),
        csp: "default-src 'self'; object-src 'none'; frame-ancestors 'none'".into(),
        registration: Registration::Open,
        rate_limits: true,
    })
}

/// `'sha256-…'` for one inline script's text.
pub fn script_hash(script: &str) -> String {
    let digest = Sha256::digest(script.as_bytes());
    format!(
        "'sha256-{}'",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// Hashes of the inline `<script>` blocks in a page (not `src=` scripts and
/// not JSON data blocks, which never run).
pub fn inline_script_hashes(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<script").map(|i| i + pos) {
        let Some(open_end) = lower[start..].find('>').map(|i| i + start) else {
            break;
        };
        let tag = &lower[start..open_end];
        let Some(close) = lower[open_end..].find("</script").map(|i| i + open_end) else {
            break;
        };
        let runs = !tag.contains(" src=") && !tag.contains("application/json");
        if runs {
            out.push(script_hash(&html[open_end + 1..close]));
        }
        pos = close + 1;
    }
    out
}

/// The scheme and host of a URL (`wss://oh.example`), for CSP sources.
fn origin_of(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    })
}

/// The Content-Security-Policy for Outhouse's own pages.
pub fn build_csp(config: &Config, script_hashes: &[String]) -> String {
    // Voice: LiveKit's address (the same site, normally; LIVEKIT_URL), with
    // its http(s) twin for LiveKit's connection check. Without LIVEKIT_URL
    // the client is sent to the page's host on port 7880.
    let mut connect = vec![
        "'self'".to_string(),
        // The Windows/Android app bridge (push to talk, notifications) and Tauri's own IPC.
        "http://outhouse.localhost".into(),
        "https://outhouse.localhost".into(),
        "ipc:".into(),
        "http://ipc.localhost".into(),
    ];
    match config.livekit_url.as_deref().and_then(origin_of) {
        Some(o) => {
            let twin = o
                .replacen("wss://", "https://", 1)
                .replacen("ws://", "http://", 1);
            connect.push(o);
            connect.push(twin);
        }
        None => {
            for s in ["ws", "wss", "http", "https"] {
                connect.push(format!("{s}://*:7880"));
            }
        }
    }
    // Link previews, GIFs and Battle.net pictures come from all over the web.
    let mut img = vec!["'self'", "data:", "blob:", "https:"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    if let Some(o) = origin_of(&config.wowhead_icon_url) {
        if o.starts_with("http://") {
            img.push(o);
        }
    }
    // blob: is for the microphone noise gate (an AudioWorklet the client
    // builds from a string); only a script already running here can make one.
    let mut script = vec!["'self'".to_string(), "blob:".to_string()];
    for h in script_hashes {
        if !script.contains(h) {
            script.push(h.clone());
        }
    }
    [
        "default-src 'self'".to_string(),
        format!("script-src {}", script.join(" ")),
        // Inline styles are used all over the client (element.style); no scripts can run from CSS.
        "style-src 'self' 'unsafe-inline' https://fonts.googleapis.com".into(),
        "font-src 'self' data: https://fonts.gstatic.com".into(),
        format!("img-src {}", img.join(" ")),
        "media-src 'self' data: blob: https:".into(),
        format!("connect-src {}", connect.join(" ")),
        // The invite page tries the outhouse:// app link in a hidden frame (Firefox).
        "frame-src outhouse:".into(),
        "worker-src 'self'".into(),
        "manifest-src 'self'".into(),
        "object-src 'none'".into(),
        "base-uri 'self'".into(),
        "form-action 'self'".into(),
        "frame-ancestors 'none'".into(),
    ]
    .join("; ")
}

fn set_default(headers: &mut HeaderMap, name: HeaderName, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        headers.entry(name).or_insert(v);
    }
}

/// Middleware: security headers on every response (a handler's own
/// Content-Security-Policy, e.g. for uploaded files, wins).
pub async fn headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let sec = get();
    let h = res.headers_mut();
    set_default(h, CONTENT_SECURITY_POLICY, &sec.csp);
    set_default(h, X_CONTENT_TYPE_OPTIONS, "nosniff");
    set_default(h, X_FRAME_OPTIONS, "DENY");
    set_default(h, REFERRER_POLICY, "strict-origin-when-cross-origin");
    // Browsers ignore this over plain http, so it's safe to always send.
    set_default(h, STRICT_TRANSPORT_SECURITY, "max-age=31536000");
    set_default(
        h,
        HeaderName::from_static("permissions-policy"),
        "geolocation=(), payment=(), usb=(), serial=(), hid=(), camera=(self), \
         microphone=(self), display-capture=(self), fullscreen=(self)",
    );
    set_default(
        h,
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    set_default(
        h,
        HeaderName::from_static("x-permitted-cross-domain-policies"),
        "none",
    );
    res
}

/// Does this JSON text contain a NUL character (`\u0000`, or a raw zero
/// byte)? Postgres can't store one in text, so such a request would only
/// end in a server error.
pub fn json_has_nul(body: &[u8]) -> bool {
    if body.contains(&0) {
        return true;
    }
    let mut i = 0;
    while i < body.len() {
        if body[i] == b'\\' {
            if body.len() >= i + 6
                && body[i + 1] == b'u'
                && body[i + 2..i + 6].iter().all(|b| *b == b'0')
            {
                return true;
            }
            i += 2; // skip the escaped character (an escaped backslash then u0000 is just text)
        } else {
            i += 1;
        }
    }
    false
}

/// The most JSON a request may carry (the API's own limit is 2 MB).
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;

/// Middleware: refuses NUL characters in JSON bodies and in the URL with a
/// clear 400 (they'd otherwise reach Postgres and fail with a 500).
pub async fn reject_nul(req: Request, next: Next) -> Response {
    use axum::response::IntoResponse;
    let bad = || {
        crate::error::AppError::BadRequest("Text can't contain NUL characters.".into())
            .into_response()
    };
    let uri = req.uri().to_string().to_ascii_lowercase();
    if uri.contains("%00") {
        return bad();
    }
    let is_json = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim_start()
                .to_ascii_lowercase()
                .starts_with("application/json")
        });
    if !is_json {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_JSON_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return (
                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                axum::Json(serde_json::json!({ "error": "That request is too big." })),
            )
                .into_response()
        }
    };
    if json_has_nul(&bytes) {
        return bad();
    }
    next.run(Request::from_parts(parts, axum::body::Body::from(bytes)))
        .await
}

// ---------- the client's address ----------

/// The address of whoever made the request: the connecting address, or,
/// when that's a trusted proxy, the last address in `X-Forwarded-For` that
/// isn't one (so addresses a client adds itself are never believed).
pub fn client_ip(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[Cidr]) -> IpAddr {
    let unspecified = IpAddr::from([0, 0, 0, 0]);
    let Some(peer) = peer.map(canonical) else {
        return unspecified;
    };
    let is_trusted = |ip: IpAddr| trusted.iter().any(|c| c.contains(ip));
    if !is_trusted(peer) {
        return peer;
    }
    let forwarded: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|s| s.trim().parse::<IpAddr>().ok())
        .map(canonical)
        .collect();
    for ip in forwarded.iter().rev() {
        if !is_trusted(*ip) {
            return *ip;
        }
    }
    // Everything on the way was a trusted proxy: the first hop is the client
    // (e.g. someone at home talking to Apache on the LAN).
    forwarded.first().copied().unwrap_or(peer)
}

/// Addresses that say nothing about who someone is: this machine, private
/// and link-local networks (everyone at home on the LAN), and any trusted
/// proxy (`TRUSTED_PROXIES`). Same-address hints skip these.
pub fn is_local_or_proxy(ip: IpAddr, trusted: &[Cidr]) -> bool {
    let ip = canonical(ip);
    if trusted.iter().any(|c| c.contains(ip)) {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || first & 0xfe00 == 0xfc00 // unique local, fc00::/7
                || first & 0xffc0 == 0xfe80 // link local, fe80::/10
        }
    }
}

/// Extractor: the client's address (see [`client_ip`]).
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        Ok(ClientIp(client_ip(
            peer,
            &parts.headers,
            &get().trusted_proxies,
        )))
    }
}

// ---------- rate limits ----------

/// Fixed-window counters kept in memory (one server process).
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn count(map: &HashMap<String, (Instant, u32)>, key: &str, window: Duration) -> u32 {
        match map.get(key) {
            Some((start, n)) if start.elapsed() < window => *n,
            _ => 0,
        }
    }

    /// Has `key` used up its `max` for this window?
    pub fn is_limited(&self, key: &str, max: u32, window: Duration) -> bool {
        let map = self.buckets.lock().expect("rate limit lock");
        Self::count(&map, key, window) >= max
    }

    /// Counts one use of `key`.
    pub fn hit(&self, key: &str, window: Duration) {
        let mut map = self.buckets.lock().expect("rate limit lock");
        if map.len() > 50_000 {
            map.retain(|_, (start, _)| start.elapsed() < Duration::from_secs(3600));
        }
        let entry = map.entry(key.to_string()).or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= window {
            *entry = (Instant::now(), 0);
        }
        entry.1 = entry.1.saturating_add(1);
    }

    /// Counts a use and says whether it's allowed (false = over the limit).
    pub fn allow(&self, key: &str, max: u32, window: Duration) -> bool {
        if self.is_limited(key, max, window) {
            return false;
        }
        self.hit(key, window);
        true
    }
}

pub fn limiter() -> &'static RateLimiter {
    static LIMITER: OnceLock<RateLimiter> = OnceLock::new();
    LIMITER.get_or_init(RateLimiter::new)
}

/// Failed sign-ins allowed from one address per window.
pub const LOGIN_FAILS_PER_IP: u32 = 20;
/// Failed sign-ins allowed for one account per window (from anywhere).
pub const LOGIN_FAILS_PER_ACCOUNT: u32 = 10;
pub const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
/// New accounts from one address per hour.
pub const SIGNUPS_PER_IP: u32 = 20;
pub const SIGNUP_WINDOW: Duration = Duration::from_secs(3600);
/// Invite look-ups and joins from one address per minute.
pub const INVITE_LOOKUPS_PER_IP: u32 = 120;
pub const INVITE_WINDOW: Duration = Duration::from_secs(60);

/// Checks (and counts) one invite look-up from `ip`.
pub fn allow_invite_lookup(ip: IpAddr) -> bool {
    !get().rate_limits
        || limiter().allow(
            &format!("invite:{ip}"),
            INVITE_LOOKUPS_PER_IP,
            INVITE_WINDOW,
        )
}

// ---------- links in embeds ----------

/// A link that's safe in an `href`/`src`: http(s), or a path on this site.
/// Anything else (`javascript:`, `data:`, `//evil.example`) is dropped.
pub fn safe_link(url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() || u.len() > 2048 || u.chars().any(|c| c.is_control()) {
        return None;
    }
    let lower = u.to_ascii_lowercase();
    let web = (lower.starts_with("https://") || lower.starts_with("http://"))
        && reqwest::Url::parse(u).is_ok();
    let local = u.starts_with('/') && !u.starts_with("//") && !u.starts_with("/\\");
    (web || local).then(|| u.to_string())
}

fn clip_opt(s: &Option<String>, max: usize) -> Option<String> {
    s.as_ref()
        .map(|s| crate::embeds::clip(s.clone(), max))
        .filter(|s| !s.is_empty())
}

/// An embed with only safe links and sensible sizes (bots, feeds and
/// webhooks can send anything).
pub fn clean_embed(e: &Embed) -> Embed {
    let link = |u: &Option<String>| u.as_deref().and_then(safe_link);
    let mut out = e.clone();
    out.url = safe_link(&e.url).unwrap_or_default();
    out.kind = e.kind.chars().take(32).collect();
    out.title = clip_opt(&e.title, 256);
    out.description = clip_opt(&e.description, 4096);
    out.site_name = clip_opt(&e.site_name, 256);
    out.image_url = link(&e.image_url);
    out.color = e.color.map(|c| c & 0xFF_FF_FF);
    out.author_name = clip_opt(&e.author_name, 256);
    out.author_url = link(&e.author_url);
    out.author_icon_url = link(&e.author_icon_url);
    out.footer = clip_opt(&e.footer, 2048);
    out.thumbnail_url = link(&e.thumbnail_url);
    out.timestamp = clip_opt(&e.timestamp, 40);
    out.fields.truncate(25);
    for f in &mut out.fields {
        f.name = crate::embeds::clip(f.name.clone(), 256);
        f.value = crate::embeds::clip(f.value.clone(), 1024);
    }
    out
}

/// Template filling in one pass: `{{NAME}}` becomes its value, and values
/// are never searched for more placeholders (so a guild called `{{DATA}}`
/// stays just text). Unknown placeholders are left as they are.
pub fn fill_template(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let replaced = after.find("}}").and_then(|end| {
            let key = &after[..end + 2];
            values
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (v, end + 2))
        });
        match replaced {
            Some((value, len)) => {
                out.push_str(value);
                rest = &after[len..];
            }
            None => {
                out.push_str("{{");
                rest = &after[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted() -> Vec<Cidr> {
        DEFAULT_TRUSTED_PROXIES
            .split(',')
            .filter_map(Cidr::parse)
            .collect()
    }

    #[test]
    fn cidrs() {
        let c = Cidr::parse("172.16.0.0/12").unwrap();
        assert!(c.contains("172.20.1.2".parse().unwrap()));
        assert!(!c.contains("172.32.0.1".parse().unwrap()));
        assert!(c.contains("::ffff:172.17.0.1".parse().unwrap()));
        assert!(Cidr::parse("::1").unwrap().contains("::1".parse().unwrap()));
        assert!(Cidr::parse("10.0.0.0/33").is_none());
        assert!(Cidr::parse("nonsense").is_none());
    }

    #[test]
    fn client_address_ignores_spoofed_forwarded_for() {
        let t = trusted();
        let mut h = HeaderMap::new();
        // Straight from the internet: the header is ignored.
        h.insert("x-forwarded-for", "10.9.9.9".parse().unwrap());
        assert_eq!(
            client_ip(Some("203.0.113.9".parse().unwrap()), &h, &t),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        // Via Apache (LAN) and Caddy (Docker): a forged first entry is skipped.
        h.insert(
            "x-forwarded-for",
            "1.2.3.4, 198.51.100.7, 192.168.1.32".parse().unwrap(),
        );
        assert_eq!(
            client_ip(Some("172.18.0.3".parse().unwrap()), &h, &t),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        // Someone at home: everything is private, the first hop wins.
        h.insert(
            "x-forwarded-for",
            "192.168.1.50, 192.168.1.32".parse().unwrap(),
        );
        assert_eq!(
            client_ip(Some("172.18.0.3".parse().unwrap()), &h, &t),
            "192.168.1.50".parse::<IpAddr>().unwrap()
        );
        // No header: the proxy itself.
        let empty = HeaderMap::new();
        assert_eq!(
            client_ip(Some("127.0.0.1".parse().unwrap()), &empty, &t),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    /// The production chain: visitor → Apache on 192.168.1.32 (drops any
    /// X-Forwarded-For the visitor sent, then mod_proxy adds the visitor's
    /// address) → Caddy in Docker (`trusted_proxies static private_ranges`:
    /// it trusts Apache, keeps that header and appends Apache's address) →
    /// the server, whose peer is the Caddy container.
    #[test]
    fn client_address_through_apache_and_caddy() {
        let t = trusted();
        let caddy: Option<IpAddr> = Some("172.18.0.5".parse().unwrap());
        let mut h = HeaderMap::new();
        // What Caddy sends on: "<visitor>, <Apache>".
        h.insert(
            "x-forwarded-for",
            "203.0.113.77, 192.168.1.32".parse().unwrap(),
        );
        assert_eq!(
            client_ip(caddy, &h, &t),
            "203.0.113.77".parse::<IpAddr>().unwrap()
        );
        // Docker's userland proxy in front of Caddy adds its gateway too.
        h.insert(
            "x-forwarded-for",
            "203.0.113.77, 172.18.0.1".parse().unwrap(),
        );
        assert_eq!(
            client_ip(caddy, &h, &t),
            "203.0.113.77".parse::<IpAddr>().unwrap()
        );
        // IPv6 visitors come through the same way.
        h.insert(
            "x-forwarded-for",
            "2001:db8::42, 192.168.1.32".parse().unwrap(),
        );
        assert_eq!(
            client_ip(caddy, &h, &t),
            "2001:db8::42".parse::<IpAddr>().unwrap()
        );
        // A visitor's fake header: Apache drops it ("unset ... early"), but
        // even if it got through, the address Apache adds is the one used.
        h.insert(
            "x-forwarded-for",
            "8.8.8.8, 203.0.113.77, 192.168.1.32".parse().unwrap(),
        );
        assert_eq!(
            client_ip(caddy, &h, &t),
            "203.0.113.77".parse::<IpAddr>().unwrap()
        );
        // Two separate headers (some proxies add a second line).
        let mut two = HeaderMap::new();
        two.append("x-forwarded-for", "203.0.113.77".parse().unwrap());
        two.append("x-forwarded-for", "192.168.1.32".parse().unwrap());
        assert_eq!(
            client_ip(caddy, &two, &t),
            "203.0.113.77".parse::<IpAddr>().unwrap()
        );
        // At home on the LAN: the device's own LAN address.
        h.insert(
            "x-forwarded-for",
            "192.168.1.60, 192.168.1.32".parse().unwrap(),
        );
        assert_eq!(
            client_ip(caddy, &h, &t),
            "192.168.1.60".parse::<IpAddr>().unwrap()
        );
        // Someone reaching the server port directly from the internet
        // can't pretend to be anyone.
        h.insert("x-forwarded-for", "203.0.113.77".parse().unwrap());
        assert_eq!(
            client_ip(Some("198.51.100.9".parse().unwrap()), &h, &t),
            "198.51.100.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn local_and_proxy_addresses() {
        let t = trusted();
        for local in [
            "127.0.0.1",
            "192.168.1.32",
            "10.1.2.3",
            "172.18.0.5",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:192.168.1.5",
            "0.0.0.0",
        ] {
            assert!(
                is_local_or_proxy(local.parse().unwrap(), &t),
                "{local} should count as local"
            );
        }
        for public in ["203.0.113.7", "8.8.8.8", "100.64.0.1", "2001:db8::1"] {
            assert!(
                !is_local_or_proxy(public.parse().unwrap(), &t),
                "{public} should count as public"
            );
        }
        // A trusted proxy with a public address (e.g. a cloud load balancer).
        let mut with_lb = t.clone();
        with_lb.push(Cidr::parse("198.51.100.0/24").unwrap());
        assert!(is_local_or_proxy("198.51.100.4".parse().unwrap(), &with_lb));
    }

    #[test]
    fn rate_limiter_counts_per_key() {
        let l = RateLimiter::new();
        let w = Duration::from_secs(60);
        for _ in 0..3 {
            assert!(l.allow("a", 3, w));
        }
        assert!(!l.allow("a", 3, w));
        assert!(l.is_limited("a", 3, w));
        assert!(l.allow("b", 3, w));
        assert!(!l.is_limited("a", 3, Duration::from_nanos(1)));
    }

    #[test]
    fn only_web_links_survive() {
        assert_eq!(
            safe_link(" https://x.example/a "),
            Some("https://x.example/a".into())
        );
        assert_eq!(
            safe_link("HTTP://x.example"),
            Some("HTTP://x.example".into())
        );
        assert_eq!(
            safe_link("/files/abc/x.png"),
            Some("/files/abc/x.png".into())
        );
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:x",
            "//evil.example/x",
            "/\\evil.example",
            "https://exa\nmple.com",
            "",
        ] {
            assert_eq!(safe_link(bad), None, "{bad:?} should be dropped");
        }
        let e = clean_embed(&Embed {
            url: "javascript:alert(document.domain)".into(),
            kind: "rich".into(),
            author_url: Some("javascript:alert(1)".into()),
            image_url: Some("https://img.example/a.png".into()),
            title: Some("x".repeat(5000)),
            ..Default::default()
        });
        assert_eq!(e.url, "");
        assert_eq!(e.author_url, None);
        assert_eq!(e.image_url.as_deref(), Some("https://img.example/a.png"));
        assert_eq!(e.title.unwrap().chars().count(), 256);
    }

    #[test]
    fn templates_fill_in_one_pass() {
        let out = fill_template(
            "<title>{{TITLE}}</title><x a=\"{{TITLE}}\">{{DATA}} {{UNKNOWN}} {{",
            &[
                ("{{TITLE}}", "Join {{DATA}}".into()),
                ("{{DATA}}", "{\"a\":\"b\"}".into()),
            ],
        );
        assert_eq!(
            out,
            "<title>Join {{DATA}}</title><x a=\"Join {{DATA}}\">{\"a\":\"b\"} {{UNKNOWN}} {{"
        );
    }

    #[test]
    fn inline_scripts_are_hashed() {
        let html = "<head><script>\n  a();\n</script><script src=\"/x.js\"></script>\
                    <script type=\"application/json\" id=d>{}</script><SCRIPT>b()</SCRIPT>";
        let hashes = inline_script_hashes(html);
        assert_eq!(hashes, vec![script_hash("\n  a();\n"), script_hash("b()")]);
        // Known value: echo -n 'b()' | openssl dgst -sha256 -binary | base64
        assert!(script_hash("b()").starts_with("'sha256-"));
    }

    #[test]
    fn bnet_page_script_is_allowed() {
        // If the Battle.net page's script changes, update BNET_PAGE_SCRIPT too
        // (otherwise the browser blocks it under the Content-Security-Policy).
        let page = include_str!("api/bnet.rs")
            .replace("{{", "{")
            .replace("}}", "}");
        assert!(
            page.contains(&format!("<script>{BNET_PAGE_SCRIPT}</script>")),
            "api/bnet.rs's page script no longer matches security::BNET_PAGE_SCRIPT"
        );
    }

    #[test]
    fn nul_characters_in_json() {
        assert!(json_has_nul(br#"{"content":"a\u0000b"}"#));
        assert!(json_has_nul(b"{\"content\":\"a\0b\"}"));
        assert!(!json_has_nul(br#"{"content":"a\\u0000b"}"#));
        assert!(json_has_nul(br#"{"content":"a\\\u0000b"}"#));
        assert!(!json_has_nul(br#"{"content":"\u0001 \n \"x\""}"#));
        assert!(!json_has_nul(br#"{"content":"trailing \"#));
    }

    #[test]
    fn registration_modes() {
        assert_eq!(Registration::parse(None), Registration::Open);
        assert_eq!(Registration::parse(Some("open")), Registration::Open);
        assert_eq!(Registration::parse(Some("Invite")), Registration::Invite);
        assert_eq!(Registration::parse(Some("closed")), Registration::Closed);
        assert_eq!(Registration::parse(Some("junk")), Registration::Open);
    }
}
