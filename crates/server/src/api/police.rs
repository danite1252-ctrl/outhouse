//! Handing evidence to police from the app (the safety console's "Hand over
//! to police"). Site owner only, after re-entering his password and saying
//! police are present (or asked for it):
//!
//! - `POST /api/safety/reports/{id}/police-links`: builds the sealed package
//!   once (handover.rs) and makes a one-time download link for police to open
//!   on their own device. Only the token's SHA-256 is stored; the link is
//!   shown once.
//! - `POST /api/safety/reports/{id}/police-links/{link}/revoke`
//! - `POST /api/safety/reports/{id}/handover-download`: the same package,
//!   straight to this browser ("Download here", only if police ask).
//! - `GET /evidence/{token}`: a plain page for police (no sign-in) showing the
//!   package's name, size, SHA-256 and expiry, with a Download button.
//!   Opening it doesn't use the link up, so link previews in chat and mail
//!   apps can't burn it.
//! - `POST /evidence/{token}`: the download itself (application/octet-stream,
//!   attachment, no-store), counted and written to the safety log with the
//!   downloader's IP address and browser. 410 once it's used up, expired or
//!   revoked; the package file is deleted from the server then.
//!
//! No route here serves a single file: only the whole sealed package.
//! Tokens never go in the server's logs (only their first characters).

use std::net::IpAddr;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, REFERRER_POLICY, USER_AGENT,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::models::User;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::safety::{log_action, require_owner};
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::handover::{self, Package, Route};
use crate::security::{self, ClientIp};
use crate::state::AppState;

/// Link lifetimes the console offers (hours).
const HOURS: [i64; 3] = [1, 24, 24 * 7];
const MAX_DOWNLOADS: i32 = 3;
/// Wrong passwords allowed per owner account per 15 minutes.
const PASSWORD_FAILS: u32 = 10;
/// Requests for /evidence/... from one address per 15 minutes.
const EVIDENCE_REQUESTS_PER_IP: u32 = 30;
const WINDOW: Duration = Duration::from_secs(15 * 60);

fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A token from a URL: 64 hex characters, or nothing.
fn clean_token(raw: &str) -> Option<&str> {
    (raw.len() == 64 && raw.bytes().all(|b| b.is_ascii_hexdigit())).then_some(raw)
}

fn clean_reference(raw: &Option<String>) -> String {
    raw.as_deref()
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect::<String>()
        .trim()
        .to_string()
}

#[derive(Deserialize)]
pub struct HandoverRequest {
    #[serde(default)]
    password: String,
    /// "Police are present / police asked me for this"
    #[serde(default)]
    police_present: bool,
    /// Officer name, police reference or case number.
    #[serde(default)]
    reference: Option<String>,
    /// Links: how long it works (1, 24 or 168 hours; default 24).
    #[serde(default)]
    hours: Option<i64>,
    /// Links: how many downloads (1 to 3; default 1).
    #[serde(default)]
    max_downloads: Option<i32>,
}

/// The owner, his password again, and "police are present".
async fn confirm(st: &AppState, user: &User, req: &HandoverRequest) -> AppResult<()> {
    require_owner(st, user).await?;
    let key = format!("police-password:{}", user.id);
    let limits = security::get().rate_limits;
    if limits && security::limiter().is_limited(&key, PASSWORD_FAILS, WINDOW) {
        return Err(AppError::TooManyRequests(
            "Too many wrong passwords. Try again in 15 minutes.".into(),
        ));
    }
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&st.db)
        .await?;
    if req.password.is_empty() || !auth::verify_password(req.password.clone(), hash).await {
        security::limiter().hit(&key, WINDOW);
        return Err(AppError::BadRequest("Your password isn't right.".into()));
    }
    if !req.police_present {
        return Err(AppError::BadRequest(
            "Tick that police are with you or have asked you for this.".into(),
        ));
    }
    Ok(())
}

fn operator(user: &User) -> String {
    format!("{} (@{}), site owner", user.display_name, user.username)
}

async fn build_for(
    st: &AppState,
    user: &User,
    report_id: i64,
    reference: &str,
    route: Route,
) -> AppResult<(handover::Request, Package)> {
    let req = handover::Request {
        report_id,
        operator: operator(user),
        reference: reference.to_string(),
        route,
        generated_by: json!({ "id": user.id.to_string(), "username": user.username }),
    };
    let package = handover::build(st, &req, &handover::packages_dir(st)).await?;
    Ok((req, package))
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    id: i64,
    report_id: i64,
    token_prefix: String,
    package_name: String,
    package_file: Option<String>,
    package_sha256: String,
    package_size: i64,
    file_count: i32,
    reference: String,
    created_by_name: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    max_downloads: i32,
    downloads: i32,
    last_download_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    deleted_at: Option<DateTime<Utc>>,
}

const LINK_SELECT: &str = "SELECT l.id, l.report_id, l.token_prefix, l.package_name, l.package_file,
        l.package_sha256, l.package_size, l.file_count, l.reference, u.display_name AS created_by_name,
        l.created_at, l.expires_at, l.max_downloads, l.downloads, l.last_download_at,
        l.revoked_at, l.deleted_at
    FROM police_links l LEFT JOIN users u ON u.id = l.created_by";

impl LinkRow {
    /// active, used, expired or revoked
    fn status(&self) -> &'static str {
        if self.revoked_at.is_some() {
            "revoked"
        } else if self.downloads >= self.max_downloads {
            "used"
        } else if self.expires_at <= Utc::now() || self.package_file.is_none() {
            "expired"
        } else {
            "active"
        }
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "token_prefix": self.token_prefix,
            "package_name": format!("{}.tar.gz", self.package_name),
            "package_sha256": self.package_sha256,
            "package_size": self.package_size,
            "file_count": self.file_count,
            "reference": self.reference,
            "created_by_name": self.created_by_name,
            "created_at": self.created_at,
            "expires_at": self.expires_at,
            "max_downloads": self.max_downloads,
            "downloads": self.downloads,
            "last_download_at": self.last_download_at,
            "revoked_at": self.revoked_at,
            "package_deleted": self.package_file.is_none(),
            "deleted_at": self.deleted_at,
            "status": self.status(),
        })
    }
}

/// A report's police links, newest first (for the console).
pub async fn links_json(st: &AppState, report_id: i64) -> AppResult<Vec<Value>> {
    let sql = format!("{LINK_SELECT} WHERE l.report_id = $1 ORDER BY l.id DESC LIMIT 50");
    let rows = sqlx::query_as::<_, LinkRow>(&sql)
        .bind(report_id)
        .fetch_all(&st.db)
        .await?;
    Ok(rows.iter().map(LinkRow::json).collect())
}

/// Deletes a link's package from the server (used up, expired or revoked).
/// Says whether this call was the one that deleted it.
async fn delete_package(st: &AppState, link_id: i64) -> bool {
    let file: Option<Option<String>> = sqlx::query_scalar(
        "WITH old AS (SELECT id, package_file FROM police_links
                      WHERE id = $1 AND package_file IS NOT NULL FOR UPDATE)
         UPDATE police_links p SET package_file = NULL, deleted_at = now()
         FROM old WHERE p.id = old.id
         RETURNING old.package_file",
    )
    .bind(link_id)
    .fetch_optional(&st.db)
    .await
    .unwrap_or(None);
    let Some(Some(name)) = file else {
        return false;
    };
    if handover::safe_package_file(&name) {
        if let Err(e) = tokio::fs::remove_file(handover::packages_dir(st).join(&name)).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(link = link_id, error = %e, "couldn't delete a police package");
            }
        }
    }
    true
}

/// `POST /api/safety/reports/{id}/police-links`
pub async fn create_link(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<HandoverRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    confirm(&st, &user, &req).await?;
    let hours = req.hours.unwrap_or(24);
    if !HOURS.contains(&hours) {
        return Err(AppError::BadRequest(
            "Pick 1 hour, 24 hours or 7 days.".into(),
        ));
    }
    let max_downloads = req.max_downloads.unwrap_or(1);
    if !(1..=MAX_DOWNLOADS).contains(&max_downloads) {
        return Err(AppError::BadRequest(
            "A link can be downloaded 1 to 3 times.".into(),
        ));
    }
    let reference = clean_reference(&req.reference);
    let (hreq, package) = build_for(&st, &user, id, &reference, Route::Link).await?;
    let token = auth::new_token();
    let prefix = token[..8].to_string();
    let expires_at = Utc::now() + chrono::Duration::hours(hours);
    let link_id = st.ids.next_id();
    let disk_name = package
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let saved = sqlx::query(
        "INSERT INTO police_links (id, report_id, token_sha256, token_prefix, package_name, package_file,
                                   package_sha256, package_size, file_count, reference, created_by,
                                   expires_at, max_downloads)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(link_id)
    .bind(id)
    .bind(token_hash(&token))
    .bind(&prefix)
    .bind(&package.name)
    .bind(&disk_name)
    .bind(&package.sha256)
    .bind(package.size as i64)
    .bind(package.file_count as i32)
    .bind(&reference)
    .bind(user.id)
    .bind(expires_at)
    .bind(max_downloads)
    .execute(&st.db)
    .await;
    if let Err(e) = saved {
        let _ = tokio::fs::remove_file(&package.path).await;
        return Err(e.into());
    }
    let subject: Option<i64> =
        sqlx::query_scalar("SELECT reported_user_id FROM reports WHERE id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?
            .flatten();
    log_action(
        &st,
        Some(user.id),
        "handover",
        Some(id),
        subject,
        handover::log_details(
            &hreq,
            &package,
            &format!(
                ". Link {prefix}… works until {} UTC, for {max_downloads} download(s)",
                expires_at.format("%Y-%m-%d %H:%M")
            ),
        ),
    )
    .await;
    tracing::info!(report = id, link = %prefix, "police download link created");
    let base = super::invites::public_base(&st, &headers);
    let sql = format!("{LINK_SELECT} WHERE l.id = $1");
    let row = sqlx::query_as::<_, LinkRow>(&sql)
        .bind(link_id)
        .fetch_one(&st.db)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            // Shown once: only its SHA-256 is kept.
            "url": format!("{base}/evidence/{token}"),
            "link": row.json(),
            "links": links_json(&st, id).await?,
        })),
    ))
}

/// `POST /api/safety/reports/{id}/police-links/{link}/revoke`
pub async fn revoke_link(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, link)): Path<(i64, i64)>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let prefix: Option<String> = sqlx::query_scalar(
        "UPDATE police_links SET revoked_at = now(), revoked_by = $3
         WHERE id = $1 AND report_id = $2 AND revoked_at IS NULL RETURNING token_prefix",
    )
    .bind(link)
    .bind(id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let Some(prefix) = prefix else {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM police_links WHERE id = $1 AND report_id = $2)",
        )
        .bind(link)
        .bind(id)
        .fetch_one(&st.db)
        .await?;
        return if exists {
            Err(AppError::Conflict("That link was already revoked.".into()))
        } else {
            Err(AppError::NotFound)
        };
    };
    delete_package(&st, link).await;
    log_action(
        &st,
        Some(user.id),
        "handover_revoke",
        Some(id),
        None,
        format!(
            "Revoked the police download link {prefix}…; its package was deleted from the server"
        ),
    )
    .await;
    Ok(Json(json!({ "links": links_json(&st, id).await? })))
}

/// A file streamed in chunks (packages can be big). The file may already be
/// unlinked: an open file keeps its bytes until it's closed.
fn file_body(file: tokio::fs::File) -> Body {
    use tokio::io::AsyncReadExt;
    let stream = futures::stream::unfold(Some(file), |state| async move {
        let mut file = state?;
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<Bytes, std::io::Error>(Bytes::from(buf)), Some(file)))
            }
            Err(e) => Some((Err(e), None)),
        }
    });
    Body::from_stream(stream)
}

fn package_response(
    file: tokio::fs::File,
    name: &str,
    size: u64,
    sha256: &str,
    file_count: i64,
) -> Response {
    let mut res = Response::new(file_body(file));
    let h = res.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        h.insert(CONTENT_DISPOSITION, v);
    }
    h.insert(CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    if let Ok(v) = HeaderValue::from_str(sha256) {
        h.insert("x-package-sha256", v);
    }
    h.insert("x-package-files", HeaderValue::from(file_count));
    res
}

/// `POST /api/safety/reports/{id}/handover-download`: "Download here".
pub async fn download_here(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<HandoverRequest>,
) -> AppResult<Response> {
    confirm(&st, &user, &req).await?;
    let reference = clean_reference(&req.reference);
    let (hreq, package) = build_for(&st, &user, id, &reference, Route::Browser).await?;
    let file = tokio::fs::File::open(&package.path).await;
    // Off the disk at once: this copy goes only to this browser.
    let _ = tokio::fs::remove_file(&package.path).await;
    let file = file.map_err(|e| AppError::Other(e.into()))?;
    let subject: Option<i64> =
        sqlx::query_scalar("SELECT reported_user_id FROM reports WHERE id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?
            .flatten();
    log_action(
        &st,
        Some(user.id),
        "handover",
        Some(id),
        subject,
        handover::log_details(&hreq, &package, ""),
    )
    .await;
    Ok(package_response(
        file,
        &package.file_name(),
        package.size,
        &package.sha256,
        package.file_count as i64,
    ))
}

// ---- The police side (no sign-in) ----------------------------------------------------

fn allow_evidence_request(ip: IpAddr) -> bool {
    !security::get().rate_limits
        || security::limiter().allow(&format!("evidence:{ip}"), EVIDENCE_REQUESTS_PER_IP, WINDOW)
}

async fn find_link(st: &AppState, token: &str) -> AppResult<Option<LinkRow>> {
    let Some(token) = clean_token(token) else {
        return Ok(None);
    };
    let sql = format!("{LINK_SELECT} WHERE l.token_sha256 = $1");
    Ok(sqlx::query_as::<_, LinkRow>(&sql)
        .bind(token_hash(&token.to_ascii_lowercase()))
        .fetch_optional(&st.db)
        .await?)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn size_label(bytes: i64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0)
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

fn page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html>\n<html lang=\"en-AU\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"robots\" content=\"noindex, nofollow\">\n<meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>{}</title>\n<link rel=\"stylesheet\" href=\"/evidence-page.css\">\n</head>\n<body>\n\
         <main class=\"ev-card\">\n{body}\n</main>\n</body>\n</html>\n",
        esc(title)
    );
    let mut res = (status, html).into_response();
    let h = res.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    res
}

fn gone_page(link: Option<&LinkRow>) -> Response {
    let (status, title, text) = match link.map(LinkRow::status) {
        None => (
            StatusCode::NOT_FOUND,
            "Link not found",
            "This evidence link doesn't exist. Check you have the whole link.",
        ),
        Some("used") => (
            StatusCode::GONE,
            "Link already used",
            "This evidence link has already been downloaded as many times as allowed, and the package has been deleted from the server. Ask the person who gave it to you for a new link.",
        ),
        Some("revoked") => (
            StatusCode::GONE,
            "Link withdrawn",
            "This evidence link was withdrawn and the package has been deleted from the server. Ask the person who gave it to you for a new link.",
        ),
        _ => (
            StatusCode::GONE,
            "Link expired",
            "This evidence link has expired and the package has been deleted from the server. Ask the person who gave it to you for a new link.",
        ),
    };
    page(
        status,
        title,
        &format!(
            "<p class=\"ev-kicker\">Evidence package</p>\n<h1>{}</h1>\n<p>{}</p>",
            esc(title),
            esc(text)
        ),
    )
}

fn too_many() -> Response {
    let mut res = page(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many tries",
        "<p class=\"ev-kicker\">Evidence package</p>\n<h1>Too many tries</h1>\n<p>Wait 15 minutes and try again.</p>",
    );
    res.headers_mut()
        .insert("retry-after", HeaderValue::from_static("900"));
    res
}

/// `GET /evidence/{token}`: the page police open (doesn't use the link up).
pub async fn police_page(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(token): Path<String>,
) -> Response {
    if !allow_evidence_request(ip) {
        return too_many();
    }
    let link = match find_link(&st, &token).await {
        Ok(l) => l,
        Err(e) => return e.into_response(),
    };
    let link = match link {
        Some(l) if l.status() == "active" => l,
        other => {
            if let Some(l) = &other {
                delete_package(&st, l.id).await;
            }
            return gone_page(other.as_ref());
        }
    };
    let left = link.max_downloads - link.downloads;
    let body = format!(
        "<p class=\"ev-kicker\">Evidence package for police</p>\n\
         <h1>{name}</h1>\n\
         <dl class=\"ev-facts\">\n\
         <dt>Size</dt><dd>{size}</dd>\n\
         <dt>Files</dt><dd>{files} original file(s), plus the report (JSON), HANDOVER.txt and MANIFEST.sha256</dd>\n\
         <dt>SHA-256</dt><dd><code>{sha}</code></dd>\n\
         <dt>Link works until</dt><dd>{until} UTC</dd>\n\
         <dt>Downloads left</dt><dd>{left} of {max}</dd>\n\
         </dl>\n\
         <form method=\"post\" class=\"ev-form\">\n\
         <button type=\"submit\" class=\"ev-btn\">Download the package</button>\n\
         </form>\n\
         <p class=\"ev-note\">Downloading uses up one of the downloads. Opening this page doesn't. \
         Once they're used up (or the link expires) the package is deleted from the server.</p>\n\
         <p class=\"ev-note\">Check the file you receive: <code>sha256sum {name}</code> should print the SHA-256 above. \
         Inside, HANDOVER.txt explains the contents and <code>sha256sum -c MANIFEST.sha256</code> checks every file.</p>\n\
         <p class=\"ev-warn\">It may contain illegal material. Handle it under your agency's procedures.</p>",
        name = esc(&format!("{}.tar.gz", link.package_name)),
        size = esc(&size_label(link.package_size)),
        files = link.file_count,
        sha = esc(&link.package_sha256),
        until = link.expires_at.format("%Y-%m-%d %H:%M"),
        max = link.max_downloads,
    );
    page(StatusCode::OK, "Evidence package", &body)
}

/// `POST /evidence/{token}`: the download (counted, logged; 410 after).
pub async fn police_download(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    if !allow_evidence_request(ip) {
        return too_many();
    }
    let link = match find_link(&st, &token).await {
        Ok(l) => l,
        Err(e) => return e.into_response(),
    };
    let Some(link) = link else {
        return gone_page(None);
    };
    // Count it first (atomically), so two at once can't both get the last one.
    let claimed: Option<(i32, String)> = sqlx::query_as(
        "UPDATE police_links SET downloads = downloads + 1, last_download_at = now()
         WHERE id = $1 AND revoked_at IS NULL AND expires_at > now()
           AND downloads < max_downloads AND package_file IS NOT NULL
         RETURNING downloads, package_file",
    )
    .bind(link.id)
    .fetch_optional(&st.db)
    .await
    .unwrap_or(None);
    let Some((n, disk_name)) = claimed else {
        delete_package(&st, link.id).await;
        return gone_page(Some(&link));
    };
    let path = handover::packages_dir(&st).join(&disk_name);
    let file = if handover::safe_package_file(&disk_name) {
        tokio::fs::File::open(&path).await.ok()
    } else {
        None
    };
    let agent: String = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("(none)")
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect();
    let Some(file) = file else {
        tracing::warn!(link = %link.token_prefix, "police package missing on disk");
        delete_package(&st, link.id).await;
        return gone_page(Some(&link));
    };
    let last = n >= link.max_downloads;
    if last {
        // The open file still streams; the package is gone from the disk.
        delete_package(&st, link.id).await;
    }
    log_action(
        &st,
        None,
        "handover_download",
        Some(link.report_id),
        None,
        format!(
            "Police download link {}… downloaded ({n} of {}) from IP {ip}, browser: {agent}. Package {}.tar.gz, SHA-256 {}{}",
            link.token_prefix,
            link.max_downloads,
            link.package_name,
            link.package_sha256,
            if last { "; the link is used up and the package was deleted from the server" } else { "" }
        ),
    )
    .await;
    tracing::info!(link = %link.token_prefix, %ip, n, "police package downloaded");
    package_response(
        file,
        &format!("{}.tar.gz", link.package_name),
        link.package_size as u64,
        &link.package_sha256,
        link.file_count as i64,
    )
}

// ---- Tidying up ------------------------------------------------------------------------

/// Deletes packages whose links have expired, were used up or revoked, and
/// stray package files (a build that never became a link). Every 5 minutes.
pub fn spawn_cleaner(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        loop {
            tick.tick().await;
            if let Err(e) = clean_up(&st).await {
                tracing::warn!(error = %e, "police package clean-up failed");
            }
        }
    });
}

pub async fn clean_up(st: &AppState) -> AppResult<usize> {
    let done: Vec<(i64, i64, String, bool)> = sqlx::query_as(
        "SELECT id, report_id, token_prefix, (revoked_at IS NULL AND downloads < max_downloads)
         FROM police_links
         WHERE package_file IS NOT NULL
           AND (expires_at <= now() OR revoked_at IS NOT NULL OR downloads >= max_downloads)",
    )
    .fetch_all(&st.db)
    .await?;
    let mut n = 0;
    for (id, report_id, prefix, expired) in done {
        if delete_package(st, id).await {
            n += 1;
            if expired {
                log_action(
                    st,
                    None,
                    "handover_expired",
                    Some(report_id),
                    None,
                    format!("The police download link {prefix}… expired; its package was deleted from the server"),
                )
                .await;
            }
        }
    }
    // Strays: older than a day and not a live link's package.
    let live: Vec<String> =
        sqlx::query_scalar("SELECT package_file FROM police_links WHERE package_file IS NOT NULL")
            .fetch_all(&st.db)
            .await?;
    let dir = handover::packages_dir(st);
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !handover::safe_package_file(&name) || live.contains(&name) {
                continue;
            }
            let old = entry
                .metadata()
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(24 * 3600));
            if old && tokio::fs::remove_file(entry.path()).await.is_ok() {
                n += 1;
            }
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_text() {
        let t = auth::new_token();
        assert_eq!(clean_token(&t), Some(t.as_str()));
        assert_eq!(clean_token("abc"), None);
        assert_eq!(clean_token(&format!("{}/x", &t[..62])), None);
        assert_eq!(token_hash(&t).len(), 64);
        assert_ne!(token_hash(&t), t);
        assert_eq!(esc("<a href=\"x\">&"), "&lt;a href=&quot;x&quot;&gt;&amp;");
        assert_eq!(
            clean_reference(&Some("  Det. Smith\u{7} #42 ".into())),
            "Det. Smith  #42"
        );
        assert_eq!(size_label(2048), "2 KB");
    }
}
