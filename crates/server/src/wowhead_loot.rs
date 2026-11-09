//! Raid loot lists from Wowhead, and item icons.
//!
//! A raid's loot comes from its Wowhead zone page (every boss's drops, with
//! who drops them), or, when the zone page doesn't say who drops what, from
//! each boss's own page. Raid leaders can also import any zone, NPC or
//! object (chest) page by pasting its link. Pages are only ever fetched
//! from the Wowhead address the server owner set (`WOWHEAD_URL`): a pasted
//! link is read for its branch, kind and ID, never fetched as given, so
//! there's nothing for a user to point at our network.
//!
//! Lists are kept in Postgres per (branch, kind, ID) and read again after a
//! week (or when a raid leader asks), so the picker works while Wowhead is
//! down. Reads are polite: one page at a time, `WOWHEAD_MIN_GAP_MS` apart,
//! with a User-Agent that says what we are.
//!
//! WoW: Forever lists try each branch in `WOWHEAD_BRANCH_FOREVER` in order
//! ("forever", then "classic"); a branch Wowhead doesn't have (404, a
//! redirect elsewhere, or 403: its CDN refuses every `/forever/…` page as of
//! November 2026) is remembered for a day, so as soon as Wowhead opens a
//! Forever section it's used. Until then Forever raids get Classic Era's
//! lists: the Classic branch's Season of Discovery rows are left out.
//!
//! A listed raid's zone page says which boss drops most items but has no
//! drop chances; bosses it says (almost) nothing about (Onyxia, Attumen,
//! chests) have their own pages read straight away, and the rest are read
//! slowly in the background for their drop chances.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use chrono::{DateTime, Utc};
use outhouse_common::raidplan;

use crate::state::{AppState, Config};
use crate::wowhead_page::{self, Drop};

const PAGE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ICON_BYTES: usize = 256 * 1024;
/// Lists are read again after this long.
pub const REFRESH_DAYS: i64 = 7;
/// A branch Wowhead doesn't have is tried again after this long.
const MISSING_RETRY_HOURS: i64 = 24;
/// After Wowhead didn't answer, wait this long before trying again.
const ERROR_RETRY_MINUTES: i64 = 10;
/// "Refresh from Wowhead" does nothing if the list was read this recently.
const FORCE_MIN_SECONDS: i64 = 60;
/// Boss pages read for one zone, at most.
const MAX_BOSS_PAGES: usize = 30;
const USER_AGENT: &str = "OuthouseLoot/1.0 (self-hosted guild chat app; reads raid loot tables for soft reserves, a few dozen pages a week, slowly)";

pub const KINDS: [&str; 3] = ["zone", "npc", "object"];

/// The Wowhead branches to try for a raid, in order.
pub fn branches_for(cfg: &Config, game: &str, era: &str) -> Vec<String> {
    if era == "tbc" || game == "tbc" {
        vec![cfg.wowhead_branch_tbc.clone()]
    } else if game == "era" {
        vec![cfg.wowhead_branch.clone()]
    } else {
        cfg.wowhead_branches_forever.clone()
    }
}

/// A page on our Wowhead.
pub fn page_url(cfg: &Config, branch: &str, kind: &str, id: i32) -> String {
    format!("{}/{kind}={id}", crate::wowhead::branch_url(cfg, branch))
}

/// A pasted Wowhead link → (branch, kind, ID). Only wowhead.com links to
/// a zone, NPC or object page.
pub fn parse_link(raw: &str) -> Option<(String, String, i32)> {
    let raw = raw.trim();
    let with_scheme = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let url = reqwest::Url::parse(&with_scheme).ok()?;
    if crate::wowhead::classify(&url) != Some(crate::wowhead::Site::Wowhead) {
        return None;
    }
    let (branch, target, _) = crate::wowhead::wowhead_target(&url);
    let (kind, id) = target?;
    let kind = kind.to_ascii_lowercase();
    if !KINDS.contains(&kind.as_str()) || !(1..=10_000_000).contains(&id) {
        return None;
    }
    // The first path part is the branch unless it's the page itself.
    let branch = if branch.contains('=') {
        String::new()
    } else {
        branch
    };
    if !branch
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || branch.len() > 30
    {
        return None;
    }
    Some((branch, kind, id as i32))
}

// ---- Fetching ----------------------------------------------------------------

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(PAGE_TIMEOUT)
            // Wowhead answers a branch it doesn't have with a redirect.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .build()
            .expect("http client")
    })
}

/// Waits our turn: Wowhead pages at most one per `WOWHEAD_MIN_GAP_MS`.
async fn polite_wait(gap: Duration) {
    static NEXT: OnceLock<tokio::sync::Mutex<Instant>> = OnceLock::new();
    let lock = NEXT.get_or_init(|| tokio::sync::Mutex::new(Instant::now()));
    let mut next = lock.lock().await;
    let now = Instant::now();
    if *next > now {
        tokio::time::sleep(*next - now).await;
    }
    *next = Instant::now() + gap;
}

enum Fetched {
    Page(String),
    /// Not on this branch (404, or sent somewhere else).
    Missing,
    /// Wowhead's CDN refused it (403 "Request blocked"): what it answers
    /// for its WoW: Forever section (2026-11), and when it's had too many
    /// requests from us.
    Forbidden,
}

/// Where a redirect for `url` goes, if it's the same page: Wowhead sends
/// `/classic/zone=2717` to `/classic/zone=2717/molten-core`. Anything else
/// (another branch, the home page) means it hasn't got the page.
fn same_page_redirect(url: &str, location: &str) -> Option<String> {
    let from = reqwest::Url::parse(url).ok()?;
    let to = from.join(location).ok()?;
    let slug = to.path().strip_prefix(from.path())?.strip_prefix('/')?;
    let ok = to.host_str() == from.host_str()
        && to.scheme() == from.scheme()
        && to.port_or_known_default() == from.port_or_known_default()
        && !slug.is_empty()
        && !slug.contains('/')
        && slug.len() <= 120;
    ok.then(|| to.to_string())
}

async fn fetch_page(cfg: &Config, url: &str) -> anyhow::Result<Fetched> {
    polite_wait(Duration::from_millis(cfg.wowhead_min_gap_ms)).await;
    let mut resp = client().get(url).send().await?;
    if resp.status().is_redirection() {
        let next = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|l| l.to_str().ok())
            .and_then(|l| same_page_redirect(url, l));
        let Some(next) = next else {
            return Ok(Fetched::Missing);
        };
        resp = client().get(&next).send().await?;
    }
    let status = resp.status();
    if status.is_redirection()
        || status == reqwest::StatusCode::NOT_FOUND
        || status == reqwest::StatusCode::GONE
    {
        return Ok(Fetched::Missing);
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Ok(Fetched::Forbidden);
    }
    if !status.is_success() {
        bail!("Wowhead answered HTTP {status}");
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_PAGE_BYTES {
            body.truncate(MAX_PAGE_BYTES);
            break;
        }
    }
    Ok(Fetched::Page(String::from_utf8_lossy(&body).into_owned()))
}

/// Wowhead refused a page (403).
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Wowhead refused the page (HTTP 403)")
    }
}

impl std::error::Error for Refused {}

/// Which game version's rows to keep from a branch's pages: the Classic
/// branch also lists Season of Discovery; WoW: Forever raids are Classic
/// Era's (until Wowhead has a Forever branch of its own).
fn version_for(branch: &str) -> wowhead_page::Version {
    if branch == "classic" {
        wowhead_page::CLASSIC_ERA
    } else {
        wowhead_page::Version::default()
    }
}

/// An encounter with fewer items than this on the zone page has its own
/// page read (Attumen's loot isn't credited to him on Karazhan's).
const FEW_ITEMS: usize = 3;

/// A list as read from Wowhead.
struct ReadList {
    name: String,
    bosses: Vec<String>,
    drops: Vec<Drop>,
    /// A listed raid's encounter pages not read yet (for drop chances):
    /// (encounter, kind, ID).
    later: Vec<(String, &'static str, i32)>,
}

/// Puts a boss or chest page's loot under `boss` (its items the zone page
/// had as trash, unknown or shared become this boss's).
fn merge_boss_page(drops: &mut Vec<Drop>, page: Vec<Drop>, boss: &str, boss_id: i32) {
    for mut d in page {
        match drops.iter_mut().find(|x| x.item_id == d.item_id) {
            Some(x) => {
                if x.boss.is_empty() || x.trash || x.boss == wowhead_page::SHARED {
                    x.boss = boss.to_string();
                    x.boss_id = boss_id;
                    x.trash = false;
                }
                if x.boss == boss {
                    x.chance = d.chance.or(x.chance);
                }
                if x.icon.is_empty() {
                    x.icon = d.icon;
                }
            }
            None => {
                d.boss = boss.to_string();
                d.boss_id = boss_id;
                d.trash = false;
                drops.push(d);
            }
        }
    }
}

/// Credits a listed raid's zone-page drops to its encounters. Returns the
/// pages to read now (encounters the zone page says little or nothing
/// about) and the rest's, to read later for drop chances:
/// (encounter, kind, ID).
fn plan_encounters(
    key: &str,
    drops: &mut [Drop],
) -> (
    Vec<(&'static str, &'static str, i32)>,
    Vec<(String, &'static str, i32)>,
) {
    for d in drops.iter_mut() {
        if let Some((_, e)) = raidplan::encounter_for(key, d.source_id, &d.source) {
            d.boss = e.name.to_string();
            d.trash = false;
        }
    }
    let (mut now, mut later) = (Vec::new(), Vec::new());
    for e in raidplan::encounters(key) {
        let have = drops.iter().filter(|d| d.boss == e.name).count();
        for (kind, id) in e.loot_pages() {
            if have >= FEW_ITEMS {
                later.push((e.name.to_string(), kind, id));
            } else {
                now.push((e.name, kind, id));
            }
        }
    }
    (now, later)
}

async fn read_list(
    cfg: &Config,
    branch: &str,
    kind: &str,
    id: i32,
) -> anyhow::Result<Option<ReadList>> {
    let version = version_for(branch);
    let html = match fetch_page(cfg, &page_url(cfg, branch, kind, id)).await? {
        Fetched::Page(h) => h,
        Fetched::Missing => return Ok(None),
        Fetched::Forbidden => return Err(Refused.into()),
    };
    let page = wowhead_page::read_page_for(&html, version);
    if !page.has_listviews && page.drops.is_empty() {
        // Not a real page (an error page, or a branch that isn't there).
        return Ok(None);
    }
    if page.other_versions > 0 {
        tracing::debug!(
            branch,
            kind,
            id,
            rows = page.other_versions,
            "left out other game versions' loot"
        );
    }
    let mut name = wowhead_page::decode_html(&page.title).trim().to_string();
    if name.is_empty() || name.eq_ignore_ascii_case("wowhead") {
        name = format!("{} {id}", title_case(kind));
    }
    let name: String = name.chars().take(100).collect();
    let mut drops = page.drops;
    let mut bosses: Vec<String> = page.bosses.iter().map(|b| b.1.clone()).collect();
    let mut later = Vec::new();
    let raid = (kind == "zone")
        .then(|| raidplan::instance_of_zone(id))
        .flatten();
    if kind != "zone" {
        // Everything on an NPC or chest page drops from it.
        for d in &mut drops {
            d.boss = name.clone();
            d.boss_id = id;
            d.trash = false;
        }
        bosses = vec![name.clone()];
    } else if let Some(key) = raid {
        // A listed raid: its encounters (Attumen and Midnight are one; the
        // Cache of the Firelord is Majordomo's).
        let (now, chances) = plan_encounters(key, &mut drops);
        later = chances;
        for (boss, pkind, pid) in now.into_iter().take(MAX_BOSS_PAGES) {
            // The zone page doesn't say (enough of) what it drops.
            let html = match fetch_page(cfg, &page_url(cfg, branch, pkind, pid)).await? {
                Fetched::Page(h) => h,
                Fetched::Missing => continue,
                Fetched::Forbidden => break,
            };
            let boss_page = wowhead_page::read_page_for(&html, version);
            merge_boss_page(&mut drops, boss_page.drops, boss, pid);
        }
        bosses = raidplan::encounters(key)
            .iter()
            .map(|e| e.name.to_string())
            .collect();
    } else if (!page.has_sources || drops.is_empty()) && !page.bosses.is_empty() {
        // The zone page doesn't say who drops what: read each boss's page.
        for (boss_id, boss) in page.bosses.iter().take(MAX_BOSS_PAGES) {
            let html = match fetch_page(cfg, &page_url(cfg, branch, "npc", *boss_id)).await? {
                Fetched::Page(h) => h,
                Fetched::Missing => continue,
                Fetched::Forbidden => break,
            };
            let boss_page = wowhead_page::read_page_for(&html, version);
            merge_boss_page(&mut drops, boss_page.drops, boss, *boss_id);
        }
    }
    for d in &drops {
        if !d.boss.is_empty() && !d.trash && !bosses.contains(&d.boss) {
            bosses.push(d.boss.clone());
        }
    }
    bosses.retain(|b| drops.iter().any(|d| &d.boss == b && !d.trash));
    drops.truncate(1500);
    Ok(Some(ReadList {
        name,
        bosses,
        drops,
        later,
    }))
}

/// Drop chances for a listed raid's bosses whose pages weren't needed for
/// the list itself (zone pages have no chances, boss pages do). Read
/// slowly in the background, a page every ten gaps, and stopped at the
/// first sign of trouble.
async fn add_chances(
    st: AppState,
    branch: String,
    zone: i32,
    later: Vec<(String, &'static str, i32)>,
) {
    let cfg = &st.config;
    let gap = Duration::from_millis(cfg.wowhead_min_gap_ms.saturating_mul(10));
    let version = version_for(&branch);
    let mut updated = 0u64;
    for (boss, kind, id) in later.into_iter().take(MAX_BOSS_PAGES) {
        tokio::time::sleep(gap).await;
        let html = match fetch_page(cfg, &page_url(cfg, &branch, kind, id)).await {
            Ok(Fetched::Page(h)) => h,
            Ok(Fetched::Missing) => continue,
            Ok(Fetched::Forbidden) | Err(_) => break,
        };
        let page = wowhead_page::read_page_for(&html, version);
        let (ids, chances): (Vec<i32>, Vec<f32>) = page
            .drops
            .iter()
            .filter_map(|d| d.chance.map(|c| (d.item_id, c)))
            .unzip();
        if ids.is_empty() {
            continue;
        }
        let done = sqlx::query(
            "UPDATE wowhead_drops d SET chance = x.chance
             FROM UNNEST($4::INT[], $5::REAL[]) AS x(item_id, chance)
             WHERE d.branch = $1 AND d.kind = 'zone' AND d.source_id = $2 AND d.boss = $3
               AND d.item_id = x.item_id",
        )
        .bind(&branch)
        .bind(zone)
        .bind(&boss)
        .bind(&ids)
        .bind(&chances)
        .execute(&st.db)
        .await;
        match done {
            Ok(r) => updated += r.rows_affected(),
            Err(e) => {
                tracing::warn!(error = %e, "saving drop chances");
                break;
            }
        }
    }
    tracing::info!(branch = %branch, zone, updated, "read drop chances from Wowhead");
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
        .unwrap_or_default()
}

// ---- The cache -----------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct ListRow {
    pub branch: String,
    pub kind: String,
    pub source_id: i32,
    pub name: String,
    pub status: String,
    pub bosses: Vec<String>,
    pub fetched_at: DateTime<Utc>,
    pub ok_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DropRow {
    pub item_id: i32,
    pub name: String,
    pub quality: i32,
    pub icon: String,
    pub item_class: i32,
    pub subclass: i32,
    pub slot: i32,
    pub req_class: i32,
    pub boss: String,
    pub chance: Option<f32>,
    pub trash: bool,
}

impl DropRow {
    pub fn to_item(&self) -> raidplan::LootItem {
        raidplan::LootItem {
            item_id: self.item_id,
            name: self.name.clone(),
            quality: self.quality,
            boss: self.boss.clone(),
            custom: false,
            icon: self.icon.clone(),
            slot: raidplan::slot_label(self.item_class, self.subclass, self.slot),
            category: raidplan::item_category(self.item_class, self.subclass, self.slot).into(),
            armour: raidplan::armour_type(self.item_class, self.subclass, self.slot).into(),
            classes: raidplan::suited_classes(
                self.item_class,
                self.subclass,
                self.slot,
                self.req_class,
            ),
            chance: self.chance,
            trash: self.trash,
        }
    }
}

async fn save(
    st: &AppState,
    branch: &str,
    kind: &str,
    id: i32,
    result: &anyhow::Result<Option<ReadList>>,
) -> anyhow::Result<()> {
    let mut tx = st.db.begin().await?;
    match result {
        Ok(Some(list)) => {
            sqlx::query(
                "INSERT INTO wowhead_lists (branch, kind, source_id, name, status, error, bosses, fetched_at, ok_at)
                 VALUES ($1, $2, $3, $4, 'ok', '', $5, now(), now())
                 ON CONFLICT (branch, kind, source_id) DO UPDATE SET name = EXCLUDED.name,
                     status = 'ok', error = '', bosses = EXCLUDED.bosses, fetched_at = now(), ok_at = now()",
            )
            .bind(branch)
            .bind(kind)
            .bind(id)
            .bind(&list.name)
            .bind(&list.bosses)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "DELETE FROM wowhead_drops WHERE branch = $1 AND kind = $2 AND source_id = $3",
            )
            .bind(branch)
            .bind(kind)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            let d = &list.drops;
            sqlx::query(
                "INSERT INTO wowhead_drops (branch, kind, source_id, item_id, name, quality, icon,
                     item_class, subclass, slot, req_class, boss, boss_id, chance, trash, position)
                 SELECT $1, $2, $3, x.item_id, x.name, x.quality, x.icon, x.item_class, x.subclass,
                        x.slot, x.req_class, x.boss, x.boss_id, NULLIF(x.chance, -1), x.trash, x.position
                 FROM UNNEST($4::INT[], $5::TEXT[], $6::INT[], $7::TEXT[], $8::INT[], $9::INT[],
                             $10::INT[], $11::INT[], $12::TEXT[], $13::INT[], $14::REAL[],
                             $15::BOOL[], $16::INT[])
                   AS x(item_id, name, quality, icon, item_class, subclass, slot, req_class, boss,
                        boss_id, chance, trash, position)
                 ON CONFLICT DO NOTHING",
            )
            .bind(branch)
            .bind(kind)
            .bind(id)
            .bind(d.iter().map(|x| x.item_id).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.name.chars().take(120).collect::<String>()).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.quality).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.icon.clone()).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.item_class).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.subclass).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.slot).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.req_class).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.boss.chars().take(100).collect::<String>()).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.boss_id).collect::<Vec<_>>())
            .bind(d.iter().map(|x| x.chance.unwrap_or(-1.0)).collect::<Vec<f32>>())
            .bind(d.iter().map(|x| x.trash).collect::<Vec<_>>())
            .bind((0..d.len() as i32).collect::<Vec<_>>())
            .execute(&mut *tx)
            .await?;
        }
        Ok(None) | Err(_) => {
            let (status, error) = match result {
                Err(e) => ("error", e.to_string()),
                _ => ("missing", String::new()),
            };
            // A list that was read before keeps its items (and ok_at).
            sqlx::query(
                "INSERT INTO wowhead_lists (branch, kind, source_id, status, error, fetched_at)
                 VALUES ($1, $2, $3, $4, $5, now())
                 ON CONFLICT (branch, kind, source_id) DO UPDATE SET status = EXCLUDED.status,
                     error = EXCLUDED.error, fetched_at = now()",
            )
            .bind(branch)
            .bind(kind)
            .bind(id)
            .bind(status)
            .bind(error.chars().take(300).collect::<String>())
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Reads a list from Wowhead, trying each branch in order. Returns the
/// branch it was found on (None: no branch has it).
async fn refresh(
    st: &AppState,
    candidates: &[String],
    kind: &str,
    id: i32,
) -> anyhow::Result<Option<String>> {
    for (n, branch) in candidates.iter().enumerate() {
        let mut result = read_list(&st.config, branch, kind, id).await;
        if n + 1 < candidates.len() && result.as_ref().is_err_and(|e| e.is::<Refused>()) {
            // A branch Wowhead won't show us (its Forever section answers
            // 403): not there, so on to the next one.
            result = Ok(None);
        }
        save(st, branch, kind, id, &result).await?;
        match result {
            Ok(Some(list)) => {
                tracing::info!(
                    branch = %branch,
                    kind = %kind,
                    id,
                    items = list.drops.len(),
                    "read a loot list from Wowhead"
                );
                if !list.later.is_empty() {
                    let key = format!("chances:{branch}:{id}");
                    let started = inflight()
                        .lock()
                        .map(|mut s| s.insert(key.clone()))
                        .unwrap_or(false);
                    if started {
                        let (st2, b, later) = (st.clone(), branch.clone(), list.later);
                        tokio::spawn(async move {
                            add_chances(st2, b, id, later).await;
                            if let Ok(mut s) = inflight().lock() {
                                s.remove(&key);
                            }
                        });
                    }
                }
                return Ok(Some(branch.clone()));
            }
            Ok(None) => continue,
            Err(e) => {
                // Wowhead is down: the branches after this one weren't
                // tried, so they're marked too (their saved lists stay).
                let rest: Vec<String> = candidates
                    .iter()
                    .skip_while(|c| *c != branch)
                    .skip(1)
                    .cloned()
                    .collect();
                sqlx::query(
                    "UPDATE wowhead_lists SET status = 'error', error = $4, fetched_at = now()
                     WHERE kind = $1 AND source_id = $2 AND branch = ANY($3) AND status <> 'missing'",
                )
                .bind(kind)
                .bind(id)
                .bind(&rest)
                .bind(e.to_string().chars().take(300).collect::<String>())
                .execute(&st.db)
                .await?;
                tracing::warn!(branch = %branch, kind = %kind, id, error = %e, "couldn't read a loot list from Wowhead");
                return Err(e);
            }
        }
    }
    Ok(None)
}

fn inflight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A loot list as we have it now.
#[derive(Debug, Clone)]
pub struct List {
    /// The branch it came from ("" if we have nothing yet).
    pub branch: String,
    pub row: Option<ListRow>,
    pub drops: Vec<DropRow>,
    /// Being read from Wowhead right now.
    pub refreshing: bool,
    /// Wowhead doesn't have it on any branch we try.
    pub missing: bool,
    /// Wowhead didn't answer the last time we asked.
    pub error: bool,
}

async fn rows_for(
    st: &AppState,
    candidates: &[String],
    kind: &str,
    id: i32,
) -> anyhow::Result<Vec<ListRow>> {
    Ok(sqlx::query_as::<_, ListRow>(
        "SELECT branch, kind, source_id, name, status, bosses, fetched_at, ok_at
         FROM wowhead_lists WHERE kind = $1 AND source_id = $2 AND branch = ANY($3)",
    )
    .bind(kind)
    .bind(id)
    .bind(candidates)
    .fetch_all(&st.db)
    .await?)
}

/// The list for (kind, ID) from the first branch that has it. Reads it from
/// Wowhead when we don't have it, it's a week old or `force` (a raid leader
/// asked), waiting up to `wait` for that; otherwise what's saved is
/// returned and the read carries on in the background.
pub async fn list(
    st: &AppState,
    candidates: &[String],
    kind: &str,
    id: i32,
    force: bool,
    wait: Duration,
) -> anyhow::Result<List> {
    let now = Utc::now();
    let rows = rows_for(st, candidates, kind, id).await?;
    let row_of = |b: &str| rows.iter().find(|r| r.branch == b);
    let mut need = false;
    let mut recent_error = false;
    for b in candidates {
        match row_of(b) {
            None => {
                need = true;
                break;
            }
            Some(r) if r.status == "missing" => {
                if now - r.fetched_at > chrono::Duration::hours(MISSING_RETRY_HOURS) {
                    need = true;
                    break;
                }
            }
            Some(r) => {
                let can_retry = r.status != "error"
                    || now - r.fetched_at > chrono::Duration::minutes(ERROR_RETRY_MINUTES);
                recent_error = r.status == "error" && !can_retry;
                let stale = r
                    .ok_at
                    .is_none_or(|t| now - t > chrono::Duration::days(REFRESH_DAYS));
                let forced =
                    force && now - r.fetched_at > chrono::Duration::seconds(FORCE_MIN_SECONDS);
                if can_retry && (stale || forced) {
                    need = true;
                }
                break;
            }
        }
    }
    if force && !need && !recent_error {
        // Asked to refresh but nothing was due: only if it's not brand new.
        need = rows
            .iter()
            .all(|r| now - r.fetched_at > chrono::Duration::seconds(FORCE_MIN_SECONDS));
    }
    let mut refreshing = false;
    if need {
        let key = format!("{kind}:{id}:{}", candidates.join(","));
        let started = inflight()
            .lock()
            .map(|mut s| s.insert(key.clone()))
            .unwrap_or(false);
        if started {
            let st2 = st.clone();
            let cands = candidates.to_vec();
            let kind2 = kind.to_string();
            let mut task = tokio::spawn(async move {
                let out = refresh(&st2, &cands, &kind2, id).await;
                if let Ok(mut s) = inflight().lock() {
                    s.remove(&key);
                }
                out
            });
            if wait.is_zero() || tokio::time::timeout(wait, &mut task).await.is_err() {
                refreshing = true;
            }
        } else {
            refreshing = true;
        }
    }
    let rows = if need && !refreshing {
        rows_for(st, candidates, kind, id).await?
    } else {
        rows
    };
    let chosen = candidates.iter().find_map(|b| {
        rows.iter()
            .find(|r| &r.branch == b && r.ok_at.is_some() && r.status != "missing")
    });
    let missing = !refreshing
        && candidates
            .iter()
            .all(|b| rows.iter().any(|r| &r.branch == b && r.status == "missing"));
    // Whether the list we show (else the first we tried) didn't answer last time.
    let error = chosen
        .or_else(|| {
            candidates.iter().find_map(|b| {
                rows.iter()
                    .find(|r| &r.branch == b && r.status != "missing")
            })
        })
        .is_some_and(|r| r.status == "error");
    let drops =
        match chosen {
            Some(r) => sqlx::query_as::<_, DropRow>(
                "SELECT item_id, name, quality, icon, item_class, subclass, slot, req_class, boss,
                    chance, trash
             FROM wowhead_drops WHERE branch = $1 AND kind = $2 AND source_id = $3
             ORDER BY position",
            )
            .bind(&r.branch)
            .bind(kind)
            .bind(id)
            .fetch_all(&st.db)
            .await?,
            None => Vec::new(),
        };
    Ok(List {
        branch: chosen.map(|r| r.branch.clone()).unwrap_or_default(),
        row: chosen.cloned(),
        drops,
        refreshing,
        missing,
        error,
    })
}

/// The branch a game's item links go to: the first of its branches that
/// has given us a list, else the last one (e.g. "classic").
pub async fn link_branch(st: &AppState, game: &str, era: &str) -> String {
    let candidates = branches_for(&st.config, game, era);
    if candidates.len() > 1 {
        let ok: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT branch FROM wowhead_lists
             WHERE branch = ANY($1) AND ok_at IS NOT NULL AND status <> 'missing'",
        )
        .bind(&candidates)
        .fetch_all(&st.db)
        .await
        .unwrap_or_default();
        if let Some(b) = candidates.iter().find(|b| ok.contains(b)) {
            return b.clone();
        }
    }
    candidates.last().cloned().unwrap_or_default()
}

// ---- Icons ---------------------------------------------------------------------

fn icon_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .redirect(reqwest::redirect::Policy::limited(2))
            .user_agent(USER_AGENT)
            .build()
            .expect("http client")
    })
}

/// The icon name we know for an item ("" if none).
async fn icon_name(st: &AppState, item_id: i32) -> anyhow::Result<String> {
    let known: Option<String> = sqlx::query_scalar(
        "SELECT icon FROM (
             SELECT icon FROM wowhead_drops WHERE item_id = $1 AND icon <> ''
             UNION ALL
             SELECT icon FROM wow_items WHERE item_id = $1 AND icon <> ''
         ) x LIMIT 1",
    )
    .bind(item_id)
    .fetch_optional(&st.db)
    .await?;
    if let Some(icon) = known {
        return Ok(icon);
    }
    // Only items someone has used here are looked up (the picture address
    // is public, so strangers can't make us fetch anything they like).
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM loot_reserves WHERE item_id = $1)
             OR EXISTS (SELECT 1 FROM loot_awards WHERE item_id = $1)
             OR EXISTS (SELECT 1 FROM loot_items WHERE item_id = $1)",
    )
    .bind(item_id)
    .fetch_one(&st.db)
    .await?;
    if !used {
        return Ok(String::new());
    }
    let info = tokio::time::timeout(
        Duration::from_secs(8),
        crate::wowhead::item_by_id(st, item_id),
    )
    .await
    .map_err(|_| anyhow!("timed out"))??;
    Ok(info.map(|i| i.icon).unwrap_or_default())
}

/// An item's icon: (content type, bytes), from our copy or the icon host.
pub async fn icon(st: &AppState, item_id: i32) -> anyhow::Result<Option<(String, Vec<u8>)>> {
    let name = wowhead_page::clean_icon(&icon_name(st, item_id).await?);
    if name.is_empty() {
        return Ok(None);
    }
    let saved: Option<(String, Vec<u8>, bool)> = sqlx::query_as(
        "SELECT content_type, data, fetched_at > now() - interval '1 day' FROM wow_icons WHERE name = $1",
    )
    .bind(&name)
    .fetch_optional(&st.db)
    .await?;
    match saved {
        Some((ct, data, _)) if !data.is_empty() => return Ok(Some((ct, data))),
        Some((_, _, true)) => return Ok(None),
        _ => {}
    }
    static SLOTS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let _slot = SLOTS
        .get_or_init(|| tokio::sync::Semaphore::new(4))
        .acquire()
        .await?;
    let url = format!("{}/{name}.jpg", st.config.wowhead_icon_url);
    let got = async {
        let mut resp = icon_client().get(&url).send().await?;
        if !resp.status().is_success() {
            bail!("HTTP {}", resp.status());
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !ct.starts_with("image/") {
            bail!("not an image");
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_ICON_BYTES {
                bail!("too big");
            }
        }
        anyhow::Ok((ct, body))
    }
    .await;
    let (ct, data) = match got {
        Ok(v) if !v.1.is_empty() => v,
        Ok(_) | Err(_) => (String::new(), Vec::new()),
    };
    sqlx::query(
        "INSERT INTO wow_icons (name, content_type, data, fetched_at) VALUES ($1, $2, $3, now())
         ON CONFLICT (name) DO UPDATE SET content_type = EXCLUDED.content_type,
             data = EXCLUDED.data, fetched_at = now()",
    )
    .bind(&name)
    .bind(&ct)
    .bind(&data)
    .execute(&st.db)
    .await?;
    Ok((!data.is_empty()).then_some((ct, data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_redirects_to_the_same_page_only() {
        let url = "https://www.wowhead.com/classic/zone=2717";
        assert_eq!(
            same_page_redirect(url, "/classic/zone=2717/molten-core").as_deref(),
            Some("https://www.wowhead.com/classic/zone=2717/molten-core")
        );
        assert!(same_page_redirect(url, "/classic/zone=27170/x").is_none());
        assert!(same_page_redirect(url, "/classic/zone=2159/onyxias-lair").is_none());
        assert!(same_page_redirect(url, "/classic").is_none());
        assert!(same_page_redirect(url, "https://evil.example/classic/zone=2717/x").is_none());
        assert!(same_page_redirect(url, "/classic/zone=2717/a/b").is_none());
        assert!(same_page_redirect(
            "https://www.wowhead.com/forever/zone=2159",
            "/classic/zone=2159"
        )
        .is_none());
    }

    #[test]
    fn plans_which_encounter_pages_to_read() {
        let v = wowhead_page::CLASSIC_ERA;
        let mut mc = wowhead_page::read_page_for(
            include_str!("../../../ci/wowhead/classic-zone-2717.html"),
            v,
        )
        .drops;
        let (now, later) = plan_encounters("mc", &mut mc);
        // Majordomo's loot is in his chest, which the zone page doesn't list.
        assert_eq!(now, vec![("Majordomo Executus", "object", 179703)]);
        assert!(later.contains(&("Ragnaros".to_string(), "npc", 11502)));
        assert_eq!(later.len(), 9);
        let mut kara = wowhead_page::read_page_for(
            include_str!("../../../ci/wowhead/tbc-zone-3457.html"),
            wowhead_page::Version::default(),
        )
        .drops;
        let (now, later) = plan_encounters("kara", &mut kara);
        assert!(now.contains(&("Attumen the Huntsman", "npc", 16152)));
        assert!(now.contains(&("Chess Event", "object", 185119)));
        // Julianne, the Wolf and the Crone are the Opera event.
        let opera: Vec<i32> = kara
            .iter()
            .filter(|d| d.boss == "Opera Event")
            .map(|d| d.item_id)
            .collect();
        assert_eq!(opera, vec![28572, 28581, 28585]);
        assert!(later.contains(&("Opera Event".to_string(), "npc", 18168)));
        let reins = kara.iter().find(|d| d.item_id == 30480).unwrap();
        assert_eq!(reins.boss, "Attumen the Huntsman");
        // His other loot comes from his own page.
        let mut page = wowhead_page::read_page_for(
            include_str!("../../../ci/wowhead/tbc-npc-16152.html"),
            wowhead_page::Version::default(),
        )
        .drops;
        page.retain(|d| d.item_id != 28454);
        merge_boss_page(&mut kara, page, "Attumen the Huntsman", 16152);
        let bracers = kara.iter().find(|d| d.item_id == 28453).unwrap();
        assert_eq!(bracers.boss, "Attumen the Huntsman");
        assert!(!bracers.trash && bracers.chance == Some(9.22));
        assert!(kara
            .iter()
            .any(|d| d.item_id == 23809 && d.boss == "Attumen the Huntsman"));
    }

    /// With `REAL_WOWHEAD_DIR` set (the "Check Wowhead" workflow, run by
    /// hand): reads the real pages saved there as `{branch}-{kind}-{id}.html`
    /// and prints what each listed raid's list would be. Pages it would
    /// still need are written to `wanted.txt` (`branch/kind=id` lines).
    #[test]
    fn real_wowhead_pages() {
        let Ok(dir) = std::env::var("REAL_WOWHEAD_DIR") else {
            return;
        };
        let read = |branch: &str, kind: &str, id: i32| {
            std::fs::read_to_string(format!("{dir}/{branch}-{kind}-{id}.html")).ok()
        };
        let mut wanted = Vec::new();
        for (key, zone, _) in raidplan::INSTANCE_ZONES {
            let tbc = raidplan::instance(key).is_some_and(|i| i.2 == "tbc");
            let branch = if tbc { "tbc" } else { "classic" };
            let Some(html) = read(branch, "zone", *zone) else {
                continue;
            };
            let version = version_for(branch);
            let every = wowhead_page::read_page(&html);
            let page = wowhead_page::read_page_for(&html, version);
            let mut drops = page.drops.clone();
            let (now, later) = plan_encounters(key, &mut drops);
            for (boss, kind, id) in &now {
                match read(branch, kind, *id) {
                    Some(h) => {
                        let p = wowhead_page::read_page_for(&h, version);
                        merge_boss_page(&mut drops, p.drops, boss, *id);
                    }
                    None => wanted.push(format!("{branch}/{kind}={id}")),
                }
            }
            for (boss, kind, id) in &later {
                let Some(h) = read(branch, kind, *id) else {
                    continue;
                };
                for c in wowhead_page::read_page_for(&h, version).drops {
                    if let Some(d) = drops
                        .iter_mut()
                        .find(|d| d.item_id == c.item_id && &d.boss == boss)
                    {
                        d.chance = c.chance.or(d.chance);
                    }
                }
            }
            println!(
                "== {key} ({branch}/zone={zone}) \"{}\": rows {} -> {} kept, {} other versions left out; page bosses {}; has_sources {}; read now {:?}",
                page.title,
                every.drops.len(),
                page.drops.len(),
                page.other_versions,
                page.bosses.len(),
                page.has_sources,
                now
            );
            let mut sections: Vec<String> = raidplan::encounters(key)
                .iter()
                .map(|e| e.name.to_string())
                .collect();
            for d in &drops {
                if !d.trash && !sections.contains(&d.boss) {
                    sections.push(d.boss.clone());
                }
            }
            sections.push(String::new());
            for b in sections {
                let list: Vec<&Drop> = drops
                    .iter()
                    .filter(|d| {
                        if b.is_empty() {
                            d.trash
                        } else {
                            !d.trash && d.boss == b
                        }
                    })
                    .collect();
                if list.is_empty() {
                    continue;
                }
                let with_chance = list.iter().filter(|d| d.chance.is_some()).count();
                let sample: Vec<String> = list
                    .iter()
                    .filter(|d| d.quality >= 4)
                    .take(4)
                    .map(|d| {
                        format!(
                            "{} [{} q{} {} {}{}]",
                            d.name,
                            d.item_id,
                            d.quality,
                            raidplan::slot_label(d.item_class, d.subclass, d.slot),
                            d.icon,
                            d.chance.map(|c| format!(" {c}%")).unwrap_or_default()
                        )
                    })
                    .collect();
                println!(
                    "  {:<28} {:>3} items, {:>3} with chances: {}",
                    if b.is_empty() { "(trash)" } else { b.as_str() },
                    list.len(),
                    with_chance,
                    sample.join(", ")
                );
            }
        }
        if !wanted.is_empty() {
            std::fs::write(format!("{dir}/wanted.txt"), wanted.join("\n")).unwrap();
        }
    }

    #[test]
    fn reads_pasted_links() {
        assert_eq!(
            parse_link("https://www.wowhead.com/classic/zone=2717/molten-core"),
            Some(("classic".into(), "zone".into(), 2717))
        );
        assert_eq!(
            parse_link("www.wowhead.com/tbc/npc=15690/prince-malchezaar"),
            Some(("tbc".into(), "npc".into(), 15690))
        );
        assert_eq!(
            parse_link("https://www.wowhead.com/object=179703"),
            Some(("".into(), "object".into(), 179703))
        );
        assert_eq!(
            parse_link("https://www.wowhead.com/forever/zone=99001"),
            Some(("forever".into(), "zone".into(), 99001))
        );
        assert_eq!(
            parse_link("https://www.wowhead.com/classic/item=19019"),
            None
        );
        assert_eq!(parse_link("https://evil.example/classic/zone=2717"), None);
        assert_eq!(parse_link("zone=2717"), None);
    }
}
