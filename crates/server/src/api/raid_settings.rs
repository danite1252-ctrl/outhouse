//! Outhouse settings → Raids: which roles are raid managers, the channel
//! "Plan a raid" starts on, whether old sign-up cards are removed from
//! chat by default, and the realm's time zone and region (the clock widget's
//! "Realm time" and weekly reset).
//!
//! A raid manager runs every raid in the outhouse (moves people between
//! roster and bench, removes sign-ups, takes attendance, edits and cancels
//! raids, exports rosters) without needing Manage Events.

use axum::extract::{Path, State};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{channel_kind, RaidSettings};
use outhouse_common::permissions as p;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{member_ctx, MemberCtx};
use crate::state::{AppState, Audience};

/// Most roles that can be raid managers at once.
const MAX_MANAGER_ROLES: usize = 25;

/// The zone a region's realms are usually on when the outhouse hasn't
/// picked one: US realms show Pacific time (Oceanic realms are in the US
/// region too, so Oceanic outhouses should pick Australia/Sydney), EU realms
/// Central European time.
pub fn region_time_zone(region: &str) -> Option<&'static str> {
    match region {
        "us" => Some("America/Los_Angeles"),
        "eu" => Some("Europe/Paris"),
        "kr" => Some("Asia/Seoul"),
        "tw" => Some("Asia/Taipei"),
        _ => None,
    }
}

/// The zone and region the clock widget uses: the outhouse's own choices,
/// else its region's usual zone, else (Armory switched on) the server's
/// Armory region. `None` zone: no realm time to show.
fn effective_realm(
    st: &AppState,
    tz: &str,
    region: &str,
) -> (Option<String>, Option<String>, bool) {
    let region = if !region.is_empty() {
        Some(region.to_string())
    } else if st.config.bnet.is_some() {
        Some(st.config.armory_region.clone())
    } else {
        None
    };
    if !tz.is_empty() {
        return (Some(tz.to_string()), region, false);
    }
    match region.as_deref().and_then(region_time_zone) {
        Some(zone) => (Some(zone.to_string()), region, true),
        None => (None, region, false),
    }
}

pub async fn load(st: &AppState, guild_id: i64) -> AppResult<RaidSettings> {
    let db = &st.db;
    let manager_role_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT m.role_id FROM raid_manager_roles m JOIN roles r ON r.id = m.role_id
         WHERE m.guild_id = $1 ORDER BY r.position DESC, r.id",
    )
    .bind(guild_id)
    .fetch_all(db)
    .await?;
    let row: Option<(Option<i64>, String, String)> = sqlx::query_as(
        "SELECT default_channel_id, realm_time_zone, realm_region
         FROM raid_settings WHERE guild_id = $1",
    )
    .bind(guild_id)
    .fetch_optional(db)
    .await?;
    let (default_channel_id, realm_tz, realm_region) =
        row.unwrap_or((None, String::new(), String::new()));
    let (effective_tz, effective_region, derived) = effective_realm(st, &realm_tz, &realm_region);
    let (series, single) = super::events::card_cleanup_defaults(db, guild_id).await?;
    Ok(RaidSettings {
        manager_role_ids,
        default_channel_id,
        card_cleanup_series: Some(series),
        card_cleanup_single: Some(single),
        realm_time_zone: Some(realm_tz),
        realm_region: Some(realm_region),
        realm_effective_time_zone: effective_tz,
        realm_effective_region: effective_region,
        realm_time_zone_derived: derived,
    })
}

/// Has one of the outhouse's raid manager roles (timed-out members don't
/// count).
pub async fn has_manager_role(db: &PgPool, ctx: &MemberCtx) -> AppResult<bool> {
    if ctx.timed_out || ctx.role_ids.is_empty() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM raid_manager_roles WHERE guild_id = $1 AND role_id = ANY($2))",
    )
    .bind(ctx.guild_id)
    .bind(&ctx.role_ids)
    .fetch_one(db)
    .await?)
}

/// `GET /api/guilds/{id}/raid-settings` (any member: the client works out
/// who manages raids from it).
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<RaidSettings>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    Ok(Json(load(&st, guild_id).await?))
}

async fn role_names(db: &PgPool, ids: &[i64]) -> AppResult<String> {
    if ids.is_empty() {
        return Ok("nobody".into());
    }
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM roles WHERE id = ANY($1) ORDER BY position DESC, id")
            .bind(ids)
            .fetch_all(db)
            .await?;
    Ok(names.join(", "))
}

fn cleanup_label(v: &str) -> &'static str {
    match v {
        "after" => "3 hours after it ends",
        "next" => "when the next raid's sign-ups go up",
        _ => "keep it",
    }
}

async fn channel_label(db: &PgPool, id: Option<i64>) -> AppResult<String> {
    let Some(id) = id else {
        return Ok("the channel you're in".into());
    };
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM channels WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(format!(
        "#{}",
        name.unwrap_or_else(|| "deleted-channel".into())
    ))
}

/// `PUT /api/guilds/{id}/raid-settings` (needs Manage Outhouse).
pub async fn set(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<RaidSettings>,
) -> AppResult<Json<RaidSettings>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let mut role_ids = req.manager_role_ids.clone();
    role_ids.sort_unstable();
    role_ids.dedup();
    if role_ids.len() > MAX_MANAGER_ROLES {
        return Err(AppError::BadRequest(format!(
            "Up to {MAX_MANAGER_ROLES} raid manager roles."
        )));
    }
    if role_ids.contains(&guild_id) {
        return Err(AppError::BadRequest(
            "@everyone can't be a raid manager role. Make a role for your raid leaders instead."
                .into(),
        ));
    }
    let found: i64 =
        sqlx::query_scalar("SELECT count(*) FROM roles WHERE guild_id = $1 AND id = ANY($2)")
            .bind(guild_id)
            .bind(&role_ids)
            .fetch_one(&st.db)
            .await?;
    if found as usize != role_ids.len() {
        return Err(AppError::BadRequest(
            "One of those roles isn't in this outhouse.".into(),
        ));
    }
    if let Some(channel_id) = req.default_channel_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM channels WHERE id = $1 AND guild_id = $2 AND kind = $3)",
        )
        .bind(channel_id)
        .bind(guild_id)
        .bind(channel_kind::TEXT)
        .fetch_one(&st.db)
        .await?;
        if !ok {
            return Err(AppError::BadRequest(
                "Raid sign-ups go in one of this outhouse's text channels.".into(),
            ));
        }
    }

    let before = load(&st, guild_id).await?;
    // Left out: unchanged.
    let series_cleanup = match req.card_cleanup_series.as_deref().map(str::trim) {
        None => before
            .card_cleanup_series
            .clone()
            .unwrap_or_else(|| "next".into()),
        Some(v @ ("off" | "after" | "next")) => v.to_string(),
        Some(_) => {
            return Err(AppError::BadRequest(
                "Unknown choice for old repeating raid cards.".into(),
            ))
        }
    };
    let single_cleanup = match req.card_cleanup_single.as_deref().map(str::trim) {
        None => before
            .card_cleanup_single
            .clone()
            .unwrap_or_else(|| "off".into()),
        Some(v @ ("off" | "after")) => v.to_string(),
        Some(_) => {
            return Err(AppError::BadRequest(
                "One-off raids can keep their card or have it removed 3 hours after the raid."
                    .into(),
            ))
        }
    };
    let realm_tz = match req.realm_time_zone.as_deref().map(str::trim) {
        None => before.realm_time_zone.clone().unwrap_or_default(),
        Some("") => String::new(),
        Some(raw) => super::events::check_time_zone(&st.db, raw)
            .await
            .map_err(|_| {
                AppError::BadRequest(
                    "Pick a time zone from the list (like Australia/Sydney).".into(),
                )
            })?,
    };
    let realm_region = match req
        .realm_region
        .as_deref()
        .map(|r| r.trim().to_ascii_lowercase())
    {
        None => before.realm_region.clone().unwrap_or_default(),
        Some(r) if r.is_empty() || outhouse_common::wow::REGIONS.contains(&r.as_str()) => r,
        Some(_) => {
            return Err(AppError::BadRequest(
                "The realm region is US, EU, KR or TW (or automatic).".into(),
            ))
        }
    };
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM raid_manager_roles WHERE guild_id = $1")
        .bind(guild_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO raid_manager_roles (guild_id, role_id) SELECT $1, unnest($2::BIGINT[])",
    )
    .bind(guild_id)
    .bind(&role_ids)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO raid_settings (guild_id, default_channel_id, card_cleanup_series,
                                    card_cleanup_single, realm_time_zone, realm_region)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (guild_id) DO UPDATE
             SET default_channel_id = EXCLUDED.default_channel_id,
                 card_cleanup_series = EXCLUDED.card_cleanup_series,
                 card_cleanup_single = EXCLUDED.card_cleanup_single,
                 realm_time_zone = EXCLUDED.realm_time_zone,
                 realm_region = EXCLUDED.realm_region",
    )
    .bind(guild_id)
    .bind(req.default_channel_id)
    .bind(&series_cleanup)
    .bind(&single_cleanup)
    .bind(&realm_tz)
    .bind(&realm_region)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let after = load(&st, guild_id).await?;

    let mut details = Vec::new();
    let (mut old_roles, mut new_roles) = (
        before.manager_role_ids.clone(),
        after.manager_role_ids.clone(),
    );
    old_roles.sort_unstable();
    new_roles.sort_unstable();
    if old_roles != new_roles {
        details.push(format!(
            "Raid managers: {} → {}",
            role_names(&st.db, &before.manager_role_ids).await?,
            role_names(&st.db, &after.manager_role_ids).await?
        ));
    }
    if before.default_channel_id != after.default_channel_id {
        details.push(format!(
            "Sign-up channel: {} → {}",
            channel_label(&st.db, before.default_channel_id).await?,
            channel_label(&st.db, after.default_channel_id).await?
        ));
    }
    for (what, old, new) in [
        (
            "Old repeating raid cards",
            &before.card_cleanup_series,
            &after.card_cleanup_series,
        ),
        (
            "Old one-off raid cards",
            &before.card_cleanup_single,
            &after.card_cleanup_single,
        ),
    ] {
        if old != new {
            details.push(format!(
                "{what}: {} → {}",
                cleanup_label(old.as_deref().unwrap_or("")),
                cleanup_label(new.as_deref().unwrap_or(""))
            ));
        }
    }
    let region_label = |v: &Option<String>| match v.as_deref() {
        None | Some("") => "automatic".to_string(),
        Some(x) => x.to_uppercase(),
    };
    let auto = |v: &Option<String>| match v.as_deref() {
        None | Some("") => "automatic".to_string(),
        Some(x) => x.to_string(),
    };
    if before.realm_time_zone != after.realm_time_zone {
        details.push(format!(
            "Realm time zone: {} → {}",
            auto(&before.realm_time_zone),
            auto(&after.realm_time_zone)
        ));
    }
    if before.realm_region != after.realm_region {
        details.push(format!(
            "Realm region: {} → {}",
            region_label(&before.realm_region),
            region_label(&after.realm_region)
        ));
    }
    if !details.is_empty() {
        crate::audit::Entry::new(guild_id, user.id, "raid_settings_update")
            .details(details.join(" · "))
            .save(&st)
            .await;
    }
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::RaidSettingsUpdate {
            guild_id,
            settings: after.clone(),
        },
    );
    Ok(Json(after))
}
