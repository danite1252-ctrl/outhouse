//! An outhouse's details: name, picture, banner, tagline, About, rules and
//! links (Outhouse settings → Overview), and the "Outhouse info" panel any
//! member can open. Name changes are kept in a short history for members.

use axum::extract::{Multipart, Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::permissions as p;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::guilds::fetch_guild;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::member_ctx;
use crate::state::{AppState, Audience};

pub const NAME_MAX: usize = 100;
pub const TAGLINE_MAX: usize = 80;
pub const DESCRIPTION_MAX: usize = 1000;
pub const RULES_MAX: usize = 2000;
pub const LINKS_MAX: usize = 5;
pub const LINK_LABEL_MAX: usize = 40;
/// Names shown in the info panel.
const NAMES_SHOWN: i64 = 5;

#[derive(Debug, Deserialize, serde::Serialize, Clone, PartialEq)]
pub struct GuildLink {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGuildRequest {
    pub name: Option<String>,
    pub tagline: Option<String>,
    pub description: Option<String>,
    pub rules: Option<String>,
    pub links: Option<Vec<GuildLink>>,
}

#[derive(sqlx::FromRow)]
struct InfoRow {
    name: String,
    owner_id: i64,
    created_at: DateTime<Utc>,
    icon_key: Option<String>,
    banner_key: Option<String>,
    tagline: String,
    description: String,
    rules: String,
    links: Value,
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}

fn clean_name(raw: &str) -> AppResult<String> {
    let name = raw.trim();
    let n = name.chars().count();
    if n == 0 || n > NAME_MAX || name.chars().any(|c| c.is_control()) {
        return Err(AppError::BadRequest(format!(
            "Outhouse names are 1–{NAME_MAX} characters."
        )));
    }
    Ok(name.to_string())
}

fn clean_text(raw: &str, max: usize, what: &str) -> AppResult<String> {
    let text = raw.trim();
    if text.chars().count() > max {
        return Err(AppError::BadRequest(format!(
            "The {what} can be up to {max} characters."
        )));
    }
    if has_control(text) {
        return Err(AppError::BadRequest(format!(
            "The {what} has characters that can't be shown."
        )));
    }
    Ok(text.to_string())
}

/// Links: up to 5, each with a label and an https address that's safe to
/// put in a link (`security::safe_link`).
pub fn clean_links(links: &[GuildLink]) -> AppResult<Vec<GuildLink>> {
    if links.len() > LINKS_MAX {
        return Err(AppError::BadRequest(format!(
            "An outhouse can have up to {LINKS_MAX} links."
        )));
    }
    let mut out = Vec::new();
    for l in links {
        let label = l.label.trim();
        let url = l.url.trim();
        if label.is_empty() && url.is_empty() {
            continue;
        }
        let n = label.chars().count();
        if n == 0 || n > LINK_LABEL_MAX || label.chars().any(|c| c.is_control()) {
            return Err(AppError::BadRequest(format!(
                "Each link needs a label of 1–{LINK_LABEL_MAX} characters."
            )));
        }
        let safe = crate::security::safe_link(url)
            .filter(|u| u.to_ascii_lowercase().starts_with("https://") && u.len() <= 500)
            .filter(|u| {
                reqwest::Url::parse(u)
                    .ok()
                    .and_then(|p| p.host_str().map(|h| !h.is_empty()))
                    .unwrap_or(false)
            });
        let Some(url) = safe else {
            return Err(AppError::BadRequest(format!(
                "“{label}” needs a full https:// address."
            )));
        };
        out.push(GuildLink {
            label: label.to_string(),
            url,
        });
    }
    Ok(out)
}

/// The picture's link, versioned by its scan state (see oh_file_url).
async fn file_url(db: &PgPool, key: &Option<String>, name: &str) -> Option<String> {
    let k = key.as_ref()?;
    sqlx::query_scalar::<_, Option<String>>("SELECT oh_file_url($1, $2)")
        .bind(k)
        .bind(name)
        .fetch_one(db)
        .await
        .ok()
        .flatten()
        .or_else(|| Some(format!("/files/{k}/{name}")))
}

/// Everything the info panel shows (members only).
async fn info_json(st: &AppState, guild_id: i64, user_id: i64) -> AppResult<Value> {
    let ctx = member_ctx(&st.db, guild_id, user_id).await?;
    let g = sqlx::query_as::<_, InfoRow>(
        "SELECT name, owner_id, created_at, icon_key, banner_key, tagline, description, rules, links
         FROM guilds WHERE id = $1",
    )
    .bind(guild_id)
    .fetch_one(&st.db)
    .await?;
    let owner: Option<(String, Option<String>)> = sqlx::query_as(&format!(
        "SELECT COALESCE(m.nickname, u.display_name), {}
         FROM users u LEFT JOIN guild_members m ON m.user_id = u.id AND m.guild_id = $2
         WHERE u.id = $1",
        auth::AVATAR_URL_SQL
    ))
    .bind(g.owner_id)
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await?;
    let member_ids: Vec<i64> =
        sqlx::query_scalar("SELECT user_id FROM guild_members WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_all(&st.db)
            .await?;
    let online = member_ids.iter().filter(|&&id| st.is_online(id)).count();
    let joined_at: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT joined_at FROM guild_members WHERE guild_id = $1 AND user_id = $2",
    )
    .bind(guild_id)
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?;
    let names: Vec<(String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT old_name, new_name, changed_at FROM guild_name_history
         WHERE guild_id = $1 ORDER BY changed_at DESC, id DESC LIMIT $2",
    )
    .bind(guild_id)
    .bind(NAMES_SHOWN)
    .fetch_all(&st.db)
    .await?;
    let icon_url = file_url(&st.db, &g.icon_key, "icon").await;
    let banner_url = file_url(&st.db, &g.banner_key, "banner").await;
    Ok(json!({
        "id": guild_id.to_string(),
        "name": g.name,
        "icon_url": icon_url,
        "banner_url": banner_url,
        "tagline": g.tagline,
        "description": g.description,
        "rules": g.rules,
        "links": g.links,
        "created_at": g.created_at,
        "owner_id": g.owner_id.to_string(),
        "owner": owner.map(|(name, avatar)| json!({
            "id": g.owner_id.to_string(),
            "display_name": name,
            "avatar_url": avatar,
        })),
        "member_count": member_ids.len(),
        "online_count": online,
        "joined_at": joined_at,
        "my_role_ids": ctx.role_ids.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
        "can_manage": ctx.guild_perms() & p::MANAGE_GUILD != 0,
        "name_history": names.into_iter().map(|(old, new, at)| json!({
            "old": old, "new": new, "changed_at": at,
        })).collect::<Vec<_>>(),
    }))
}

/// `GET /api/guilds/{id}/info`: the info panel (any member).
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Value>> {
    Ok(Json(info_json(&st, guild_id, user.id).await?))
}

/// Tells members the outhouse's name or picture changed.
pub async fn announce(st: &AppState, guild_id: i64) {
    if let Ok(guild) = fetch_guild(&st.db, guild_id).await {
        st.dispatch(Audience::Guild(guild_id), ServerEvent::GuildUpdate(guild));
    }
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(60).collect();
    if s.chars().count() > 60 {
        format!("{t}…")
    } else {
        t
    }
}

/// `PATCH /api/guilds/{id}` `{name?, tagline?, description?, rules?, links?}`
/// (Manage Outhouse).
pub async fn update(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<UpdateGuildRequest>,
) -> AppResult<Json<Value>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    ctx.require(p::MANAGE_GUILD)?;
    let old = sqlx::query_as::<_, InfoRow>(
        "SELECT name, owner_id, created_at, icon_key, banner_key, tagline, description, rules, links
         FROM guilds WHERE id = $1",
    )
    .bind(guild_id)
    .fetch_one(&st.db)
    .await?;
    let name = match &req.name {
        Some(n) => clean_name(n)?,
        None => old.name.clone(),
    };
    let tagline = match &req.tagline {
        Some(t) => {
            let t = clean_text(t, TAGLINE_MAX, "tagline")?;
            if t.contains('\n') {
                return Err(AppError::BadRequest("The tagline is one line.".into()));
            }
            t
        }
        None => old.tagline.clone(),
    };
    let description = match &req.description {
        Some(d) => clean_text(d, DESCRIPTION_MAX, "About section")?,
        None => old.description.clone(),
    };
    let rules = match &req.rules {
        Some(r) => clean_text(r, RULES_MAX, "rules")?,
        None => old.rules.clone(),
    };
    let old_links: Vec<GuildLink> = serde_json::from_value(old.links.clone()).unwrap_or_default();
    let links = match &req.links {
        Some(l) => clean_links(l)?,
        None => old_links.clone(),
    };

    let mut tx = st.db.begin().await?;
    sqlx::query(
        "UPDATE guilds SET name = $2, tagline = $3, description = $4, rules = $5, links = $6
         WHERE id = $1",
    )
    .bind(guild_id)
    .bind(&name)
    .bind(&tagline)
    .bind(&description)
    .bind(&rules)
    .bind(serde_json::to_value(&links).unwrap_or_else(|_| json!([])))
    .execute(&mut *tx)
    .await?;
    if name != old.name {
        sqlx::query(
            "INSERT INTO guild_name_history (id, guild_id, old_name, new_name, changed_by)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(st.ids.next_id())
        .bind(guild_id)
        .bind(&old.name)
        .bind(&name)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let details = crate::audit::join([
        crate::audit::change("Name", old.name.as_str(), name.as_str()),
        crate::audit::change("Tagline", short(&old.tagline), short(&tagline)),
        (old.description != description).then(|| "About changed".to_string()),
        (old.rules != rules).then(|| "Rules changed".to_string()),
        (old_links != links).then(|| {
            format!(
                "Links: {}",
                if links.is_empty() {
                    "none".to_string()
                } else {
                    links
                        .iter()
                        .map(|l| l.label.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )
        }),
    ]);
    if !details.is_empty() {
        crate::audit::Entry::new(guild_id, user.id, "guild_update")
            .target("guild", guild_id, name.clone())
            .details(details)
            .save(&st)
            .await;
    }
    announce(&st, guild_id).await;
    Ok(Json(info_json(&st, guild_id, user.id).await?))
}

fn column(kind: &str) -> &'static str {
    if kind == "banner" {
        "banner_key"
    } else {
        "icon_key"
    }
}

async fn replace_image(
    st: &AppState,
    guild_id: i64,
    kind: &str,
    new_key: Option<String>,
) -> AppResult<()> {
    let col = column(kind);
    let old: Option<String> =
        sqlx::query_scalar(&format!("SELECT {col} FROM guilds WHERE id = $1"))
            .bind(guild_id)
            .fetch_one(&st.db)
            .await?;
    sqlx::query(&format!("UPDATE guilds SET {col} = $2 WHERE id = $1"))
        .bind(guild_id)
        .bind(&new_key)
        .execute(&st.db)
        .await?;
    if let Some(old) = old {
        sqlx::query("DELETE FROM profile_files WHERE storage_key = $1")
            .bind(&old)
            .execute(&st.db)
            .await?;
        if !crate::api::safety::file_retained(&st.db, &old).await {
            let _ = tokio::fs::remove_file(st.config.upload_dir.join(&old)).await;
        }
    }
    announce(st, guild_id).await;
    Ok(())
}

async fn upload(
    st: &AppState,
    guild_id: i64,
    user_id: i64,
    kind: &'static str,
    mut form: Multipart,
) -> AppResult<()> {
    let limit = if kind == "banner" {
        super::profiles::MAX_BANNER_BYTES
    } else {
        super::profiles::MAX_AVATAR_BYTES
    };
    let bad = |e: axum::extract::multipart::MultipartError| {
        AppError::BadRequest(format!("Upload failed: {e}"))
    };
    while let Some(field) = form.next_field().await.map_err(bad)? {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or(kind).to_string();
        let content_type = field.content_type().unwrap_or("").to_string();
        let Some(content_type) = super::profiles::image_type(&content_type, &filename) else {
            return Err(AppError::BadRequest(
                "Pictures must be PNG, JPEG, GIF or WebP.".into(),
            ));
        };
        let data = field.bytes().await.map_err(bad)?;
        if data.is_empty() {
            return Err(AppError::BadRequest("That file is empty.".into()));
        }
        if data.len() > limit {
            return Err(AppError::BadRequest(format!(
                "Pictures can be up to {} MB.",
                limit / 1024 / 1024
            )));
        }
        let key = auth::random_code(24, super::profiles::KEY_ALPHABET);
        tokio::fs::create_dir_all(&st.config.upload_dir)
            .await
            .map_err(|e| anyhow::anyhow!("creating upload folder: {e}"))?;
        tokio::fs::write(st.config.upload_dir.join(&key), &data)
            .await
            .map_err(|e| anyhow::anyhow!("saving upload: {e}"))?;
        sqlx::query(
            "INSERT INTO profile_files (storage_key, user_id, filename, content_type)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&key)
        .bind(user_id)
        .bind(kind)
        .bind(content_type)
        .execute(&st.db)
        .await?;
        // Checked by the image detector like avatars (hidden behind a
        // placeholder until it's been looked at, where that applies).
        crate::api::detector::track_upload(
            st,
            if kind == "banner" {
                crate::api::detector::KIND_GUILD_BANNER
            } else {
                crate::api::detector::KIND_GUILD_ICON
            },
            &key,
            content_type,
            user_id,
            Some(guild_id),
        )
        .await;
        return replace_image(st, guild_id, kind, Some(key)).await;
    }
    Err(AppError::BadRequest("Choose a picture to upload.".into()))
}

async fn set_image(
    st: AppState,
    user_id: i64,
    guild_id: i64,
    kind: &'static str,
    form: Option<Multipart>,
) -> AppResult<Json<Value>> {
    let ctx = member_ctx(&st.db, guild_id, user_id).await?;
    ctx.require(p::MANAGE_GUILD)?;
    let name: String = sqlx::query_scalar("SELECT name FROM guilds WHERE id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    let what = if kind == "banner" {
        "banner"
    } else {
        "picture"
    };
    let details = if form.is_some() {
        format!("New {what}")
    } else {
        format!("Removed the {what}")
    };
    match form {
        Some(form) => upload(&st, guild_id, user_id, kind, form).await?,
        None => replace_image(&st, guild_id, kind, None).await?,
    }
    crate::audit::Entry::new(guild_id, user_id, "guild_update")
        .target("guild", guild_id, name)
        .details(details)
        .save(&st)
        .await;
    Ok(Json(info_json(&st, guild_id, user_id).await?))
}

pub async fn upload_icon(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    form: Multipart,
) -> AppResult<Json<Value>> {
    set_image(st, user.id, guild_id, "icon", Some(form)).await
}

pub async fn delete_icon(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Value>> {
    set_image(st, user.id, guild_id, "icon", None).await
}

pub async fn upload_banner(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    form: Multipart,
) -> AppResult<Json<Value>> {
    set_image(st, user.id, guild_id, "banner", Some(form)).await
}

pub async fn delete_banner(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Value>> {
    set_image(st, user.id, guild_id, "banner", None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(label: &str, url: &str) -> GuildLink {
        GuildLink {
            label: label.into(),
            url: url.into(),
        }
    }

    #[test]
    fn links_must_be_https_with_labels() {
        assert!(clean_links(&[link("Site", "https://example.com/x")]).is_ok());
        assert!(clean_links(&[link("Site", "http://example.com")]).is_err());
        assert!(clean_links(&[link("Site", "javascript:alert(1)")]).is_err());
        assert!(clean_links(&[link("Site", "/relative")]).is_err());
        assert!(clean_links(&[link("", "https://example.com")]).is_err());
        assert!(clean_links(&[link("", "")]).unwrap().is_empty());
        let six: Vec<_> = (0..6)
            .map(|i| link(&format!("L{i}"), "https://e.com"))
            .collect();
        assert!(clean_links(&six).is_err());
    }

    #[test]
    fn names_are_trimmed_and_limited() {
        assert_eq!(clean_name("  Hut  ").unwrap(), "Hut");
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(101)).is_err());
        assert!(clean_name(&"x".repeat(100)).is_ok());
    }
}
