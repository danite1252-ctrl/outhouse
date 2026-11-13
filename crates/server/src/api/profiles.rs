//! Profiles: avatar, banner, "about me", accent colour and background.

use axum::extract::{Multipart, Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::models::{Profile, User};

use serde_json::{json, Value};

use super::account::refresh_my_guilds;
use crate::auth::{self, AuthUser, USER_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub(crate) const KEY_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const MAX_AVATAR_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_BANNER_BYTES: usize = 10 * 1024 * 1024;

#[derive(sqlx::FromRow)]
struct ProfileRow {
    id: i64,
    username: String,
    display_name: String,
    avatar_key: Option<String>,
    banner_key: Option<String>,
    about_me: String,
    profile_theme: String,
    accent_color: i32,
    is_bot: bool,
    created_at: DateTime<Utc>,
}

/// Someone's profile. You can see people you share an outhouse with.
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(user_id): Path<i64>,
) -> AppResult<Json<Profile>> {
    let row = sqlx::query_as::<_, ProfileRow>(
        "SELECT u.id, u.username, u.display_name, u.avatar_key, u.banner_key, u.about_me,
                u.profile_theme, u.accent_color, u.is_bot, u.created_at
         FROM users u
         WHERE u.id = $1
           AND (u.id = $2 OR EXISTS (
                SELECT 1 FROM guild_members a
                JOIN guild_members b ON a.guild_id = b.guild_id
                WHERE a.user_id = $1 AND b.user_id = $2))",
    )
    .bind(user_id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    // Suspended accounts' profiles are hidden (except from the site owner).
    if row.id != user.id
        && super::safety::hidden_profiles(&st.db, user.id, &[row.id])
            .await?
            .contains(&row.id)
    {
        return Err(AppError::NotFound);
    }
    Ok(Json(Profile {
        id: row.id,
        username: row.username,
        display_name: row.display_name,
        avatar_url: row.avatar_key.map(|k| format!("/files/{k}/avatar")),
        banner_url: row.banner_key.map(|k| format!("/files/{k}/banner")),
        about_me: row.about_me,
        profile_theme: row.profile_theme,
        accent_color: row.accent_color,
        bot: row.is_bot,
        created_at: row.created_at,
        characters: super::wow::characters_of(&st.db, row.id).await?,
    }))
}

pub(crate) fn image_type(content_type: &str, filename: &str) -> Option<&'static str> {
    let by_type = match content_type {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    };
    by_type.or_else(|| {
        let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            _ => None,
        }
    })
}

/// "avatar" or "banner" → the users column it lives in.
fn column(kind: &str) -> &'static str {
    if kind == "banner" {
        "banner_key"
    } else {
        "avatar_key"
    }
}

async fn replace_image(
    st: &AppState,
    user_id: i64,
    kind: &str,
    new_key: Option<String>,
) -> AppResult<()> {
    let col = column(kind);
    let old: Option<String> = sqlx::query_scalar(&format!("SELECT {col} FROM users WHERE id = $1"))
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    sqlx::query(&format!("UPDATE users SET {col} = $2 WHERE id = $1"))
        .bind(user_id)
        .bind(&new_key)
        .execute(&st.db)
        .await?;
    if let Some(old) = old {
        sqlx::query("DELETE FROM profile_files WHERE storage_key = $1")
            .bind(&old)
            .execute(&st.db)
            .await?;
        // Kept while a safety report refers to it.
        if !crate::api::safety::file_retained(&st.db, &old).await {
            let _ = tokio::fs::remove_file(st.config.upload_dir.join(&old)).await;
        }
    }
    refresh_my_guilds(st, user_id).await
}

async fn upload(st: &AppState, user_id: i64, kind: &str, mut form: Multipart) -> AppResult<()> {
    let limit = if kind == "banner" {
        MAX_BANNER_BYTES
    } else {
        MAX_AVATAR_BYTES
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
        let Some(content_type) = image_type(&content_type, &filename) else {
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
        let key = auth::random_code(24, KEY_ALPHABET);
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
        crate::api::detector::track_upload(
            st,
            if kind == "banner" {
                crate::api::detector::KIND_BANNER
            } else {
                crate::api::detector::KIND_AVATAR
            },
            &key,
            content_type,
            user_id,
            None,
        )
        .await;
        return replace_image(st, user_id, kind, Some(key)).await;
    }
    Err(AppError::BadRequest("Choose a picture to upload.".into()))
}

/// Multipart form with one `file` field.
pub async fn upload_avatar(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    form: Multipart,
) -> AppResult<Json<User>> {
    upload(&st, user.id, "avatar", form).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}

pub async fn delete_avatar(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<User>> {
    replace_image(&st, user.id, "avatar", None).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}

pub async fn upload_banner(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    form: Multipart,
) -> AppResult<Json<User>> {
    upload(&st, user.id, "banner", form).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}

pub async fn delete_banner(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<User>> {
    replace_image(&st, user.id, "banner", None).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}

// ---- The full profile page ------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct NameRow {
    kind: String,
    guild_id: Option<i64>,
    guild_name: Option<String>,
    old_value: Option<String>,
    new_value: Option<String>,
    changed_at: DateTime<Utc>,
}

/// Names someone has gone by, newest first. Nicknames only from
/// `only_guilds` (None = every outhouse, for themselves and the site owner).
pub async fn name_history(
    db: &sqlx::PgPool,
    user_id: i64,
    only_guilds: Option<&[i64]>,
    limit: i64,
) -> AppResult<Vec<Value>> {
    let rows = sqlx::query_as::<_, NameRow>(
        "SELECT h.kind, h.guild_id, g.name AS guild_name, h.old_value, h.new_value, h.changed_at
         FROM name_history h LEFT JOIN guilds g ON g.id = h.guild_id
         WHERE h.user_id = $1
           AND (h.kind <> 'nickname' OR $2::BIGINT[] IS NULL OR h.guild_id = ANY($2))
         ORDER BY h.changed_at DESC, h.id DESC
         LIMIT $3",
    )
    .bind(user_id)
    .bind(only_guilds)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            json!({
                "kind": r.kind,
                "guild_id": r.guild_id.map(|g| g.to_string()),
                "guild_name": r.guild_name,
                "old": r.old_value,
                "new": r.new_value,
                "changed_at": r.changed_at,
            })
        })
        .collect())
}

/// Shown on the profile page (all of it is kept).
const NAMES_SHOWN: i64 = 20;

#[derive(sqlx::FromRow)]
struct FullRow {
    id: i64,
    username: String,
    display_name: String,
    avatar_key: Option<String>,
    banner_key: Option<String>,
    about_me: String,
    profile_theme: String,
    accent_color: i32,
    is_bot: bool,
    created_at: DateTime<Utc>,
    age_bracket: String,
    suspended: bool,
}

#[derive(sqlx::FromRow)]
struct SharedRow {
    guild_id: i64,
    name: String,
    joined_at: DateTime<Utc>,
    my_joined_at: DateTime<Utc>,
    nickname: Option<String>,
    owner: bool,
}

#[derive(sqlx::FromRow)]
struct RoleRow {
    guild_id: i64,
    id: i64,
    name: String,
    color: i32,
}

#[derive(sqlx::FromRow)]
struct PersonRow {
    id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    bot: bool,
}

/// `GET /api/users/{id}/profile/full`: everything about someone that you're
/// allowed to see, worked out here (the app shows what it's given).
///
/// - Yourself and the site owner: everything (every nickname, too).
/// - Friends and people in an outhouse with you: the full profile, names
///   they've gone by (nicknames only from outhouses you share), outhouses
///   and friends in common, roles, WoW characters.
/// - Anyone else linked to you (a friend in common, a DM or message
///   request, a friend request): the profile without name history.
/// - Teens, to adults who aren't their friend: name, picture and outhouses
///   in common only.
/// - Someone who has blocked you (or whom you've blocked): name and picture.
/// - Suspended accounts: "This account is suspended" (the owner sees all).
/// - Nobody else: not found.
pub async fn full(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(user_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let db = &st.db;
    let me = user.id;
    let row = sqlx::query_as::<_, FullRow>(
        "SELECT id, username, display_name, avatar_key, banner_key, about_me, profile_theme,
                accent_color, is_bot, created_at, age_bracket,
                COALESCE(suspended_until > now(), false) AS suspended
         FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    let is_self = user_id == me;
    let owner = super::audit::is_site_owner(db, me).await?;
    let friends = !is_self && super::friends::are_friends(db, me, user_id).await?;
    let shared = sqlx::query_as::<_, SharedRow>(
        "SELECT g.id AS guild_id, g.name, t.joined_at, v.joined_at AS my_joined_at, t.nickname,
                g.owner_id = t.user_id AS owner
         FROM guild_members t
         JOIN guild_members v ON v.guild_id = t.guild_id AND v.user_id = $2
         JOIN guilds g ON g.id = t.guild_id
         WHERE t.user_id = $1
         ORDER BY lower(g.name), g.id",
    )
    .bind(user_id)
    .bind(me)
    .fetch_all(db)
    .await?;
    let i_blocked = !is_self && super::friends::has_blocked(db, me, user_id).await;
    let blocked_me = !is_self && super::friends::has_blocked(db, user_id, me).await;
    let relationship: String = if is_self {
        "self".into()
    } else if i_blocked {
        "blocked".into()
    } else if friends {
        "friend".into()
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT CASE WHEN from_id = $1 THEN 'outgoing' ELSE 'incoming' END
             FROM friend_requests
             WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1) LIMIT 1",
        )
        .bind(me)
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .unwrap_or_else(|| "none".into())
    };
    let sql = format!(
        "WITH mine AS (
             SELECT CASE WHEN user_a = $1 THEN user_b ELSE user_a END AS f
             FROM friendships WHERE user_a = $1 OR user_b = $1),
         theirs AS (
             SELECT CASE WHEN user_a = $2 THEN user_b ELSE user_a END AS f
             FROM friendships WHERE user_a = $2 OR user_b = $2)
         SELECT {USER_COLUMNS} FROM users u
         WHERE u.id IN (SELECT f FROM mine JOIN theirs USING (f))
           AND NOT COALESCE(u.suspended_until > now(), false)
         ORDER BY lower(u.display_name), u.id LIMIT 100"
    );
    let in_common = if is_self {
        Vec::new()
    } else {
        sqlx::query_as::<_, PersonRow>(&sql)
            .bind(me)
            .bind(user_id)
            .fetch_all(db)
            .await?
    };
    let dm_linked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM channels WHERE dm_key = $1)
             OR EXISTS(SELECT 1 FROM dm_participants a JOIN dm_participants b
                       ON a.channel_id = b.channel_id WHERE a.user_id = $2 AND b.user_id = $3)",
    )
    .bind(format!("{}:{}", me.min(user_id), me.max(user_id)))
    .bind(me)
    .bind(user_id)
    .fetch_one(db)
    .await?;
    let full_view = is_self || owner || friends || !shared.is_empty();
    if !(full_view
        || i_blocked
        || blocked_me
        || dm_linked
        || relationship != "none"
        || !in_common.is_empty())
    {
        return Err(AppError::NotFound);
    }

    let avatar_url = row
        .avatar_key
        .as_ref()
        .map(|k| format!("/files/{k}/avatar"));
    let mut out = json!({
        "id": row.id.to_string(),
        "username": row.username,
        "display_name": row.display_name,
        "avatar_url": avatar_url,
        "bot": row.is_bot,
        "self": is_self,
        "site_owner_view": owner && !is_self,
        "relationship": relationship,
    });
    let obj = out.as_object_mut().expect("an object");

    // Suspended accounts (the owner and they themselves still see it all).
    if row.suspended && !owner && !is_self {
        obj.insert("level".into(), json!("suspended"));
        obj.insert("suspended".into(), json!(true));
        obj.insert("avatar_url".into(), Value::Null);
        return Ok(Json(out));
    }
    let contact = if is_self {
        Value::Null
    } else {
        let (reason, request) = super::requests::contact_status(&st, me, user_id).await?;
        json!({ "can_message": reason.is_none() && !row.is_bot, "reason": reason, "request": request })
    };
    let friend_refusal = if is_self || friends {
        None
    } else {
        super::friends::friend_request_refusal(db, me, user_id).await?
    };
    obj.insert("contact".into(), contact);
    obj.insert("friend_refusal".into(), json!(friend_refusal));
    obj.insert("suspended".into(), json!(row.suspended));

    // Blocked either way: just who they are.
    if !owner && (i_blocked || blocked_me) {
        obj.insert("level".into(), json!("minimal"));
        return Ok(Json(out));
    }
    let outhouses_basic: Vec<Value> = shared
        .iter()
        .map(|g| json!({ "id": g.guild_id.to_string(), "name": g.name }))
        .collect();

    // Teens, to adults who aren't their friend: name, picture, outhouses in common.
    let viewer_teen: bool =
        sqlx::query_scalar("SELECT age_bracket = 'teen' AND NOT is_bot FROM users WHERE id = $1")
            .bind(me)
            .fetch_one(db)
            .await?;
    if !is_self && !owner && !friends && row.age_bracket == "teen" && !row.is_bot && !viewer_teen {
        obj.insert("level".into(), json!("teen"));
        obj.insert("outhouses".into(), json!(outhouses_basic));
        return Ok(Json(out));
    }

    let online = st.is_online(row.id) && !blocked_me;
    obj.insert(
        "banner_url".into(),
        json!(row
            .banner_key
            .as_ref()
            .map(|k| format!("/files/{k}/banner"))),
    );
    obj.insert("about_me".into(), json!(row.about_me));
    obj.insert("profile_theme".into(), json!(row.profile_theme));
    obj.insert("accent_color".into(), json!(row.accent_color));
    obj.insert("created_at".into(), json!(row.created_at));
    obj.insert("online".into(), json!(online));
    obj.insert(
        "friends_in_common".into(),
        json!(in_common
            .iter()
            .map(|p| json!({
                "id": p.id.to_string(),
                "username": p.username,
                "display_name": p.display_name,
                "avatar_url": p.avatar_url,
                "bot": p.bot,
            }))
            .collect::<Vec<_>>()),
    );
    if !full_view {
        obj.insert("level".into(), json!("limited"));
        obj.insert("names".into(), Value::Null);
        obj.insert("outhouses".into(), json!(outhouses_basic));
        return Ok(Json(out));
    }

    obj.insert("level".into(), json!("full"));
    let guild_ids: Vec<i64> = shared.iter().map(|g| g.guild_id).collect();
    let roles = sqlx::query_as::<_, RoleRow>(
        "SELECT mr.guild_id, r.id, r.name, r.color FROM member_roles mr
         JOIN roles r ON r.id = mr.role_id
         WHERE mr.user_id = $1 AND mr.guild_id = ANY($2)
         ORDER BY r.position DESC, r.id",
    )
    .bind(user_id)
    .bind(&guild_ids)
    .fetch_all(db)
    .await?;
    obj.insert(
        "outhouses".into(),
        json!(shared
            .iter()
            .map(|g| json!({
                "id": g.guild_id.to_string(),
                "name": g.name,
                "joined_at": g.joined_at,
                "my_joined_at": g.my_joined_at,
                "nickname": g.nickname,
                "owner": g.owner,
                "roles": roles.iter().filter(|r| r.guild_id == g.guild_id).map(|r| json!({
                    "id": r.id.to_string(), "name": r.name, "color": r.color,
                })).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()),
    );
    let only = if is_self || owner {
        None
    } else {
        Some(guild_ids.as_slice())
    };
    let names = name_history(db, user_id, only, NAMES_SHOWN).await?;
    obj.insert("names".into(), json!(names));
    obj.insert(
        "characters".into(),
        json!(super::wow::characters_of(db, row.id).await?),
    );
    if owner && !is_self {
        let sent: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM dm_requests WHERE from_id = $1 AND sent_at IS NOT NULL",
        )
        .bind(row.id)
        .fetch_one(db)
        .await?;
        obj.insert("message_requests_sent".into(), json!(sent));
    }
    Ok(Json(out))
}
