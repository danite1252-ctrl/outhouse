//! Invites: make shareable links (with an expiry and a use limit), list and
//! revoke them, look one up without signing in, join with one, and the
//! `/invite/<code>` page those links open (plus Android's App Links file).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{Guild, DM_GUILD};
use outhouse_common::permissions as p;
use serde::{Deserialize, Serialize};

use super::guilds::fetch_guild;
use super::members::fetch_member;
use crate::audit;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::member_ctx;
use crate::security::{self, ClientIp};
use crate::state::{AppState, Audience};

/// Invite codes can't be guessed by hammering the look-up (or join) page.
fn check_lookup_rate(ip: std::net::IpAddr) -> AppResult<()> {
    if security::allow_invite_lookup(ip) {
        Ok(())
    } else {
        Err(AppError::TooManyRequests(
            "That's a lot of invite look-ups. Wait a minute and try again.".into(),
        ))
    }
}

const INVITE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
/// An invite made without saying otherwise lasts a week.
const DEFAULT_MAX_AGE_SECS: i64 = 7 * 24 * 3600;
/// The longest an expiring invite can last (0 = never expires).
const LONGEST_MAX_AGE_SECS: i64 = 30 * 24 * 3600;
const MOST_MAX_USES: i32 = 1000;
/// Invites one person can have going in one outhouse at once.
const MAX_LIVE_INVITES_PER_MEMBER: i64 = 50;

#[derive(Debug, Default, Deserialize)]
struct CreateInviteRequest {
    /// Seconds until it expires; 0 = never. Missing = 7 days.
    max_age_secs: Option<i64>,
    /// How many people can join with it; 0 = no limit. Missing = no limit.
    max_uses: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InviteCreator {
    #[serde(with = "outhouse_common::id::as_string")]
    pub id: i64,
    pub name: String,
    pub avatar_url: Option<String>,
}

/// An invite as its makers (and moderators) see it.
#[derive(Debug, Clone, Serialize)]
pub struct Invite {
    pub code: String,
    /// The full link to share, e.g. https://outhouse.example/invite/abc123.
    pub url: String,
    #[serde(with = "outhouse_common::id::as_string")]
    pub guild_id: i64,
    pub creator: InviteCreator,
    pub uses: i32,
    pub max_uses: Option<i32>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InviteGuild {
    #[serde(with = "outhouse_common::id::as_string")]
    pub id: i64,
    pub name: String,
}

/// What anyone with the link can see, signed in or not.
#[derive(Debug, Clone, Serialize)]
pub struct InvitePreview {
    pub code: String,
    pub url: String,
    pub guild: InviteGuild,
    pub inviter: Option<InviteCreator>,
    pub member_count: i64,
    pub online_count: i64,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct InviteRow {
    code: String,
    guild_id: i64,
    guild_name: String,
    creator_id: i64,
    creator_name: String,
    creator_avatar: Option<String>,
    uses: i32,
    max_uses: Option<i32>,
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    usable: bool,
}

const INVITE_SELECT: &str = "
    SELECT i.code, i.guild_id, g.name AS guild_name, i.creator_id,
           COALESCE(NULLIF(m.nickname, ''), u.display_name) AS creator_name,
           CASE WHEN u.avatar_key IS NULL THEN NULL ELSE '/files/' || u.avatar_key || '/avatar' END AS creator_avatar,
           i.uses, i.max_uses, i.expires_at, i.created_at,
           (i.expires_at IS NULL OR i.expires_at > now())
             AND (i.max_uses IS NULL OR i.uses < i.max_uses) AS usable
    FROM invites i
    JOIN guilds g ON g.id = i.guild_id
    JOIN users u ON u.id = i.creator_id
    LEFT JOIN guild_members m ON m.guild_id = i.guild_id AND m.user_id = i.creator_id";

impl InviteRow {
    fn creator(&self) -> InviteCreator {
        InviteCreator {
            id: self.creator_id,
            name: self.creator_name.clone(),
            avatar_url: self.creator_avatar.clone(),
        }
    }

    fn into_invite(self, base: &str) -> Invite {
        Invite {
            url: invite_url(base, &self.code),
            creator: self.creator(),
            code: self.code,
            guild_id: self.guild_id,
            uses: self.uses,
            max_uses: self.max_uses,
            expires_at: self.expires_at,
            created_at: self.created_at,
        }
    }
}

/// Invite codes are short and only use letters and digits; anything else
/// can't be one (and never reaches the database or the page).
fn clean_code(code: &str) -> Option<&str> {
    let code = code.trim();
    (!code.is_empty() && code.len() <= 32 && code.chars().all(|c| c.is_ascii_alphanumeric()))
        .then_some(code)
}

async fn find_invite(st: &AppState, code: &str) -> AppResult<Option<InviteRow>> {
    let Some(code) = clean_code(code) else {
        return Ok(None);
    };
    let sql = format!("{INVITE_SELECT} WHERE i.code = $1");
    Ok(sqlx::query_as::<_, InviteRow>(&sql)
        .bind(code)
        .fetch_optional(&st.db)
        .await?)
}

/// The site's public address: `PUBLIC_URL` when it's set, otherwise worked
/// out from the request (Apache/Caddy pass on the original host and scheme).
pub fn public_base(st: &AppState, headers: &HeaderMap) -> String {
    if let Some(base) = &st.config.public_url {
        return base.clone();
    }
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
            .filter(|v| {
                !v.is_empty()
                    && v.chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']')
                    })
            })
    };
    let host = get("x-forwarded-host")
        .or_else(|| get("host"))
        .unwrap_or_else(|| "localhost:8080".into());
    let proto = match get("x-forwarded-proto").as_deref() {
        Some("https") => "https",
        Some("http") => "http",
        _ => "http",
    };
    format!("{proto}://{host}")
}

fn invite_url(base: &str, code: &str) -> String {
    format!("{base}/invite/{code}")
}

/// `POST /api/guilds/{id}/invites` with an optional
/// `{"max_age_secs": 3600, "max_uses": 5}` (an empty body = 7 days, no limit).
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Invite>> {
    if guild_id == DM_GUILD {
        return Err(AppError::NotFound);
    }
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::CREATE_INVITE)?;
    let req: CreateInviteRequest = if body.iter().all(|b| b.is_ascii_whitespace()) {
        CreateInviteRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|_| AppError::BadRequest("That invite request didn't make sense.".into()))?
    };
    let max_age = req.max_age_secs.unwrap_or(DEFAULT_MAX_AGE_SECS);
    if max_age != 0 && !(60..=LONGEST_MAX_AGE_SECS).contains(&max_age) {
        return Err(AppError::BadRequest(
            "Invites can last from a minute to 30 days, or forever.".into(),
        ));
    }
    let max_uses = req.max_uses.unwrap_or(0);
    if !(0..=MOST_MAX_USES).contains(&max_uses) {
        return Err(AppError::BadRequest(
            "Max uses is 1 to 1000 (or no limit).".into(),
        ));
    }
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM invites
         WHERE guild_id = $1 AND creator_id = $2
           AND (expires_at IS NULL OR expires_at > now())
           AND (max_uses IS NULL OR uses < max_uses)",
    )
    .bind(guild_id)
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if live >= MAX_LIVE_INVITES_PER_MEMBER {
        return Err(AppError::BadRequest(
            "You've got plenty of invites going already. Revoke a few first.".into(),
        ));
    }

    let code = auth::random_code(8, INVITE_ALPHABET);
    sqlx::query(
        "INSERT INTO invites (code, guild_id, creator_id, max_uses, expires_at)
         VALUES ($1, $2, $3, $4,
                 CASE WHEN $5::BIGINT = 0 THEN NULL ELSE now() + $5::BIGINT * interval '1 second' END)",
    )
    .bind(&code)
    .bind(guild_id)
    .bind(user.id)
    .bind((max_uses > 0).then_some(max_uses))
    .bind(max_age)
    .execute(&st.db)
    .await?;
    let row = find_invite(&st, &code).await?.ok_or(AppError::NotFound)?;
    Ok(Json(row.into_invite(&public_base(&st, &headers))))
}

/// `GET /api/guilds/{id}/invites`: invites that still work. Manage Outhouse
/// sees everyone's; anyone else who can invite sees their own.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Json<Vec<Invite>>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let all = ctx.guild_perms() & p::MANAGE_GUILD == p::MANAGE_GUILD;
    if !all {
        ctx.require(p::CREATE_INVITE)?;
    }
    let sql = format!(
        "{INVITE_SELECT}
         WHERE i.guild_id = $1 AND ($2 OR i.creator_id = $3)
           AND (i.expires_at IS NULL OR i.expires_at > now())
           AND (i.max_uses IS NULL OR i.uses < i.max_uses)
         ORDER BY i.created_at DESC
         LIMIT 200"
    );
    let rows = sqlx::query_as::<_, InviteRow>(&sql)
        .bind(guild_id)
        .bind(all)
        .bind(user.id)
        .fetch_all(&st.db)
        .await?;
    let base = public_base(&st, &headers);
    Ok(Json(
        rows.into_iter().map(|r| r.into_invite(&base)).collect(),
    ))
}

/// `DELETE /api/invites/{code}`: the person who made it, or Manage Outhouse.
pub async fn revoke(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(code): Path<String>,
) -> AppResult<StatusCode> {
    let row = find_invite(&st, &code).await?.ok_or(AppError::NotFound)?;
    let ctx = member_ctx(&st.db, row.guild_id, user.id).await?;
    if row.creator_id != user.id {
        ctx.require(p::MANAGE_GUILD)?;
    }
    sqlx::query("DELETE FROM invites WHERE code = $1")
        .bind(&row.code)
        .execute(&st.db)
        .await?;
    if row.creator_id != user.id {
        audit::Entry::new(row.guild_id, user.id, "invite_delete")
            .target("user", row.creator_id, row.creator_name.clone())
            .details(format!("Revoked invite {} ({} uses)", row.code, row.uses))
            .save(&st)
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn preview_for(st: &AppState, row: &InviteRow, base: &str) -> AppResult<InvitePreview> {
    let member_ids: Vec<i64> =
        sqlx::query_scalar("SELECT user_id FROM guild_members WHERE guild_id = $1")
            .bind(row.guild_id)
            .fetch_all(&st.db)
            .await?;
    let online = member_ids.iter().filter(|id| st.is_online(**id)).count();
    Ok(InvitePreview {
        code: row.code.clone(),
        url: invite_url(base, &row.code),
        guild: InviteGuild {
            id: row.guild_id,
            name: row.guild_name.clone(),
        },
        inviter: Some(row.creator()),
        member_count: member_ids.len() as i64,
        online_count: online as i64,
        expires_at: row.expires_at,
    })
}

/// `GET /api/invites/{code}`: who's inviting you where (no sign-in needed).
/// Expired, used-up and revoked invites are "not found".
pub async fn preview(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> AppResult<Json<InvitePreview>> {
    check_lookup_rate(ip)?;
    let row = find_invite(&st, &code)
        .await?
        .filter(|r| r.usable)
        .ok_or(AppError::NotFound)?;
    crate::perms::refuse_if_shut_down(&st.db, row.guild_id).await?;
    Ok(Json(
        preview_for(&st, &row, &public_base(&st, &headers)).await?,
    ))
}

/// `POST /api/invites/{code}`: join the outhouse. Already a member? You just
/// get the outhouse back (and the invite isn't used up).
pub async fn join(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    ClientIp(ip): ClientIp,
    Path(code): Path<String>,
) -> AppResult<Json<Guild>> {
    check_lookup_rate(ip)?;
    let row = find_invite(&st, &code).await?.ok_or(AppError::NotFound)?;
    crate::perms::refuse_if_shut_down(&st.db, row.guild_id).await?;
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM guild_members WHERE guild_id = $1 AND user_id = $2)",
    )
    .bind(row.guild_id)
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if already {
        return Ok(Json(fetch_guild(&st.db, row.guild_id).await?));
    }
    if !row.usable {
        return Err(AppError::BadRequest(
            "That invite has expired or been used up.".into(),
        ));
    }
    let banned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM bans WHERE guild_id = $1 AND user_id = $2)",
    )
    .bind(row.guild_id)
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if banned {
        return Err(AppError::BadRequest(
            "You've been banned from that outhouse.".into(),
        ));
    }

    // Count the use and add the member together, so a 1-use invite can't
    // let two people in at once.
    let mut tx = st.db.begin().await?;
    let counted = sqlx::query(
        "UPDATE invites SET uses = uses + 1
         WHERE code = $1
           AND (expires_at IS NULL OR expires_at > now())
           AND (max_uses IS NULL OR uses < max_uses)",
    )
    .bind(&row.code)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !counted {
        return Err(AppError::BadRequest(
            "That invite has expired or been used up.".into(),
        ));
    }
    let joined = sqlx::query(
        "INSERT INTO guild_members (guild_id, user_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(row.guild_id)
    .bind(user.id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if joined {
        tx.commit().await?;
    } else {
        // Joined some other way a moment ago: don't use up the invite.
        tx.rollback().await?;
    }

    let guild = fetch_guild(&st.db, row.guild_id).await?;
    if joined {
        // Message requests between them and people here go through now.
        crate::api::requests::accept_for_new_member(&st, user.id, guild.id).await?;
        let mut member = fetch_member(&st.db, guild.id, user.id).await?;
        member.online = st.is_online(user.id);
        st.dispatch(
            Audience::Guild(guild.id),
            ServerEvent::MemberJoin {
                guild_id: guild.id,
                member,
            },
        );
        st.dispatch(
            Audience::User(user.id),
            ServerEvent::GuildCreate(guild.clone()),
        );
    }
    Ok(Json(guild))
}

// ---------- the /invite/<code> page ----------

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// JSON that's safe inside a `<script type="application/json">` block.
fn script_json(value: &serde_json::Value) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Up to two letters for the outhouse's tile ("The Long Drop" → "TL").
pub fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "O".into()
    } else {
        letters
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Fills in the page template (`web/invite.html`): `{{NAME}}` placeholders
/// get HTML-escaped values.
pub fn render_invite_page(
    template: &str,
    base: &str,
    preview: Option<&InvitePreview>,
    code: &str,
    android_package: &str,
) -> String {
    let page_url = invite_url(base, code);
    let image = format!("{base}/icons/icon-512.png");
    let (state, title, description, guild, inviter, members, online, data) = match preview {
        Some(p) => {
            let inviter = p
                .inviter
                .as_ref()
                .map(|i| i.name.clone())
                .unwrap_or_else(|| "Someone".into());
            (
                "valid",
                format!("You've been invited to {} on Outhouse", p.guild.name),
                format!(
                    "{inviter} saved you a seat in {}. {} · {} online. Chat, voice and raid sign-ups out the back.",
                    p.guild.name,
                    plural(p.member_count, "member", "members"),
                    p.online_count
                ),
                p.guild.name.clone(),
                inviter,
                plural(p.member_count, "member", "members"),
                format!("{} online", p.online_count),
                serde_json::json!({ "valid": true, "invite": p }),
            )
        }
        None => (
            "invalid",
            "This invite has gone down the long drop".to_string(),
            "This Outhouse invite has expired or been revoked. Ask whoever sent it for a fresh one."
                .to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            serde_json::json!({ "valid": false, "code": code }),
        ),
    };
    let mut data = data;
    data["android_package"] = serde_json::Value::from(android_package);
    let replacements: [(&str, String); 12] = [
        ("{{STATE}}", state.to_string()),
        ("{{TITLE}}", html_escape(&title)),
        ("{{DESCRIPTION}}", html_escape(&description)),
        ("{{URL}}", html_escape(&page_url)),
        ("{{IMAGE}}", html_escape(&image)),
        ("{{GUILD_NAME}}", html_escape(&guild)),
        ("{{GUILD_INITIALS}}", html_escape(&initials(&guild))),
        ("{{INVITER}}", html_escape(&inviter)),
        ("{{MEMBERS}}", html_escape(&members)),
        ("{{ONLINE}}", html_escape(&online)),
        ("{{CODE}}", html_escape(code)),
        ("{{DATA}}", script_json(&data)),
    ];
    // One pass, so a name like "{{DATA}}" can't pull other values (with
    // their quotes) into an attribute.
    security::fill_template(template, &replacements)
}

const FALLBACK_TEMPLATE: &str =
    "<!doctype html><html lang=\"en-AU\"><head><meta charset=\"utf-8\">\
<title>{{TITLE}}</title><meta property=\"og:title\" content=\"{{TITLE}}\">\
<meta property=\"og:description\" content=\"{{DESCRIPTION}}\"></head>\
<body data-state=\"{{STATE}}\"><h1>{{TITLE}}</h1><p>{{DESCRIPTION}}</p>\
<p><a href=\"/app?invite={{CODE}}\">Open Outhouse</a></p></body></html>";

/// `GET /invite/{code}`: the page an invite link opens. Works signed out;
/// link previews (Discord, Messenger, WhatsApp) read its Open Graph tags.
pub async fn page(
    State(st): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = check_lookup_rate(ip) {
        return e.into_response();
    }
    let base = public_base(&st, &headers);
    let row = match find_invite(&st, &code).await {
        Ok(row) => row.filter(|r| r.usable),
        Err(e) => return e.into_response(),
    };
    // A shut-down outhouse's invites look like dead ones.
    let row = match row {
        Some(r)
            if crate::perms::refuse_if_shut_down(&st.db, r.guild_id)
                .await
                .is_err() =>
        {
            None
        }
        other => other,
    };
    let preview = match &row {
        Some(r) => match preview_for(&st, r, &base).await {
            Ok(p) => Some(p),
            Err(e) => return e.into_response(),
        },
        None => None,
    };
    let template = tokio::fs::read_to_string(st.config.web_dir.join("invite.html"))
        .await
        .unwrap_or_else(|_| FALLBACK_TEMPLATE.to_string());
    let shown_code = clean_code(&code).unwrap_or("");
    let html = render_invite_page(
        &template,
        &base,
        preview.as_ref(),
        shown_code,
        &st.config.android_package,
    );
    let status = if preview.is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    (
        status,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (
                header::HeaderName::from_static("x-robots-tag"),
                HeaderValue::from_static("noindex"),
            ),
        ],
        html,
    )
        .into_response()
}

/// `GET /.well-known/assetlinks.json`: tells Android that the Outhouse app
/// may open this site's /invite links (Android App Links).
pub async fn asset_links(State(st): State<AppState>) -> Response {
    let fingerprints = &st.config.android_cert_sha256;
    if fingerprints.is_empty() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!([]))).into_response();
    }
    let body = serde_json::json!([{
        "relation": ["delegate_permission/common.handle_all_urls"],
        "target": {
            "namespace": "android_app",
            "package_name": st.config.android_package,
            "sha256_cert_fingerprints": fingerprints,
        }
    }]);
    (
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=3600"),
        )],
        Json(body),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_letters_and_digits() {
        assert_eq!(clean_code(" abC123 "), Some("abC123"));
        assert_eq!(clean_code("../etc"), None);
        assert_eq!(clean_code("<script>"), None);
        assert_eq!(clean_code(""), None);
    }

    #[test]
    fn fingerprints_get_colons() {
        use crate::state::{format_fingerprint, DEFAULT_ANDROID_CERT_SHA256};
        let f = format_fingerprint(DEFAULT_ANDROID_CERT_SHA256).unwrap();
        assert!(f.starts_with("D6:31:45:91:"));
        assert_eq!(f.len(), 95);
        assert_eq!(format_fingerprint(&f), Some(f.clone()));
        assert_eq!(format_fingerprint("abc"), None);
    }

    #[test]
    fn initials_from_names() {
        assert_eq!(initials("The Long Drop"), "TL");
        assert_eq!(initials("guild"), "G");
        assert_eq!(initials("  "), "O");
    }

    #[test]
    fn page_escapes_names() {
        let p = InvitePreview {
            code: "abc".into(),
            url: "https://x/invite/abc".into(),
            guild: InviteGuild {
                id: 1,
                name: "<b>Tom & Jerry</b>".into(),
            },
            inviter: Some(InviteCreator {
                id: 2,
                name: "\"Al\"".into(),
                avatar_url: None,
            }),
            member_count: 1,
            online_count: 0,
            expires_at: None,
        };
        let html = render_invite_page(
            "<title>{{TITLE}}</title><i>{{INVITER}}</i><p>{{MEMBERS}}</p><script>{{DATA}}</script>",
            "https://x",
            Some(&p),
            "abc",
            "cc.example",
        );
        assert!(html.contains("&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;"));
        assert!(html.contains("&quot;Al&quot;"));
        assert!(!html.contains("<b>"));
        assert!(html.contains("1 member"));
    }

    #[test]
    fn page_placeholders_in_names_stay_text() {
        let p = InvitePreview {
            code: "abc".into(),
            url: "https://x/invite/abc".into(),
            guild: InviteGuild {
                id: 1,
                name: "{{DATA}}".into(),
            },
            inviter: Some(InviteCreator {
                id: 2,
                name: "\" autofocus onfocus=\"alert(1)".into(),
                avatar_url: None,
            }),
            member_count: 1,
            online_count: 0,
            expires_at: None,
        };
        let html = render_invite_page(
            "<meta content=\"{{TITLE}}\"><script>{{DATA}}</script>",
            "https://x",
            Some(&p),
            "abc",
            "cc.example",
        );
        let meta = html.split("<script>").next().unwrap();
        assert!(meta.contains("{{DATA}}"), "{meta}");
        assert!(!meta.contains("autofocus"), "{meta}");
        assert_eq!(meta.matches('"').count(), 2, "{meta}");
    }
}
