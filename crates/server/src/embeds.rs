//! Link previews.
//!
//! After a message is posted, a background task fetches up to three links
//! in it and stores a preview (title, description, image). Fetching URLs
//! that users supply is risky, so this refuses anything that resolves to a
//! private or local address (no poking at your router or the database),
//! pins the checked address for the request, follows at most three
//! redirects (re-checking each), times out after 6 s and reads at most
//! 512 KB.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{anyhow, bail};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::Embed;
use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::Url;

use crate::api::messages::{fetch_message, insert_embeds};
use crate::state::{AppState, Audience};

const MAX_LINKS: usize = 3;
const MAX_HTML_BYTES: usize = 512 * 1024;
const MAX_REDIRECTS: usize = 3;

/// Finds http(s) links in a message.
pub fn extract_urls(content: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for word in content.split_whitespace() {
        let word = word.trim_start_matches(['<', '(']);
        if !(word.starts_with("http://") || word.starts_with("https://")) {
            continue;
        }
        let word = word.trim_end_matches(['>', ')', '.', ',', '!', '?', ';', ':', '\'', '"']);
        if Url::parse(word).is_ok() && !urls.iter().any(|u| u == word) {
            urls.push(word.to_string());
        }
        if urls.len() == MAX_LINKS {
            break;
        }
    }
    urls
}

pub fn spawn_previews(
    st: AppState,
    guild_id: i64,
    channel_id: i64,
    message_id: i64,
    content: String,
) {
    let urls = extract_urls(&content);
    if urls.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let mut embeds = Vec::new();
        for url in urls {
            // WoW sites get their own cards (and one even if the fetch fails).
            if let Some((parsed, site)) = Url::parse(&url)
                .ok()
                .and_then(|u| crate::wowhead::classify(&u).map(|s| (u, s)))
            {
                embeds.push(crate::wowhead::preview(&st, &url, &parsed, site).await);
                continue;
            }
            if st.config.previews_offline {
                continue;
            }
            match fetch_preview(&url).await {
                Ok(Some(embed)) => embeds.push(embed),
                Ok(None) => {}
                Err(e) => tracing::debug!(%url, error = %e, "no link preview"),
            }
        }
        if embeds.is_empty() {
            return;
        }
        if let Err(e) = save_and_announce(&st, guild_id, channel_id, message_id, embeds).await {
            tracing::warn!(error = %e, "saving link previews failed");
        }
    });
}

async fn save_and_announce(
    st: &AppState,
    guild_id: i64,
    channel_id: i64,
    message_id: i64,
    embeds: Vec<Embed>,
) -> anyhow::Result<()> {
    // The message may have been deleted while we were fetching.
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages WHERE id = $1)")
        .bind(message_id)
        .fetch_one(&st.db)
        .await?;
    if !exists {
        return Ok(());
    }
    insert_embeds(&st.db, message_id, &embeds)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    if let Ok(message) = fetch_message(&st.db, message_id).await {
        st.dispatch(
            Audience::Channel {
                guild_id,
                channel_id,
            },
            ServerEvent::MessageUpdate(message),
        );
    }
    Ok(())
}

/// True for addresses on the public internet.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..=127).contains(&o[1])) // carrier-grade NAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (18..=19).contains(&o[1])) // benchmarking
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v6.segments();
            let embedded_v4 = |hi: u16, lo: u16| {
                std::net::Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8)
            };
            // Old IPv4-compatible form (::a.b.c.d).
            if s[..6].iter().all(|&x| x == 0) && !v6.is_loopback() && !v6.is_unspecified() {
                return is_public(IpAddr::V4(embedded_v4(s[6], s[7])));
            }
            // NAT64 (64:ff9b::a.b.c.d) reaches the IPv4 address inside it.
            if s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|&x| x == 0) {
                return is_public(IpAddr::V4(embedded_v4(s[6], s[7])));
            }
            // 6to4 (2002:aabb:ccdd::) carries an IPv4 address too.
            if s[0] == 0x2002 {
                return is_public(IpAddr::V4(embedded_v4(s[1], s[2])));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] & 0xffc0) == 0xfec0 // old site local
                || (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1) // local NAT64
                || (s[0] == 0x2001 && s[1] == 0) // Teredo
                || (s[0] == 0x0100 && s[1..4].iter().all(|&x| x == 0)) // discard
                || s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        }
    }
}

/// Resolves the URL's host and refuses private addresses.
async fn checked_addrs(url: &Url) -> anyhow::Result<(String, Vec<SocketAddr>)> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("not http(s)");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("no host"))?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("no port"))?;
    let lookup_host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((lookup_host.as_str(), port))
        .await?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !is_public(a.ip())) {
        bail!("{host} is not a public address");
    }
    Ok((host, addrs))
}

/// A response fetched by [`safe_fetch`].
pub struct Fetched {
    /// Where we ended up after redirects.
    pub final_url: Url,
    pub content_type: String,
    /// Empty for images when `skip_image_bodies` was set.
    pub body: Vec<u8>,
}

/// GETs a user-supplied URL safely: public addresses only, at most three
/// redirects (each re-checked), a 6 s timeout and at most `max_bytes` read.
/// Returns `None` for non-success responses.
pub async fn safe_fetch(
    raw_url: &str,
    max_bytes: usize,
    skip_image_bodies: bool,
) -> anyhow::Result<Option<Fetched>> {
    let mut url = Url::parse(raw_url)?;

    for _ in 0..=MAX_REDIRECTS {
        let (host, addrs) = checked_addrs(&url).await?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // A proxy from the environment would do its own DNS and skip the
            // address check above.
            .no_proxy()
            .timeout(Duration::from_secs(6))
            .user_agent("Mozilla/5.0 (compatible; OuthouseBot/0.3; +link previews and feeds)")
            .resolve_to_addrs(&host, &addrs)
            .build()?;
        let mut resp = client.get(url.clone()).send().await?;

        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("redirect without a location"))?;
            url = url.join(location)?;
            continue;
        }
        if !resp.status().is_success() {
            return Ok(None);
        }

        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut body = Vec::new();
        if !(skip_image_bodies && content_type.starts_with("image/")) {
            while let Some(chunk) = resp.chunk().await? {
                body.extend_from_slice(&chunk);
                if body.len() >= max_bytes {
                    body.truncate(max_bytes);
                    break;
                }
            }
        }
        return Ok(Some(Fetched {
            final_url: url,
            content_type,
            body,
        }));
    }
    bail!("too many redirects")
}

/// Builds a preview for one link (or an inline image for image links).
pub async fn fetch_preview(raw_url: &str) -> anyhow::Result<Option<Embed>> {
    let Some(page) = safe_fetch(raw_url, MAX_HTML_BYTES, true).await? else {
        return Ok(None);
    };
    if page.content_type.starts_with("image/") {
        return Ok(Some(Embed {
            url: raw_url.to_string(),
            kind: "image".into(),
            image_url: Some(page.final_url.to_string()),
            ..Default::default()
        }));
    }
    if !page.content_type.starts_with("text/html") {
        return Ok(None);
    }
    let html = String::from_utf8_lossy(&page.body);
    Ok(parse_preview(&html, &page.final_url, raw_url))
}

// ---- Tiny HTML meta-tag reader ---------------------------------------

fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
        .map(|i| i + from)
}

/// Value of `attr="..."` (or single quotes) inside one tag.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find(name) {
        let start = search_from + pos;
        search_from = start + name.len();
        // Must be a whole attribute name.
        let before_ok = start == 0 || lower.as_bytes()[start - 1].is_ascii_whitespace();
        let rest = lower[search_from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let after_eq = search_from + (lower[search_from..].len() - rest.len()) + 1;
        let value = tag[after_eq..].trim_start();
        let quote = value.chars().next()?;
        if quote == '"' || quote == '\'' {
            let inner = &value[1..];
            let end = inner.find(quote)?;
            return Some(decode_entities(&inner[..end]));
        }
        let end = value
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(value.len());
        return Some(decode_entities(&value[..end]));
    }
    None
}

pub fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

pub fn clip(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        s
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

fn parse_preview(html: &str, page_url: &Url, original: &str) -> Option<Embed> {
    let head_end = find_ci(html, "</head>", 0).unwrap_or(html.len());
    let head = &html[..head_end];

    let mut meta = std::collections::HashMap::new();
    let mut pos = 0;
    while let Some(start) = find_ci(head, "<meta", pos) {
        let end = head[start..]
            .find('>')
            .map(|e| start + e)
            .unwrap_or(head.len());
        let tag = &head[start..end];
        let key = attr(tag, "property").or_else(|| attr(tag, "name"));
        if let (Some(key), Some(content)) = (key, attr(tag, "content")) {
            meta.entry(key.to_ascii_lowercase()).or_insert(content);
        }
        pos = end;
    }
    let get = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| meta.get(*k).cloned())
            .filter(|v: &String| !v.is_empty())
    };

    let html_title = find_ci(head, "<title", 0).and_then(|start| {
        let open_end = head[start..].find('>')? + start + 1;
        let close = find_ci(head, "</title>", open_end)?;
        Some(decode_entities(&head[open_end..close]))
    });

    let title = get(&["og:title", "twitter:title"]).or(html_title);
    let description = get(&["og:description", "twitter:description", "description"]);
    let image_url = get(&["og:image", "og:image:url", "twitter:image"])
        .and_then(|src| page_url.join(&src).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .map(|u| u.to_string());
    let site_name = get(&["og:site_name"]).or_else(|| page_url.host_str().map(str::to_string));

    if title.is_none() && image_url.is_none() {
        return None;
    }
    Some(Embed {
        url: original.to_string(),
        kind: "link".into(),
        title: title.map(|t| clip(t, 200)),
        description: description.map(|d| clip(d, 350)),
        site_name: site_name.map(|s| clip(s, 80)),
        image_url,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links() {
        let urls = extract_urls("look (https://wowhead.com/news) and <https://example.com/a>.");
        assert_eq!(
            urls,
            vec!["https://wowhead.com/news", "https://example.com/a"]
        );
    }

    #[test]
    fn blocks_private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:c0a8:0101::1",
            "2002:7f00:0001::",
            "fec0::1",
            "2001:0:4136:e378::1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip} should be blocked");
        }
        for ip in [
            "1.1.1.1",
            "142.250.70.78",
            "2606:4700:4700::1111",
            "64:ff9b::808:808",
            "2002:0808:0808::1",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip} should be allowed");
        }
    }

    #[test]
    fn odd_ways_of_writing_local_addresses_are_normalised() {
        // The URL parser turns decimal, octal and hex IPv4 forms into the
        // plain address before we look at it, so they're caught too.
        for raw in [
            "http://2130706433/",
            "http://0x7f.1/",
            "http://0177.0.0.1/",
            "http://127.1/",
            "http://[::ffff:7f00:1]/",
        ] {
            let url = Url::parse(raw).unwrap();
            let host = url.host_str().unwrap().trim_matches(['[', ']']).to_string();
            let ip: IpAddr = host.parse().unwrap();
            assert!(!is_public(ip), "{raw} → {ip} should be blocked");
        }
    }

    #[test]
    fn reads_open_graph_tags() {
        let html = r#"<html><head>
            <title>Fallback</title>
            <meta property="og:title" content="Patch 12.1 notes &amp; more">
            <meta name="description" content='Big changes'>
            <meta property="og:image" content="/img/hero.png">
            <meta property="og:site_name" content="Wowhead">
            </head><body></body></html>"#;
        let page = Url::parse("https://www.wowhead.com/news/123").unwrap();
        let e = parse_preview(html, &page, "https://www.wowhead.com/news/123").unwrap();
        assert_eq!(e.title.as_deref(), Some("Patch 12.1 notes & more"));
        assert_eq!(e.description.as_deref(), Some("Big changes"));
        assert_eq!(
            e.image_url.as_deref(),
            Some("https://www.wowhead.com/img/hero.png")
        );
        assert_eq!(e.site_name.as_deref(), Some("Wowhead"));
    }

    #[test]
    fn falls_back_to_title_tag() {
        let html = "<head><TITLE>Just a title</TITLE></head>";
        let page = Url::parse("https://example.com/").unwrap();
        let e = parse_preview(html, &page, "https://example.com/").unwrap();
        assert_eq!(e.title.as_deref(), Some("Just a title"));
        assert_eq!(e.site_name.as_deref(), Some("example.com"));
    }
}
