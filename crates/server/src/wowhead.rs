//! WoW link previews and Wowhead item lookups.
//!
//! Item details come from Wowhead's tooltip service (JSON) and name searches
//! from its search suggestions. Both addresses are set by the server owner
//! (`WOWHEAD_TOOLTIP_URL`, `WOWHEAD_URL`), never by users, so they're called
//! directly with a short timeout; results are cached in Postgres. Pages
//! people link to (Wowhead, Warcraft Logs, Raider.IO) are fetched with the
//! locked-down link-preview fetcher, and if that fails the card is made
//! from the link itself.

use std::time::Duration;

use anyhow::anyhow;
use outhouse_common::models::{Embed, EmbedField};
use reqwest::Url;
use serde_json::Value;

use crate::embeds::{clip, decode_entities};
use crate::state::{AppState, Config};

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(4);
/// How long item details and name searches are kept.
const ITEM_CACHE_DAYS: i32 = 7;
const SEARCH_CACHE_HOURS: i32 = 24;

// ---- Items -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ItemInfo {
    pub item_id: i32,
    pub name: String,
    pub quality: i32,
    pub icon: String,
    /// Tooltip as plain text, one line per row.
    pub tooltip: String,
}

/// Wowhead's item quality colours (poor … heirloom).
pub fn quality_colour(quality: i32) -> i32 {
    match quality {
        0 => 0x9d9d9d,
        1 => 0xffffff,
        2 => 0x1eff00,
        3 => 0x0070dd,
        4 => 0xa335ee,
        5 => 0xff8000,
        6 => 0xe6cc80,
        7 => 0x00ccff,
        _ => 0xffffff,
    }
}

pub fn quality_name(quality: i32) -> &'static str {
    match quality {
        0 => "Poor",
        1 => "Common",
        2 => "Uncommon",
        3 => "Rare",
        4 => "Epic",
        5 => "Legendary",
        6 => "Artifact",
        7 => "Heirloom",
        _ => "Item",
    }
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn branch_base(cfg: &Config) -> String {
    if cfg.wowhead_branch.is_empty() {
        cfg.wowhead_url.clone()
    } else {
        format!("{}/{}", cfg.wowhead_url, cfg.wowhead_branch)
    }
}

pub fn item_url(cfg: &Config, id: i32) -> String {
    format!("{}/item={id}", branch_base(cfg))
}

pub fn search_url(cfg: &Config, query: &str) -> String {
    format!("{}/search?q={}", branch_base(cfg), url_encode(query))
}

pub fn site_label(cfg: &Config) -> String {
    match cfg.wowhead_branch.as_str() {
        "classic" => "Wowhead Classic".into(),
        "" => "Wowhead".into(),
        other => format!("Wowhead ({other})"),
    }
}

/// Turns Wowhead's tooltip HTML into readable lines.
pub fn tooltip_text(html: &str, name: &str) -> String {
    // Drop comments (`<!--ilvl-->`) first.
    let mut s = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        s.push_str(&rest[..start]);
        rest = match rest[start..].find("-->") {
            Some(end) => &rest[start + end + 3..],
            None => "",
        };
    }
    s.push_str(rest);

    let mut out = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        let end = s[i..].find('>').map(|e| i + e).unwrap_or(s.len() - 1);
        let tag = s[i + 1..end].trim().to_ascii_lowercase();
        let tag_name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        match tag_name.as_str() {
            "br" | "tr" | "table" | "div" | "p" | "li" => out.push('\n'),
            "td" | "th" | "span" => out.push(' '),
            _ => {}
        }
        while let Some(&(j, _)) = chars.peek() {
            if j > end {
                break;
            }
            chars.next();
        }
    }
    let decoded = decode_entities(&out);
    let mut lines: Vec<String> = Vec::new();
    for line in decoded.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() || (lines.is_empty() && line == name) {
            continue;
        }
        if lines.last() != Some(&line) {
            lines.push(line);
        }
        if lines.len() >= 14 {
            break;
        }
    }
    clip(lines.join("\n"), 700)
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(LOOKUP_TIMEOUT)
        .user_agent("Mozilla/5.0 (compatible; OuthouseBot/0.4; +wow item cards)")
        .build()?)
}

/// Item details by ID: from the cache, or Wowhead's tooltip service.
pub async fn item_by_id(st: &AppState, id: i32) -> anyhow::Result<Option<ItemInfo>> {
    if id <= 0 {
        return Ok(None);
    }
    let cached = sqlx::query_as::<_, ItemInfo>(
        "SELECT item_id, name, quality, icon, tooltip FROM wow_items
         WHERE item_id = $1 AND fetched_at > now() - make_interval(days => $2)",
    )
    .bind(id)
    .bind(ITEM_CACHE_DAYS)
    .fetch_optional(&st.db)
    .await?;
    if cached.is_some() {
        return Ok(cached);
    }
    let cfg = &st.config;
    let url = format!(
        "{}/tooltip/item/{id}?dataEnv={}&locale=0",
        cfg.wowhead_tooltip_url,
        url_encode(&cfg.wowhead_data_env)
    );
    let resp = http_client()?.get(&url).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(anyhow!("tooltip lookup: HTTP {}", resp.status()));
    }
    let json: Value = resp.json().await?;
    let Some(name) = json["name"].as_str().filter(|n| !n.trim().is_empty()) else {
        return Ok(None);
    };
    let name = clip(decode_entities(name), 120);
    let info = ItemInfo {
        item_id: id,
        quality: json["quality"].as_i64().unwrap_or(1).clamp(0, 7) as i32,
        icon: json["icon"]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(80)
            .collect(),
        tooltip: tooltip_text(json["tooltip"].as_str().unwrap_or(""), &name),
        name,
    };
    sqlx::query(
        "INSERT INTO wow_items (item_id, name, quality, icon, tooltip, fetched_at)
         VALUES ($1, $2, $3, $4, $5, now())
         ON CONFLICT (item_id) DO UPDATE SET name = EXCLUDED.name, quality = EXCLUDED.quality,
             icon = EXCLUDED.icon, tooltip = EXCLUDED.tooltip, fetched_at = now()",
    )
    .bind(info.item_id)
    .bind(&info.name)
    .bind(info.quality)
    .bind(&info.icon)
    .bind(&info.tooltip)
    .execute(&st.db)
    .await?;
    Ok(Some(info))
}

/// Picks the best item from Wowhead search suggestions.
pub fn pick_search_result(json: &Value, query: &str) -> Option<i32> {
    let results = json["results"].as_array()?;
    let items: Vec<&Value> = results
        .iter()
        .filter(|r| r["type"].as_i64() == Some(3) || r["typeName"].as_str() == Some("Item"))
        .collect();
    let exact = items.iter().find(|r| {
        r["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(query.trim()))
    });
    let best = exact.or(items.first())?;
    best["id"]
        .as_i64()
        .or_else(|| best["id"].as_str()?.parse().ok())
        .and_then(|id| i32::try_from(id).ok())
}

/// Finds an item ID by name (cached for a day).
pub async fn search_item(st: &AppState, query: &str) -> anyhow::Result<Option<i32>> {
    let key = query.trim().to_lowercase();
    if key.is_empty() {
        return Ok(None);
    }
    let cached: Option<i32> = sqlx::query_scalar(
        "SELECT item_id FROM wow_item_searches
         WHERE query = $1 AND fetched_at > now() - make_interval(hours => $2)",
    )
    .bind(&key)
    .bind(SEARCH_CACHE_HOURS)
    .fetch_optional(&st.db)
    .await?;
    if cached.is_some() {
        return Ok(cached);
    }
    let url = format!(
        "{}/search/suggestions-template?q={}",
        branch_base(&st.config),
        url_encode(query.trim())
    );
    let resp = http_client()?.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("item search: HTTP {}", resp.status()));
    }
    let json: Value = resp.json().await?;
    let Some(id) = pick_search_result(&json, query) else {
        return Ok(None);
    };
    sqlx::query(
        "INSERT INTO wow_item_searches (query, item_id, fetched_at) VALUES ($1, $2, now())
         ON CONFLICT (query) DO UPDATE SET item_id = EXCLUDED.item_id, fetched_at = now()",
    )
    .bind(&key)
    .bind(id)
    .execute(&st.db)
    .await?;
    Ok(Some(id))
}

/// An item ID typed directly: "19019", "item=19019" or a Wowhead link.
pub fn item_id_in(query: &str) -> Option<i32> {
    let q = query.trim();
    if !q.is_empty() && q.len() <= 9 && q.chars().all(|c| c.is_ascii_digit()) {
        return q.parse().ok();
    }
    let pos = q.find("item=")?;
    let digits: String = q[pos + 5..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .take(9)
        .collect();
    digits.parse().ok()
}

/// Looks an item up by ID or name.
pub async fn lookup(st: &AppState, query: &str) -> anyhow::Result<Option<ItemInfo>> {
    let id = match item_id_in(query) {
        Some(id) => Some(id),
        None => search_item(st, query).await?,
    };
    match id {
        Some(id) => item_by_id(st, id).await,
        None => Ok(None),
    }
}

/// The card for an item.
pub fn item_embed(cfg: &Config, item: &ItemInfo, link: Option<&str>) -> Embed {
    let mut fields = vec![EmbedField {
        name: "Quality".into(),
        value: quality_name(item.quality).into(),
        inline: true,
    }];
    if let Some(ilvl) = item
        .tooltip
        .lines()
        .find_map(|l| l.strip_prefix("Item Level "))
    {
        fields.push(EmbedField {
            name: "Item level".into(),
            value: ilvl.trim().into(),
            inline: true,
        });
    }
    Embed {
        url: link
            .map(str::to_string)
            .unwrap_or_else(|| item_url(cfg, item.item_id)),
        kind: "wow-item".into(),
        title: Some(item.name.clone()),
        description: (!item.tooltip.is_empty()).then(|| item.tooltip.clone()),
        site_name: Some(site_label(cfg)),
        color: Some(quality_colour(item.quality)),
        thumbnail_url: (!item.icon.is_empty())
            .then(|| format!("{}/{}.jpg", cfg.wowhead_icon_url, item.icon)),
        footer: Some(format!("Item {}", item.item_id)),
        fields,
        ..Default::default()
    }
}

/// The card when an item couldn't be looked up: a Wowhead search link.
pub fn search_embed(cfg: &Config, query: &str, why: &str) -> Embed {
    Embed {
        url: search_url(cfg, query),
        kind: "wow-item".into(),
        title: Some(format!(
            "Search Wowhead for “{}”",
            clip(query.trim().into(), 80)
        )),
        description: Some(why.into()),
        site_name: Some(site_label(cfg)),
        footer: Some("Lookup unavailable".into()),
        ..Default::default()
    }
}

// ---- Links -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    Wowhead,
    WarcraftLogs,
    RaiderIo,
}

impl Site {
    fn colour(self) -> i32 {
        match self {
            Site::Wowhead => 0xd8352a,
            Site::WarcraftLogs => 0x6b8cff,
            Site::RaiderIo => 0xf5a623,
        }
    }
}

fn host_is(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

pub fn classify(url: &Url) -> Option<Site> {
    let host = url.host_str()?.to_ascii_lowercase();
    if host_is(&host, "wowhead.com") {
        Some(Site::Wowhead)
    } else if host_is(&host, "warcraftlogs.com") {
        Some(Site::WarcraftLogs)
    } else if host_is(&host, "raider.io") {
        Some(Site::RaiderIo)
    } else {
        None
    }
}

fn title_case(slug: &str) -> String {
    slug.split(['-', '_', '+', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn decoded_segments(url: &Url) -> Vec<String> {
    url.path_segments()
        .map(|s| {
            s.filter(|p| !p.is_empty())
                .map(|p| {
                    // Plain %20-style decoding (names like "Mankrik" rarely need more).
                    let bytes = p.as_bytes();
                    let mut out = Vec::with_capacity(bytes.len());
                    let mut i = 0;
                    while i < bytes.len() {
                        if bytes[i] == b'%' && i + 2 < bytes.len() {
                            if let Some(Ok(b)) =
                                p.get(i + 1..i + 3).map(|h| u8::from_str_radix(h, 16))
                            {
                                out.push(b);
                                i += 3;
                                continue;
                            }
                        }
                        out.push(bytes[i]);
                        i += 1;
                    }
                    String::from_utf8_lossy(&out).into_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Where a Wowhead link points: (branch, kind, id, slug).
pub fn wowhead_target(url: &Url) -> (String, Option<(String, i64)>, Option<String>) {
    let segs = decoded_segments(url);
    let mut branch = String::new();
    let mut target = None;
    let mut slug = None;
    for (i, seg) in segs.iter().enumerate() {
        if let Some((kind, id)) = seg.split_once('=') {
            if let Ok(id) = id.parse::<i64>() {
                target = Some((kind.to_string(), id));
                slug = segs.get(i + 1).cloned();
                break;
            }
        }
        if i == 0 {
            branch = seg.to_ascii_lowercase();
        }
    }
    (branch, target, slug)
}

/// Branches that use the Classic-era item data.
fn classic_branch(branch: &str, cfg: &Config) -> bool {
    branch == cfg.wowhead_branch
        || matches!(
            branch,
            "classic" | "wow-forever" | "forever" | "hardcore" | "sod" | "som"
        )
}

fn wowhead_label(branch: &str) -> String {
    match branch {
        "classic" => "Wowhead Classic".into(),
        "wow-forever" | "forever" => "Wowhead · WoW: Forever".into(),
        "hardcore" => "Wowhead Classic Hardcore".into(),
        "sod" => "Wowhead Season of Discovery".into(),
        _ => "Wowhead".into(),
    }
}

fn region_name(r: &str) -> String {
    match r.len() {
        2 => r.to_ascii_uppercase(),
        _ => title_case(r),
    }
}

/// The site's name and a title worked out from the link alone.
pub fn describe_link(url: &Url, site: Site) -> (String, String) {
    let segs = decoded_segments(url);
    let seg = |i: usize| segs.get(i).map(String::as_str).unwrap_or("");
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    match site {
        Site::Wowhead => {
            let (branch, target, slug) = wowhead_target(url);
            let title = match (&target, &slug) {
                (Some(_), Some(slug)) => title_case(slug),
                (Some((kind, id)), None) => format!("{} {id}", title_case(kind)),
                (None, _) => {
                    let last = segs.last().map(String::as_str).unwrap_or("");
                    if last.is_empty() || last == branch {
                        "Wowhead".into()
                    } else {
                        title_case(last)
                    }
                }
            };
            (wowhead_label(&branch), title)
        }
        Site::WarcraftLogs => {
            let flavour = host.split('.').next().unwrap_or("");
            let label = match flavour {
                "classic" | "vanilla" => "Warcraft Logs Classic",
                "fresh" => "Warcraft Logs Fresh",
                "sod" => "Warcraft Logs Season of Discovery",
                _ => "Warcraft Logs",
            }
            .to_string();
            let title = match seg(0) {
                "reports" if !seg(1).is_empty() => format!("Combat log {}", seg(1)),
                "character" if !seg(3).is_empty() => {
                    format!(
                        "{} – {} ({})",
                        title_case(seg(3)),
                        title_case(seg(2)),
                        region_name(seg(1))
                    )
                }
                "guild" if !seg(3).is_empty() => {
                    format!(
                        "<{}> – {} ({})",
                        seg(3),
                        title_case(seg(2)),
                        region_name(seg(1))
                    )
                }
                "zone" => "Zone rankings".into(),
                _ => "Warcraft Logs".into(),
            };
            (label, title)
        }
        Site::RaiderIo => {
            let title = match seg(0) {
                "characters" if !seg(3).is_empty() => {
                    format!(
                        "{} – {} ({})",
                        title_case(seg(3)),
                        title_case(seg(2)),
                        region_name(seg(1))
                    )
                }
                "guilds" if !seg(3).is_empty() => {
                    format!(
                        "<{}> – {} ({})",
                        seg(3),
                        title_case(seg(2)),
                        region_name(seg(1))
                    )
                }
                _ => "Raider.IO".into(),
            };
            ("Raider.IO".into(), title)
        }
    }
}

/// A preview for a WoW site link. Never fails: if nothing can be fetched,
/// the card is made from the link.
pub async fn preview(st: &AppState, raw_url: &str, url: &Url, site: Site) -> Embed {
    let (label, guessed_title) = describe_link(url, site);
    let (branch, target, _) = if site == Site::Wowhead {
        wowhead_target(url)
    } else {
        (String::new(), None, None)
    };

    // Wowhead item links: the item card, coloured by quality.
    if let Some((kind, id)) = &target {
        if kind == "item" && classic_branch(&branch, &st.config) {
            match tokio::time::timeout(
                LOOKUP_TIMEOUT + Duration::from_secs(1),
                item_by_id(st, *id as i32),
            )
            .await
            {
                Ok(Ok(Some(item))) => {
                    let mut e = item_embed(&st.config, &item, Some(raw_url));
                    e.site_name = Some(label.clone());
                    return e;
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => tracing::debug!(error = %e, "wowhead item lookup failed"),
                Err(_) => tracing::debug!("wowhead item lookup timed out"),
            }
        }
    }

    if !st.config.previews_offline {
        if let Ok(Some(mut e)) = crate::embeds::fetch_preview(raw_url).await {
            if e.kind == "link" {
                e.kind = "wow-link".into();
                e.site_name = Some(label);
                e.color = Some(site.colour());
                if e.title.is_none() {
                    e.title = Some(guessed_title);
                }
                // Big site banners make poor previews; show them small.
                e.thumbnail_url = e.image_url.take();
                return e;
            }
        }
    }

    Embed {
        url: raw_url.to_string(),
        kind: "wow-link".into(),
        title: Some(guessed_title),
        description: Some("Preview not available — open the link to see it.".into()),
        site_name: Some(label),
        color: Some(site.colour()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tooltips() {
        let html = r#"<table><tr><td><!--nstart--><b class="q5">Thunderfury, Blessed Blade of the Windseeker</b><!--nend--><br><!--bo-->Binds when picked up<table width="100%"><tr><td>One-Hand</td><th>Sword</th></tr></table><span>44 - 115 Damage</span><br>+5 Agility<br>Item Level <!--ilvl-->80<br>Requires Level 60</td></tr></table>"#;
        let text = tooltip_text(html, "Thunderfury, Blessed Blade of the Windseeker");
        assert!(
            text.starts_with("Binds when picked up\nOne-Hand Sword"),
            "{text}"
        );
        assert!(text.contains("Item Level 80"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn picks_items_from_search() {
        let json: Value = serde_json::from_str(
            r#"{"results":[{"type":1,"id":5,"name":"Thunderfury"},{"type":3,"id":"18564","name":"Bindings of the Windseeker"},{"type":3,"id":19019,"name":"Thunderfury, Blessed Blade of the Windseeker"}]}"#,
        )
        .unwrap();
        assert_eq!(pick_search_result(&json, "thunderfury"), Some(18564));
        assert_eq!(
            pick_search_result(&json, "Thunderfury, Blessed Blade of the Windseeker"),
            Some(19019)
        );
        assert_eq!(item_id_in("19019"), Some(19019));
        assert_eq!(
            item_id_in("https://www.wowhead.com/classic/item=19019/x"),
            Some(19019)
        );
        assert_eq!(item_id_in("thunderfury"), None);
    }

    #[test]
    fn knows_wow_links() {
        let u = Url::parse("https://www.wowhead.com/classic/item=19019/thunderfury-blessed-blade-of-the-windseeker").unwrap();
        assert_eq!(classify(&u), Some(Site::Wowhead));
        let (branch, target, _) = wowhead_target(&u);
        assert_eq!(branch, "classic");
        assert_eq!(target, Some(("item".into(), 19019)));
        assert_eq!(
            describe_link(&u, Site::Wowhead),
            (
                "Wowhead Classic".into(),
                "Thunderfury Blessed Blade Of The Windseeker".into()
            )
        );
        let u = Url::parse("https://classic.warcraftlogs.com/reports/aBcD1234#fight=3").unwrap();
        assert_eq!(classify(&u), Some(Site::WarcraftLogs));
        assert_eq!(
            describe_link(&u, Site::WarcraftLogs),
            ("Warcraft Logs Classic".into(), "Combat log aBcD1234".into())
        );
        let u = Url::parse("https://raider.io/characters/us/mankrik/Tankadin").unwrap();
        assert_eq!(
            describe_link(&u, Site::RaiderIo).1,
            "Tankadin – Mankrik (US)"
        );
        assert_eq!(
            classify(&Url::parse("https://notwowhead.com/x").unwrap()),
            None
        );
        assert_eq!(
            classify(&Url::parse("https://evil.com/wowhead.com").unwrap()),
            None
        );
    }
}
