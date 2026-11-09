//! Soft reserves and the loot log.
//!
//! A raid leader turns soft reserves on in Plan/Edit raid (1 or 2 per
//! person) and picks the raid's loot list (a listed raid, with a few famous
//! items to start with, or "Other"). People signed up reserve items until
//! reserves lock (at the sign-up deadline, at the start, or by hand), and
//! see who else wants the same thing. Raid leaders see everything, lock and
//! unlock, remove reserves, add items to the list (by Wowhead link, ID or
//! name), export the reserves and log who won what. The loot log shows on
//! profile cards and as an outhouse-wide list.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::models::EventSignup;
use outhouse_common::permissions as p;
use outhouse_common::raidplan::{
    self, AwardRequest, ItemLookup, LootAward, LootInstance, LootItem, LootItemRequest, LootTable,
    RaidReserve, ReserveLockRequest, ReserveRequest,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::events::{broadcast, build, can_manage, load, EventReply, EventRow};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{drop_age_gated, member_ctx, visible_text_channels};
use crate::state::AppState;

/// Items a raid leader can add to one raid's list.
const MAX_CUSTOM_ITEMS: i64 = 300;
const MAX_QUERY_CHARS: usize = 200;
const MAX_NOTE_CHARS: usize = 100;

// ---- Reserves on raids ---------------------------------------------------------

/// Everyone's reserves on these raids (for the raid cards).
pub(crate) async fn reserves_for(
    db: &PgPool,
    event_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<RaidReserve>>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        event_id: i64,
        user_id: i64,
        item_id: i32,
        item_name: String,
        item_quality: i32,
        created_at: DateTime<Utc>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT event_id, user_id, item_id, item_name, item_quality, created_at
         FROM loot_reserves WHERE event_id = ANY($1)
         ORDER BY created_at, user_id, item_id",
    )
    .bind(event_ids)
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<RaidReserve>> = HashMap::new();
    for r in rows {
        map.entry(r.event_id).or_default().push(RaidReserve {
            user_id: r.user_id,
            item_id: r.item_id,
            item_name: r.item_name,
            quality: r.item_quality,
            reserved_at: r.created_at,
        });
    }
    Ok(map)
}

/// The loot list's key for a raid (see [`raidplan::instance_key`]).
fn table_key(row: &EventRow) -> String {
    raidplan::instance_key(&row.loot_instance)
}

/// The built-in items for a raid plus those this outhouse added.
async fn table_items(db: &PgPool, guild_id: i64, key: &str) -> AppResult<Vec<LootItem>> {
    let mut items: Vec<LootItem> = raidplan::SEED_ITEMS
        .iter()
        .filter(|s| !key.is_empty() && s.0 == key)
        .map(|&(_, item_id, name, quality, boss)| LootItem {
            item_id,
            name: name.into(),
            quality,
            boss: boss.into(),
            custom: false,
        })
        .collect();
    if key.is_empty() {
        return Ok(items);
    }
    let added: Vec<(i32, String, i32, String)> = sqlx::query_as(
        "SELECT item_id, name, quality, boss FROM loot_items
         WHERE guild_id = $1 AND instance = $2 ORDER BY created_at, item_id",
    )
    .bind(guild_id)
    .bind(key)
    .fetch_all(db)
    .await?;
    for (item_id, name, quality, boss) in added {
        if items.iter().any(|i| i.item_id == item_id) {
            continue;
        }
        items.push(LootItem {
            item_id,
            name,
            quality,
            boss,
            custom: true,
        });
    }
    Ok(items)
}

async fn loot_table(st: &AppState, row: &EventRow) -> AppResult<LootTable> {
    let key = table_key(row);
    let mut items = table_items(&st.db, row.guild_id, &key).await?;
    // Things people reserved that aren't on the list (looked up by name).
    let reserved: Vec<(i32, String, i32)> = sqlx::query_as(
        "SELECT DISTINCT ON (item_id) item_id, item_name, item_quality FROM loot_reserves
         WHERE event_id = $1 ORDER BY item_id, created_at",
    )
    .bind(row.id)
    .fetch_all(&st.db)
    .await?;
    for (item_id, name, quality) in reserved {
        if !items.iter().any(|i| i.item_id == item_id) {
            items.push(LootItem {
                item_id,
                name,
                quality,
                boss: String::new(),
                custom: false,
            });
        }
    }
    Ok(LootTable {
        instance_name: raidplan::instance_name(&row.loot_instance),
        instance: row.loot_instance.clone(),
        items,
    })
}

/// `GET /api/events/{id}/loot-table`
pub async fn get_table(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<LootTable>> {
    let (row, _) = load(&st, event_id, user.id).await?;
    Ok(Json(loot_table(&st, &row).await?))
}

// ---- Finding items ---------------------------------------------------------------

fn item_lookup(st: &AppState, item: &crate::wowhead::ItemInfo) -> ItemLookup {
    let cfg = &st.config;
    ItemLookup {
        item_id: item.item_id,
        name: item.name.clone(),
        quality: item.quality,
        icon_url: if item.icon.is_empty() {
            String::new()
        } else {
            format!("{}/{}.jpg", cfg.wowhead_icon_url, item.icon)
        },
        tooltip: item.tooltip.clone(),
        url: crate::wowhead::item_url(cfg, item.item_id),
    }
}

/// Looks an item up on Wowhead (cached), then WoW: Forever's own item data
/// on Battle.net when the keys are set.
async fn look_up(st: &AppState, query: &str) -> Option<ItemLookup> {
    match tokio::time::timeout(Duration::from_secs(9), crate::wowhead::lookup(st, query)).await {
        Ok(Ok(Some(item))) => return Some(item_lookup(st, &item)),
        Ok(Err(e)) => tracing::info!(error = %e, "item lookup failed"),
        _ => {}
    }
    st.config.bnet.as_ref()?;
    let embed = tokio::time::timeout(Duration::from_secs(6), crate::armory::item_embed(st, query))
        .await
        .ok()??;
    let item_id: i32 = embed
        .footer
        .as_deref()?
        .strip_prefix("Item ")?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    let colour = embed.color.unwrap_or(0xffffff);
    let quality = (0..=7)
        .find(|&q| crate::wowhead::quality_colour(q) == colour)
        .unwrap_or(1);
    Some(ItemLookup {
        item_id,
        name: embed.title.clone()?,
        quality,
        icon_url: embed.thumbnail_url.clone().unwrap_or_default(),
        tooltip: embed.description.clone().unwrap_or_default(),
        url: embed.url.clone(),
    })
}

fn clean_query(raw: &str) -> AppResult<String> {
    let q: String = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect();
    if q.is_empty() {
        return Err(AppError::BadRequest("Which item?".into()));
    }
    Ok(q)
}

const NOT_FOUND_ITEM: &str = "Couldn't find that item. Try its Wowhead link or item ID.";

/// The item a request means: from the raid's list (by ID or name), else
/// looked up.
async fn resolve_item(
    st: &AppState,
    row: &EventRow,
    item_id: Option<i32>,
    query: Option<&str>,
) -> AppResult<(i32, String, i32)> {
    let items = table_items(&st.db, row.guild_id, &table_key(row)).await?;
    let query = match query {
        Some(q) => Some(clean_query(q)?),
        None => None,
    };
    let id = item_id.or_else(|| query.as_deref().and_then(crate::wowhead::item_id_in));
    if let Some(id) = id {
        if !(1..=10_000_000).contains(&id) {
            return Err(AppError::BadRequest(NOT_FOUND_ITEM.into()));
        }
        if let Some(i) = items.iter().find(|i| i.item_id == id) {
            return Ok((i.item_id, i.name.clone(), i.quality));
        }
        // Someone already reserved it, or another raid's list has it.
        let known: Option<(String, i32)> = sqlx::query_as(
            "SELECT item_name, item_quality FROM loot_reserves WHERE item_id = $1
             UNION ALL
             SELECT name, quality FROM loot_items WHERE item_id = $1 AND guild_id = $2
             LIMIT 1",
        )
        .bind(id)
        .bind(row.guild_id)
        .fetch_optional(&st.db)
        .await?;
        if let Some((name, quality)) = known {
            return Ok((id, name, quality));
        }
        if let Some((_, _, name, quality, _)) =
            raidplan::SEED_ITEMS.iter().find(|s| s.1 == id).copied()
        {
            return Ok((id, name.into(), quality));
        }
        return match look_up(st, &id.to_string()).await {
            Some(found) => Ok((found.item_id, found.name, found.quality)),
            None => Err(AppError::BadRequest(NOT_FOUND_ITEM.into())),
        };
    }
    let Some(q) = query else {
        return Err(AppError::BadRequest("Which item?".into()));
    };
    if let Some(i) = items.iter().find(|i| i.name.eq_ignore_ascii_case(&q)) {
        return Ok((i.item_id, i.name.clone(), i.quality));
    }
    match look_up(st, &q).await {
        Some(found) => Ok((found.item_id, found.name, found.quality)),
        None => Err(AppError::BadRequest(NOT_FOUND_ITEM.into())),
    }
}

#[derive(Deserialize)]
pub struct ItemQuery {
    q: Option<String>,
}

/// `GET /api/wow/items?q=` (ID, Wowhead link or name): an item's name,
/// quality, icon and tooltip for item pickers. 404 if nothing matched.
pub async fn search_item(
    State(st): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(q): Query<ItemQuery>,
) -> AppResult<Json<ItemLookup>> {
    let query = clean_query(q.q.as_deref().unwrap_or(""))?;
    look_up(&st, &query)
        .await
        .map(Json)
        .ok_or(AppError::NotFound)
}

/// `GET /api/wow/items/{id}`: one item (tooltips).
pub async fn get_item(
    State(st): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(item_id): Path<i32>,
) -> AppResult<Json<ItemLookup>> {
    if !(1..=10_000_000).contains(&item_id) {
        return Err(AppError::NotFound);
    }
    if let Ok(Ok(Some(item))) = tokio::time::timeout(
        Duration::from_secs(9),
        crate::wowhead::item_by_id(&st, item_id),
    )
    .await
    {
        return Ok(Json(item_lookup(&st, &item)));
    }
    // Not on Wowhead: what we know from the lists.
    if let Some(s) = raidplan::SEED_ITEMS.iter().find(|s| s.1 == item_id) {
        return Ok(Json(ItemLookup {
            item_id,
            name: s.2.into(),
            quality: s.3,
            icon_url: String::new(),
            tooltip: String::new(),
            url: crate::wowhead::item_url(&st.config, item_id),
        }));
    }
    look_up(&st, &item_id.to_string())
        .await
        .map(Json)
        .ok_or(AppError::NotFound)
}

// ---- Reserving ---------------------------------------------------------------------

async fn require_manager(
    st: &AppState,
    row: &EventRow,
    access: &crate::perms::ChannelAccess,
) -> AppResult<()> {
    if can_manage(&st.db, row, access).await? {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn signup_of(signups: &[EventSignup], user_id: i64) -> Option<&EventSignup> {
    signups.iter().find(|s| s.user_id == user_id)
}

fn person(s: Option<&EventSignup>, fallback: &str) -> String {
    match s {
        Some(s) => s
            .character
            .as_ref()
            .map(|c| c.full_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| s.display_name.clone()),
        None => fallback.to_string(),
    }
}

async fn display_name(db: &PgPool, guild_id: i64, user_id: i64) -> AppResult<String> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(gm.nickname, u.display_name) FROM users u
         LEFT JOIN guild_members gm ON gm.guild_id = $1 AND gm.user_id = u.id
         WHERE u.id = $2",
    )
    .bind(guild_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .unwrap_or_else(|| "someone".into()))
}

/// `PUT /api/events/{id}/reserves` `{item_id}` or `{query}`: reserve an
/// item for yourself (you must be signed up; up to the raid's limit).
pub async fn reserve(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<ReserveRequest>,
) -> AppResult<Json<EventReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if row.soft_reserves <= 0 {
        return Err(AppError::BadRequest(
            "Soft reserves are off for this raid.".into(),
        ));
    }
    if access.ctx.timed_out {
        return Err(AppError::Forbidden);
    }
    let manager = can_manage(&st.db, &row, &access).await?;
    if !manager && row.not_open_yet() {
        return Err(AppError::BadRequest(
            "Sign-ups for this raid aren't open yet, so neither are reserves.".into(),
        ));
    }
    if !manager && row.reserves_locked() {
        return Err(AppError::BadRequest(
            "Soft reserves for this raid are locked. Ask the raid leader if you need a change."
                .into(),
        ));
    }
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM event_signups WHERE event_id = $1 AND user_id = $2")
            .bind(event_id)
            .bind(user.id)
            .fetch_optional(&st.db)
            .await?;
    if !status.is_some_and(|s| s != "no") {
        return Err(AppError::BadRequest(
            "Sign up for the raid first, then pick what you'd like to reserve.".into(),
        ));
    }
    let (item_id, name, quality) =
        resolve_item(&st, &row, req.item_id, req.query.as_deref()).await?;

    let mut tx = st.db.begin().await?;
    // One at a time per raid, so two quick clicks can't go over the limit.
    sqlx::query("SELECT id FROM events WHERE id = $1 FOR UPDATE")
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
    let mine: Vec<i32> = sqlx::query_scalar(
        "SELECT item_id FROM loot_reserves WHERE event_id = $1 AND user_id = $2",
    )
    .bind(event_id)
    .bind(user.id)
    .fetch_all(&mut *tx)
    .await?;
    if mine.contains(&item_id) {
        return Err(AppError::BadRequest(format!(
            "You've already reserved {name}."
        )));
    }
    if mine.len() as i32 >= row.soft_reserves {
        return Err(AppError::BadRequest(if row.soft_reserves == 1 {
            "You've used your soft reserve. Remove it to pick something else.".into()
        } else {
            format!(
                "You've used all {} of your soft reserves. Remove one to pick something else.",
                row.soft_reserves
            )
        }));
    }
    sqlx::query(
        "INSERT INTO loot_reserves (event_id, user_id, item_id, item_name, item_quality)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(event_id)
    .bind(user.id)
    .bind(item_id)
    .bind(&name)
    .bind(quality)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(broadcast(&st, event_id).await?))
}

#[derive(Deserialize)]
pub struct UserQuery {
    user_id: Option<String>,
}

/// `DELETE /api/events/{id}/reserves/{item}[?user_id=]`: take back your
/// reserve (raid leaders: anyone's).
pub async fn unreserve(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((event_id, item_id)): Path<(i64, i32)>,
    Query(q): Query<UserQuery>,
) -> AppResult<Json<EventReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    let target = match q.user_id.as_deref().filter(|s| !s.is_empty()) {
        Some(raw) => raw
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest("Unknown user.".into()))?,
        None => user.id,
    };
    let manager = can_manage(&st.db, &row, &access).await?;
    if target != user.id && !manager {
        return Err(AppError::Forbidden);
    }
    if !manager && (access.ctx.timed_out || row.reserves_locked()) {
        return Err(AppError::BadRequest(
            "Soft reserves for this raid are locked. Ask the raid leader if you need a change."
                .into(),
        ));
    }
    let gone: Option<String> = sqlx::query_scalar(
        "DELETE FROM loot_reserves WHERE event_id = $1 AND user_id = $2 AND item_id = $3
         RETURNING item_name",
    )
    .bind(event_id)
    .bind(target)
    .bind(item_id)
    .fetch_optional(&st.db)
    .await?;
    let Some(item_name) = gone else {
        return Err(AppError::NotFound);
    };
    if target != user.id {
        let who = display_name(&st.db, row.guild_id, target).await?;
        crate::audit::Entry::new(row.guild_id, user.id, "loot_reserve_remove")
            .target("user", target, who.clone())
            .details(format!(
                "Removed {who}'s soft reserve on {item_name} ({})",
                row.title
            ))
            .save(&st)
            .await;
    }
    Ok(Json(broadcast(&st, event_id).await?))
}

/// `PUT /api/events/{id}/reserves/lock` `{locked: true | false | null}`
/// (raid leaders).
pub async fn set_lock(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<ReserveLockRequest>,
) -> AppResult<Json<EventReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    require_manager(&st, &row, &access).await?;
    sqlx::query("UPDATE events SET reserve_lock_override = $2 WHERE id = $1")
        .bind(event_id)
        .bind(req.locked)
        .execute(&st.db)
        .await?;
    let what = match req.locked {
        Some(true) => "Locked",
        Some(false) => "Unlocked",
        None => "Automatic lock for",
    };
    crate::audit::Entry::new(row.guild_id, user.id, "loot_reserves_lock")
        .details(format!("{what} soft reserves: {}", row.title))
        .save(&st)
        .await;
    Ok(Json(broadcast(&st, event_id).await?))
}

/// Puts a CSV field in quotes when it needs them, and stops spreadsheets
/// treating it as a formula.
fn csv_field(raw: &str) -> String {
    let mut v = raw.replace(['\r', '\n'], " ");
    if v.starts_with(['=', '+', '-', '@', '\t']) {
        v.insert(0, '\'');
    }
    if v.contains([',', '"']) || v.starts_with(' ') || v.ends_with(' ') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

#[derive(Serialize)]
pub struct ReserveExport {
    /// Item ID, Item, Player, Character, Class, Spec, Reserved at.
    pub csv: String,
    /// "Item — Player1, Player2", one item per line.
    pub text: String,
}

/// `GET /api/events/{id}/reserves/export` (raid leaders).
pub async fn export(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<ReserveExport>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    require_manager(&st, &row, &access).await?;
    let ev = build(&st.db, vec![row])
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    let mut csv = String::from("Item ID,Item,Player,Character,Class,Spec,Reserved at\n");
    let mut by_item: Vec<(i32, String, Vec<String>)> = Vec::new();
    for r in &ev.reserves {
        let s = signup_of(&ev.signups, r.user_id);
        let player = s.map(|s| s.display_name.clone()).unwrap_or_default();
        let character = s
            .and_then(|s| s.character.as_ref())
            .map(|c| c.full_name.clone())
            .unwrap_or_default();
        csv.push_str(
            &[
                r.item_id.to_string(),
                csv_field(&r.item_name),
                csv_field(&player),
                csv_field(&character),
                csv_field(s.map(|s| s.class.as_str()).unwrap_or("")),
                csv_field(s.map(|s| s.spec.as_str()).unwrap_or("")),
                r.reserved_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            ]
            .join(","),
        );
        csv.push('\n');
        let name = person(s, &player);
        match by_item.iter_mut().find(|i| i.0 == r.item_id) {
            Some(i) => i.2.push(name),
            None => by_item.push((r.item_id, r.item_name.clone(), vec![name])),
        }
    }
    by_item.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    let text = by_item
        .iter()
        .map(|(_, item, names)| format!("{item} — {}", names.join(", ")))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Json(ReserveExport { csv, text }))
}

// ---- The raid's loot list ---------------------------------------------------------------

/// `POST /api/events/{id}/loot-items` `{query}` (raid leaders): add an item
/// to this raid's loot list for the whole outhouse.
pub async fn add_item(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<LootItemRequest>,
) -> AppResult<Json<LootTable>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    require_manager(&st, &row, &access).await?;
    let key = table_key(&row);
    if key.is_empty() {
        return Err(AppError::BadRequest(
            "Pick which raid this is (Edit raid → Loot list) before adding items.".into(),
        ));
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM loot_items WHERE guild_id = $1 AND instance = $2")
            .bind(row.guild_id)
            .bind(&key)
            .fetch_one(&st.db)
            .await?;
    if count >= MAX_CUSTOM_ITEMS {
        return Err(AppError::BadRequest(format!(
            "A raid's list can have up to {MAX_CUSTOM_ITEMS} added items."
        )));
    }
    let query = clean_query(&req.query)?;
    let found = match crate::wowhead::item_id_in(&query) {
        Some(id) if raidplan::SEED_ITEMS.iter().any(|s| s.0 == key && s.1 == id) => {
            return Err(AppError::BadRequest("That's already on the list.".into()))
        }
        _ => look_up(&st, &query).await,
    };
    let Some(item) = found else {
        return Err(AppError::BadRequest(NOT_FOUND_ITEM.into()));
    };
    if raidplan::SEED_ITEMS
        .iter()
        .any(|s| s.0 == key && s.1 == item.item_id)
    {
        return Err(AppError::BadRequest("That's already on the list.".into()));
    }
    let added = sqlx::query(
        "INSERT INTO loot_items (guild_id, instance, item_id, name, quality, added_by)
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(row.guild_id)
    .bind(&key)
    .bind(item.item_id)
    .bind(&item.name)
    .bind(item.quality)
    .bind(user.id)
    .execute(&st.db)
    .await?
    .rows_affected();
    if added == 0 {
        return Err(AppError::BadRequest("That's already on the list.".into()));
    }
    crate::audit::Entry::new(row.guild_id, user.id, "loot_item_add")
        .details(format!(
            "Added {} to the {} loot list",
            item.name,
            raidplan::instance_name(&row.loot_instance)
        ))
        .save(&st)
        .await;
    Ok(Json(loot_table(&st, &row).await?))
}

/// `DELETE /api/events/{id}/loot-items/{item}` (raid leaders): only items
/// someone added (the built-in ones stay).
pub async fn remove_item(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((event_id, item_id)): Path<(i64, i32)>,
) -> AppResult<Json<LootTable>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    require_manager(&st, &row, &access).await?;
    let gone: Option<String> = sqlx::query_scalar(
        "DELETE FROM loot_items WHERE guild_id = $1 AND instance = $2 AND item_id = $3
         RETURNING name",
    )
    .bind(row.guild_id)
    .bind(table_key(&row))
    .bind(item_id)
    .fetch_optional(&st.db)
    .await?;
    let Some(name) = gone else {
        return Err(AppError::NotFound);
    };
    crate::audit::Entry::new(row.guild_id, user.id, "loot_item_remove")
        .details(format!(
            "Removed {name} from the {} loot list",
            raidplan::instance_name(&row.loot_instance)
        ))
        .save(&st)
        .await;
    Ok(Json(loot_table(&st, &row).await?))
}

/// `GET /api/guilds/{id}/loot-instances`: the listed raids plus any of the
/// outhouse's own (from Plan/Edit raid's "Other").
pub async fn instances(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<LootInstance>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let mut list = raidplan::builtin_instances();
    let own: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM (
             SELECT DISTINCT ON (lower(loot_instance)) loot_instance AS name, starts_at
             FROM events WHERE guild_id = $1 AND loot_instance <> ''
             ORDER BY lower(loot_instance), starts_at DESC
         ) x ORDER BY starts_at DESC LIMIT 30",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    for name in own {
        if raidplan::instance(&name).is_some()
            || list.iter().any(|i| i.name.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        list.push(LootInstance {
            key: name.clone(),
            name,
            era: String::new(),
            size: 0,
            builtin: false,
        });
    }
    Ok(Json(list))
}

// ---- The loot log ---------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct AwardRow {
    id: i64,
    event_id: Option<i64>,
    channel_id: i64,
    raid_title: String,
    raid_starts_at: DateTime<Utc>,
    loot_instance: String,
    user_id: i64,
    display_name: String,
    character_name: String,
    item_id: i32,
    item_name: String,
    item_quality: i32,
    reserved: bool,
    note: String,
    awarded_by: Option<i64>,
    awarded_at: DateTime<Utc>,
}

impl From<AwardRow> for LootAward {
    fn from(r: AwardRow) -> Self {
        LootAward {
            id: r.id,
            event_id: r.event_id,
            channel_id: r.channel_id,
            raid_title: r.raid_title,
            raid_starts_at: r.raid_starts_at,
            instance_name: raidplan::instance_name(&r.loot_instance),
            loot_instance: r.loot_instance,
            user_id: r.user_id,
            display_name: r.display_name,
            character_name: r.character_name,
            item_id: r.item_id,
            item_name: r.item_name,
            quality: r.item_quality,
            reserved: r.reserved,
            note: r.note,
            awarded_by: r.awarded_by,
            awarded_at: r.awarded_at,
        }
    }
}

const AWARD_SELECT: &str = "SELECT a.id, a.event_id, a.channel_id, a.raid_title, a.raid_starts_at,
        a.loot_instance, a.user_id, COALESCE(gm.nickname, u.display_name) AS display_name,
        a.character_name, a.item_id, a.item_name, a.item_quality, a.reserved, a.note,
        a.awarded_by, a.awarded_at
    FROM loot_awards a
    JOIN users u ON u.id = a.user_id
    LEFT JOIN guild_members gm ON gm.guild_id = a.guild_id AND gm.user_id = a.user_id";

/// `GET /api/events/{id}/loot`: what's been handed out in this raid.
pub async fn raid_awards(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<Vec<LootAward>>> {
    load(&st, event_id, user.id).await?;
    let sql = format!("{AWARD_SELECT} WHERE a.event_id = $1 ORDER BY a.awarded_at, a.id");
    let rows = sqlx::query_as::<_, AwardRow>(&sql)
        .bind(event_id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.into_iter().map(LootAward::from).collect()))
}

/// `POST /api/events/{id}/loot` `{user_id, item_id | query, note?}` (raid
/// leaders, once the raid has started): someone won an item.
pub async fn award(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<AwardRequest>,
) -> AppResult<Json<LootAward>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    require_manager(&st, &row, &access).await?;
    if row.starts_at > Utc::now() {
        return Err(AppError::BadRequest(
            "You can hand out loot once the raid has started.".into(),
        ));
    }
    // The winner has to be in the outhouse.
    member_ctx(&st.db, row.guild_id, req.user_id)
        .await
        .map_err(|_| AppError::BadRequest("They aren't in this outhouse.".into()))?;
    let (item_id, item_name, quality) =
        resolve_item(&st, &row, req.item_id, req.query.as_deref()).await?;
    let note: String = req.note.trim().chars().take(MAX_NOTE_CHARS).collect();
    let (guild_id, channel_id, title, starts_at, instance) = (
        row.guild_id,
        row.channel_id,
        row.title.clone(),
        row.starts_at,
        row.loot_instance.clone(),
    );
    let ev = build(&st.db, vec![row])
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    let signup = signup_of(&ev.signups, req.user_id);
    let character = signup
        .and_then(|s| s.character.as_ref())
        .map(|c| c.full_name.clone())
        .unwrap_or_default();
    let reserved = ev
        .reserves
        .iter()
        .any(|r| r.user_id == req.user_id && r.item_id == item_id);
    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO loot_awards (id, guild_id, event_id, channel_id, raid_title, raid_starts_at,
             loot_instance, user_id, character_name, item_id, item_name, item_quality, reserved,
             note, awarded_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(id)
    .bind(guild_id)
    .bind(event_id)
    .bind(channel_id)
    .bind(&title)
    .bind(starts_at)
    .bind(&instance)
    .bind(req.user_id)
    .bind(&character)
    .bind(item_id)
    .bind(&item_name)
    .bind(quality)
    .bind(reserved)
    .bind(&note)
    .bind(user.id)
    .execute(&st.db)
    .await?;
    let who = display_name(&st.db, guild_id, req.user_id).await?;
    crate::audit::Entry::new(guild_id, user.id, "loot_award")
        .target("user", req.user_id, who.clone())
        .details(format!("{item_name} → {who} ({title})"))
        .save(&st)
        .await;
    broadcast(&st, event_id).await?;
    let sql = format!("{AWARD_SELECT} WHERE a.id = $1");
    let row = sqlx::query_as::<_, AwardRow>(&sql)
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    Ok(Json(row.into()))
}

/// `DELETE /api/loot/{id}`: take an entry out of the loot log (the raid's
/// leaders; Manage Events if the raid's gone).
pub async fn remove_award(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(award_id): Path<i64>,
) -> AppResult<StatusCode> {
    let found: Option<(i64, Option<i64>, String, String, i64)> = sqlx::query_as(
        "SELECT guild_id, event_id, item_name, raid_title, user_id FROM loot_awards WHERE id = $1",
    )
    .bind(award_id)
    .fetch_optional(&st.db)
    .await?;
    let Some((guild_id, event_id, item_name, title, winner)) = found else {
        return Err(AppError::NotFound);
    };
    match event_id {
        Some(event_id) => {
            let (row, access) = load(&st, event_id, user.id).await?;
            require_manager(&st, &row, &access).await?;
        }
        None => member_ctx(&st.db, guild_id, user.id)
            .await?
            .require(p::MANAGE_EVENTS)?,
    }
    sqlx::query("DELETE FROM loot_awards WHERE id = $1")
        .bind(award_id)
        .execute(&st.db)
        .await?;
    let who = display_name(&st.db, guild_id, winner).await?;
    crate::audit::Entry::new(guild_id, user.id, "loot_award_remove")
        .target("user", winner, who.clone())
        .details(format!("{item_name} → {who} ({title})"))
        .save(&st)
        .await;
    if let Some(event_id) = event_id {
        broadcast(&st, event_id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct LogQuery {
    user: Option<String>,
    q: Option<String>,
    instance: Option<String>,
    before: Option<String>,
    limit: Option<i64>,
}

#[derive(Serialize)]
pub struct LootLog {
    pub entries: Vec<LootAward>,
    pub has_more: bool,
}

/// `GET /api/guilds/{id}/loot?user=&q=&instance=&before=&limit=`: the
/// outhouse's loot log, newest first, from raids in channels you can see.
pub async fn guild_log(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(q): Query<LogQuery>,
) -> AppResult<Json<LootLog>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let mut channels = visible_text_channels(&st.db, &ctx, p::VIEW_CHANNEL).await?;
    drop_age_gated(&st.db, user.id, &mut channels).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let parse = |v: &Option<String>| -> AppResult<Option<i64>> {
        match v.as_deref().filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => s
                .parse()
                .map(Some)
                .map_err(|_| AppError::BadRequest("Bad ID.".into())),
        }
    };
    let who = parse(&q.user)?;
    let before = parse(&q.before)?;
    let text =
        q.q.as_deref()
            .map(|t| t.trim().chars().take(100).collect::<String>())
            .filter(|t| !t.is_empty())
            .map(|t| {
                format!(
                    "%{}%",
                    t.replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
            });
    let instance = q
        .instance
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(60).collect::<String>());
    let sql = format!(
        "{AWARD_SELECT}
         WHERE a.guild_id = $1 AND a.channel_id = ANY($2)
           AND ($3::BIGINT IS NULL OR a.user_id = $3)
           AND ($4::TEXT IS NULL OR a.item_name ILIKE $4 OR a.raid_title ILIKE $4
                OR a.character_name ILIKE $4 OR COALESCE(gm.nickname, u.display_name) ILIKE $4)
           AND ($5::TEXT IS NULL OR lower(a.loot_instance) = lower($5))
           AND ($6::BIGINT IS NULL OR a.id < $6)
         ORDER BY a.id DESC
         LIMIT $7"
    );
    let mut rows = sqlx::query_as::<_, AwardRow>(&sql)
        .bind(guild_id)
        .bind(&channels)
        .bind(who)
        .bind(text)
        .bind(instance)
        .bind(before)
        .bind(limit + 1)
        .fetch_all(&st.db)
        .await?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    Ok(Json(LootLog {
        entries: rows.into_iter().map(LootAward::from).collect(),
        has_more,
    }))
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn csv_fields_are_safe() {
        assert_eq!(csv_field("Bonereaver's Edge"), "Bonereaver's Edge");
        assert_eq!(
            csv_field("Ashkandi, Greatsword"),
            "\"Ashkandi, Greatsword\""
        );
        assert_eq!(csv_field("=HYPERLINK(1)"), "'=HYPERLINK(1)");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
