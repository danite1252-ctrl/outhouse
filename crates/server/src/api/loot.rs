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

/// "classic" or "tbc": a listed raid's era, else the raid's game's.
fn era_of(row: &EventRow, key: &str) -> &'static str {
    match raidplan::instance(key) {
        Some(i) if i.2 == "tbc" => "tbc",
        Some(_) => "classic",
        None => raidplan::era_of(&row.game),
    }
}

/// How long the loot window waits for Wowhead before showing what we have.
const TABLE_WAIT: Duration = Duration::from_secs(10);
/// Pages a raid leader can import into one list.
const MAX_SOURCES: i64 = 20;

#[derive(sqlx::FromRow)]
struct SourceRow {
    branch: String,
    kind: String,
    source_id: i32,
    name: String,
}

fn seed_items(key: &str) -> Vec<LootItem> {
    raidplan::SEED_ITEMS
        .iter()
        .filter(|s| !key.is_empty() && s.0 == key)
        .map(|&(_, item_id, name, quality, boss)| LootItem {
            item_id,
            name: name.into(),
            quality,
            boss: boss.into(),
            trash: boss.is_empty(),
            ..Default::default()
        })
        .collect()
}

/// A raid's loot list: its zone on Wowhead, pages the outhouse imported,
/// items its raid leaders added and anything already reserved. `wait`:
/// how long to wait for Wowhead when a list needs reading (0 = don't).
async fn build_table(
    st: &AppState,
    row: &EventRow,
    force: bool,
    wait: Duration,
    with_reserved: bool,
) -> AppResult<LootTable> {
    let cfg = &st.config;
    let key = table_key(row);
    let era = era_of(row, &key);
    let candidates = crate::wowhead_loot::branches_for(cfg, &row.game, era);
    let mut items: Vec<LootItem> = Vec::new();
    let mut sources = Vec::new();
    let mut bosses: Vec<String> = Vec::new();
    let mut refreshing = false;
    let mut offline = false;
    let mut item_branch: Option<String> = None;
    let push = |items: &mut Vec<LootItem>, it: LootItem| {
        if !items.iter().any(|i| i.item_id == it.item_id) {
            items.push(it);
        }
    };
    let mut lists: Vec<(bool, Vec<String>, String, i32, String)> = Vec::new();
    if let Some(zone) = raidplan::instance_zone(&key) {
        lists.push((true, candidates.clone(), "zone".into(), zone, String::new()));
    }
    if !key.is_empty() {
        let imported = sqlx::query_as::<_, SourceRow>(
            "SELECT branch, kind, source_id, name FROM loot_sources
             WHERE guild_id = $1 AND instance = $2 ORDER BY created_at",
        )
        .bind(row.guild_id)
        .bind(&key)
        .fetch_all(&st.db)
        .await?;
        for s in imported {
            lists.push((false, vec![s.branch], s.kind, s.source_id, s.name));
        }
    }
    for (builtin, branches, kind, id, saved_name) in lists {
        let got = match crate::wowhead_loot::list(st, &branches, &kind, id, force, wait).await {
            Ok(l) => Some(l),
            Err(e) => {
                tracing::warn!(error = %e, "loot list");
                None
            }
        };
        let (status, name, branch, fetched_at, count) = match &got {
            Some(l) => {
                refreshing |= l.refreshing;
                if !l.drops.is_empty() && item_branch.is_none() {
                    item_branch = Some(l.branch.clone());
                }
                for b in l.row.as_ref().map(|r| r.bosses.clone()).unwrap_or_default() {
                    if !bosses.contains(&b) {
                        bosses.push(b);
                    }
                }
                for d in &l.drops {
                    push(&mut items, d.to_item());
                }
                let status = if l.refreshing && l.drops.is_empty() {
                    "pending"
                } else if l.error {
                    "error"
                } else if l.missing {
                    "missing"
                } else if l.row.is_some() {
                    "ok"
                } else {
                    "pending"
                };
                (
                    status,
                    l.row.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                    if l.branch.is_empty() {
                        branches[0].clone()
                    } else {
                        l.branch.clone()
                    },
                    l.row.as_ref().and_then(|r| r.ok_at),
                    l.drops.len(),
                )
            }
            None => ("error", String::new(), branches[0].clone(), None, 0),
        };
        if builtin && count == 0 {
            // Nothing from Wowhead (yet): the few items we know.
            offline = !refreshing;
            for it in seed_items(&key) {
                push(&mut items, it);
            }
        }
        sources.push(raidplan::LootSource {
            url: crate::wowhead_loot::page_url(cfg, &branch, &kind, id),
            kind,
            id,
            branch,
            name: if name.is_empty() {
                if builtin {
                    raidplan::instance_name(&key)
                } else {
                    saved_name
                }
            } else {
                name
            },
            builtin,
            status: status.into(),
            items: count,
            fetched_at,
        });
    }
    if !key.is_empty() {
        let added: Vec<(i32, String, i32, String)> = sqlx::query_as(
            "SELECT item_id, name, quality, boss FROM loot_items
             WHERE guild_id = $1 AND instance = $2 ORDER BY created_at, item_id",
        )
        .bind(row.guild_id)
        .bind(&key)
        .fetch_all(&st.db)
        .await?;
        for (item_id, name, quality, boss) in added {
            push(
                &mut items,
                LootItem {
                    item_id,
                    name,
                    quality,
                    boss,
                    custom: true,
                    ..Default::default()
                },
            );
        }
    }
    // Things people reserved that aren't on the list (looked up by name).
    let reserved: Vec<(i32, String, i32)> = if with_reserved {
        sqlx::query_as(
            "SELECT DISTINCT ON (item_id) item_id, item_name, item_quality FROM loot_reserves
             WHERE event_id = $1 ORDER BY item_id, created_at",
        )
        .bind(row.id)
        .fetch_all(&st.db)
        .await?
    } else {
        Vec::new()
    };
    for (item_id, name, quality) in reserved {
        push(
            &mut items,
            LootItem {
                item_id,
                name,
                quality,
                ..Default::default()
            },
        );
    }
    // Icons and details we know for items that didn't come with them.
    let bare: Vec<i32> = items
        .iter()
        .filter(|i| i.icon.is_empty())
        .map(|i| i.item_id)
        .collect();
    if !bare.is_empty() {
        let known: Vec<crate::wowhead_loot::DropRow> = sqlx::query_as(
            "SELECT DISTINCT ON (item_id) item_id, name, quality, icon, item_class, subclass,
                    slot, req_class, boss, chance, trash
             FROM wowhead_drops WHERE item_id = ANY($1) ORDER BY item_id, icon = ''",
        )
        .bind(&bare)
        .fetch_all(&st.db)
        .await?;
        let icons: Vec<(i32, String)> = sqlx::query_as(
            "SELECT DISTINCT ON (item_id) item_id, icon FROM wow_items
             WHERE item_id = ANY($1) AND icon <> '' ORDER BY item_id",
        )
        .bind(&bare)
        .fetch_all(&st.db)
        .await?;
        for it in items.iter_mut().filter(|i| i.icon.is_empty()) {
            if let Some(k) = known.iter().find(|k| k.item_id == it.item_id) {
                let full = k.to_item();
                it.icon = full.icon;
                if it.slot.is_empty() {
                    it.slot = full.slot;
                    it.category = full.category;
                    it.armour = full.armour;
                    it.classes = full.classes;
                }
            }
            if it.icon.is_empty() {
                if let Some((_, icon)) = icons.iter().find(|i| i.0 == it.item_id) {
                    it.icon = icon.clone();
                }
            }
        }
    }
    // Boss sections in raid order, then the rest in Wowhead's order.
    for it in &items {
        if !it.boss.is_empty() && !it.trash && !bosses.contains(&it.boss) {
            bosses.push(it.boss.clone());
        }
    }
    bosses.retain(|b| items.iter().any(|i| &i.boss == b && !i.trash));
    // Raid order, then other bosses (rare spawns, imported pages), then
    // loot several bosses share.
    let rank = |b: &String| match raidplan::boss_rank(&key, b) {
        Some(r) => r,
        None if b == crate::wowhead_page::SHARED => usize::MAX,
        None => usize::MAX - 1,
    };
    let mut ranked: Vec<(usize, usize, String)> = bosses
        .into_iter()
        .enumerate()
        .map(|(i, b)| (rank(&b), i, b))
        .collect();
    ranked.sort();
    let branch = match item_branch {
        Some(b) => b,
        None => crate::wowhead_loot::link_branch(st, &row.game, era).await,
    };
    Ok(LootTable {
        instance_name: raidplan::instance_name(&row.loot_instance),
        instance: row.loot_instance.clone(),
        items,
        bosses: ranked.into_iter().map(|r| r.2).collect(),
        sources,
        item_base: crate::wowhead::branch_url(cfg, &branch),
        item_branch: branch,
        refreshing,
        offline,
    })
}

/// The items on a raid's list (not counting reserves made by name),
/// without waiting for Wowhead.
pub(super) async fn table_items(st: &AppState, row: &EventRow) -> AppResult<Vec<LootItem>> {
    Ok(build_table(st, row, false, Duration::ZERO, false)
        .await?
        .items)
}

async fn loot_table(st: &AppState, row: &EventRow) -> AppResult<LootTable> {
    build_table(st, row, false, TABLE_WAIT, true).await
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

/// `POST /api/events/{id}/loot-table/refresh` (raid leaders): read the
/// raid's lists from Wowhead again now.
pub async fn refresh_table(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<LootTable>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    require_manager(&st, &row, &access).await?;
    if table_key(&row).is_empty() {
        return Err(AppError::BadRequest(
            "Pick which raid this is (Edit raid → Loot list) first.".into(),
        ));
    }
    Ok(Json(
        build_table(&st, &row, true, Duration::from_secs(20), true).await?,
    ))
}

/// `POST /api/events/{id}/loot-sources` `{url}` (raid leaders): add a
/// Wowhead zone, NPC or object (chest) page's loot to this raid's list for
/// the whole outhouse.
pub async fn add_source(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<raidplan::LootSourceRequest>,
) -> AppResult<Json<LootTable>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    require_manager(&st, &row, &access).await?;
    let key = table_key(&row);
    if key.is_empty() {
        return Err(AppError::BadRequest(
            "Pick which raid this is (Edit raid → Loot list) before importing loot.".into(),
        ));
    }
    let raw: String = req.url.trim().chars().take(400).collect();
    let Some((mut branch, kind, id)) = crate::wowhead_loot::parse_link(&raw) else {
        return Err(AppError::BadRequest(
            "That isn't a Wowhead zone, NPC or object link. Try one like wowhead.com/classic/zone=2717."
                .into(),
        ));
    };
    if branch.is_empty() && !st.config.wowhead_branch.is_empty() {
        // A link without a branch is the retail site; WoW: Forever's
        // lists live on its own branch, so use the raid's.
        branch = crate::wowhead_loot::link_branch(&st, &row.game, era_of(&row, &key)).await;
    }
    if raidplan::instance_zone(&key) == Some(id) && kind == "zone" {
        let mine = crate::wowhead_loot::branches_for(&st.config, &row.game, era_of(&row, &key));
        if mine.contains(&branch) {
            return Err(AppError::BadRequest(
                "That's this raid's own list already.".into(),
            ));
        }
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM loot_sources WHERE guild_id = $1 AND instance = $2",
    )
    .bind(row.guild_id)
    .bind(&key)
    .fetch_one(&st.db)
    .await?;
    if count >= MAX_SOURCES {
        return Err(AppError::BadRequest(format!(
            "A raid's list can take up to {MAX_SOURCES} imported pages."
        )));
    }
    let list = crate::wowhead_loot::list(
        &st,
        std::slice::from_ref(&branch),
        &kind,
        id,
        true,
        Duration::from_secs(25),
    )
    .await
    .map_err(|e| AppError::Other(e))?;
    if list.drops.is_empty() {
        return Err(AppError::BadRequest(if list.refreshing {
            "Wowhead is slow right now. Give it a minute and try again.".into()
        } else if list.missing {
            "Wowhead doesn't have that page (check the link).".into()
        } else if list.error {
            "Couldn't reach Wowhead just now. Try again in a few minutes.".into()
        } else {
            "That page has no loot listed on Wowhead.".into()
        }));
    }
    let name = list
        .row
        .as_ref()
        .map(|r| r.name.clone())
        .unwrap_or_default();
    let added = sqlx::query(
        "INSERT INTO loot_sources (guild_id, instance, branch, kind, source_id, name, added_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING",
    )
    .bind(row.guild_id)
    .bind(&key)
    .bind(&branch)
    .bind(&kind)
    .bind(id)
    .bind(&name)
    .bind(user.id)
    .execute(&st.db)
    .await?
    .rows_affected();
    if added == 0 {
        return Err(AppError::BadRequest(
            "That page is on the list already.".into(),
        ));
    }
    crate::audit::Entry::new(row.guild_id, user.id, "loot_source_add")
        .details(format!(
            "Imported {name} ({} items) into the {} loot list",
            list.drops.len(),
            raidplan::instance_name(&row.loot_instance)
        ))
        .save(&st)
        .await;
    Ok(Json(loot_table(&st, &row).await?))
}

#[derive(Deserialize)]
pub struct SourceQuery {
    #[serde(default)]
    branch: String,
    kind: String,
    id: i32,
}

/// `DELETE /api/events/{id}/loot-sources?branch=&kind=&id=` (raid leaders).
pub async fn remove_source(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Query(q): Query<SourceQuery>,
) -> AppResult<Json<LootTable>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    require_manager(&st, &row, &access).await?;
    let gone: Option<String> = sqlx::query_scalar(
        "DELETE FROM loot_sources
         WHERE guild_id = $1 AND instance = $2 AND branch = $3 AND kind = $4 AND source_id = $5
         RETURNING name",
    )
    .bind(row.guild_id)
    .bind(table_key(&row))
    .bind(&q.branch)
    .bind(&q.kind)
    .bind(q.id)
    .fetch_optional(&st.db)
    .await?;
    let Some(name) = gone else {
        return Err(AppError::NotFound);
    };
    crate::audit::Entry::new(row.guild_id, user.id, "loot_source_remove")
        .details(format!(
            "Removed {name} from the {} loot list",
            raidplan::instance_name(&row.loot_instance)
        ))
        .save(&st)
        .await;
    Ok(Json(loot_table(&st, &row).await?))
}

// ---- Finding items ---------------------------------------------------------------

/// Our copy of an item's icon (`/api/wow/item-icons/{id}`).
fn icon_path(item_id: i32) -> String {
    format!("/api/wow/item-icons/{item_id}")
}

fn item_lookup(st: &AppState, item: &crate::wowhead::ItemInfo) -> ItemLookup {
    let cfg = &st.config;
    ItemLookup {
        item_id: item.item_id,
        name: item.name.clone(),
        quality: item.quality,
        icon_url: if item.icon.is_empty() {
            String::new()
        } else {
            icon_path(item.item_id)
        },
        tooltip: item.tooltip.clone(),
        url: crate::wowhead::item_url(cfg, item.item_id),
        lines: item.tip_lines(),
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
        lines: Vec::new(),
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
    let items = table_items(st, row).await?;
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
             UNION ALL
             SELECT name, quality FROM wowhead_drops WHERE item_id = $1
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
    /// Wowhead branch for the item's details ("tbc", …).
    branch: Option<String>,
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

/// `GET /api/wow/items/{id}[?branch=tbc]`: one item (tooltips).
pub async fn get_item(
    State(st): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(item_id): Path<i32>,
    Query(q): Query<ItemQuery>,
) -> AppResult<Json<ItemLookup>> {
    if !(1..=10_000_000).contains(&item_id) {
        return Err(AppError::NotFound);
    }
    let branch = q
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| b.len() <= 30 && b.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or("")
        .to_string();
    let env = crate::wowhead::data_env_for(&st.config, &branch);
    if let Ok(Ok(Some(item))) = tokio::time::timeout(
        Duration::from_secs(9),
        crate::wowhead::item_by_id_env(&st, item_id, &env),
    )
    .await
    {
        let mut out = item_lookup(&st, &item);
        if !branch.is_empty() {
            out.url = crate::wowhead::item_url_on(&st.config, &branch, item_id);
        }
        return Ok(Json(out));
    }
    // Not on Wowhead (or it's down): what the loot lists know.
    let known: Option<crate::wowhead_loot::DropRow> = sqlx::query_as(
        "SELECT item_id, name, quality, icon, item_class, subclass, slot, req_class, boss,
                chance, trash
         FROM wowhead_drops WHERE item_id = $1 ORDER BY (branch = $2) DESC LIMIT 1",
    )
    .bind(item_id)
    .bind(&branch)
    .fetch_optional(&st.db)
    .await?;
    let url = if branch.is_empty() {
        crate::wowhead::item_url(&st.config, item_id)
    } else {
        crate::wowhead::item_url_on(&st.config, &branch, item_id)
    };
    if let Some(k) = known {
        let it = k.to_item();
        let mut lines = Vec::new();
        if !it.slot.is_empty() {
            lines.push(raidplan::TipLine {
                left: it.slot.clone(),
                ..Default::default()
            });
        }
        if !it.boss.is_empty() {
            lines.push(raidplan::TipLine {
                left: format!("Drops from {}", it.boss),
                q: "q".into(),
                ..Default::default()
            });
        }
        return Ok(Json(ItemLookup {
            item_id,
            tooltip: lines
                .iter()
                .map(|l| l.left.clone())
                .collect::<Vec<_>>()
                .join("\n"),
            name: it.name,
            quality: it.quality,
            icon_url: if it.icon.is_empty() {
                String::new()
            } else {
                icon_path(item_id)
            },
            url,
            lines,
        }));
    }
    if let Some(s) = raidplan::SEED_ITEMS.iter().find(|s| s.1 == item_id) {
        return Ok(Json(ItemLookup {
            item_id,
            name: s.2.into(),
            quality: s.3,
            icon_url: String::new(),
            tooltip: String::new(),
            url,
            lines: Vec::new(),
        }));
    }
    look_up(&st, &item_id.to_string())
        .await
        .map(Json)
        .ok_or(AppError::NotFound)
}

/// `GET /api/wow/item-icons/{id}`: an item's icon, from our copy (no
/// sign-in, so `<img>` can load it; only items the lists know).
pub async fn item_icon(
    State(st): State<AppState>,
    Path(item_id): Path<String>,
) -> AppResult<axum::response::Response> {
    use axum::http::header;
    let item_id: i32 = item_id
        .trim_end_matches(".jpg")
        .parse()
        .map_err(|_| AppError::NotFound)?;
    if !(1..=10_000_000).contains(&item_id) {
        return Err(AppError::NotFound);
    }
    let Some((content_type, data)) = crate::wowhead_loot::icon(&st, item_id).await? else {
        return Err(AppError::NotFound);
    };
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(axum::body::Body::from(data))
        .map_err(|e| AppError::Other(e.into()))
}

// ---- Reserving ---------------------------------------------------------------------

pub(super) async fn require_manager(
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

pub(super) fn signup_of(signups: &[EventSignup], user_id: i64) -> Option<&EventSignup> {
    signups.iter().find(|s| s.user_id == user_id)
}

pub(super) fn person(s: Option<&EventSignup>, fallback: &str) -> String {
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

/// Puts a CSV field in quotes when it needs them (RFC 4180: commas, quotes
/// and line breaks), and stops spreadsheets treating it as a formula.
pub(super) fn csv_field(raw: &str) -> String {
    let mut v = raw.replace("\r\n", "\n");
    if v.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        v.insert(0, '\'');
    }
    if v.contains([',', '"', '\n', '\r']) || v.starts_with(' ') || v.ends_with(' ') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
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
    let on_list = table_items(&st, &row).await?;
    let listed = |id: i32| on_list.iter().any(|i| i.item_id == id && !i.custom);
    let found = match crate::wowhead::item_id_in(&query) {
        Some(id) if listed(id) => {
            return Err(AppError::BadRequest("That's already on the list.".into()))
        }
        _ => look_up(&st, &query).await,
    };
    let Some(item) = found else {
        return Err(AppError::BadRequest(NOT_FOUND_ITEM.into()));
    };
    if listed(item.item_id) {
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
            branch: String::new(),
        }
    }
}

/// Which Wowhead branch each entry's item links go to.
async fn fill_branches(st: &AppState, awards: &mut [LootAward]) {
    if awards.is_empty() {
        return;
    }
    let classic = crate::wowhead_loot::link_branch(st, "forever", "classic").await;
    for a in awards {
        let tbc = raidplan::instance(&raidplan::instance_key(&a.loot_instance))
            .is_some_and(|i| i.2 == "tbc");
        a.branch = if tbc {
            st.config.wowhead_branch_tbc.clone()
        } else {
            classic.clone()
        };
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
    let mut out: Vec<LootAward> = rows.into_iter().map(LootAward::from).collect();
    fill_branches(&st, &mut out).await;
    Ok(Json(out))
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
    let mut out = [LootAward::from(row)];
    fill_branches(&st, &mut out).await;
    let [out] = out;
    Ok(Json(out))
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
    let mut entries: Vec<LootAward> = rows.into_iter().map(LootAward::from).collect();
    fill_branches(&st, &mut entries).await;
    Ok(Json(LootLog { entries, has_more }))
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
        assert_eq!(csv_field("two\r\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field("Danité"), "Danité");
    }
}
