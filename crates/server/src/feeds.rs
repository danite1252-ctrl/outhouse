//! RSS / Atom news feeds.
//!
//! A background task checks each feed every 10 minutes and posts new items
//! to its channel as rich cards. Fetching goes through the same safety
//! checks as link previews.

use std::time::Duration;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use outhouse_common::models::{Embed, Feed};

use crate::api::messages::post_bot_message;
use crate::embeds::{clip, decode_entities, fetch_preview, safe_fetch};
use crate::state::AppState;

const MAX_FEED_BYTES: usize = 3 * 1024 * 1024;
const MAX_POSTS_PER_CHECK: usize = 5;
const FEED_COLOUR: i32 = 0xFF6B35;

pub const FEED_SELECT: &str = "SELECT id, guild_id, channel_id, user_id, url, title,
       include_keywords, exclude_keywords, last_checked_at, last_error FROM feeds";

/// Starts the background checker.
pub fn spawn_poller(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = check_due(&st).await {
                tracing::warn!(error = %e, "feed check failed");
            }
        }
    });
}

async fn check_due(st: &AppState) -> anyhow::Result<()> {
    let sql = format!(
        "{FEED_SELECT}
         WHERE last_checked_at IS NULL OR last_checked_at < now() - interval '10 minutes'
         ORDER BY last_checked_at NULLS FIRST LIMIT 20"
    );
    let due = sqlx::query_as::<_, Feed>(&sql).fetch_all(&st.db).await?;
    for feed in due {
        check_and_record(st, &feed, false).await;
    }
    Ok(())
}

/// Checks a feed and stores the time and any error.
pub async fn check_and_record(st: &AppState, feed: &Feed, first_run: bool) -> Option<String> {
    let result = check_feed(st, feed, first_run).await;
    let error = result.as_ref().err().map(|e| clip(format!("{e:#}"), 300));
    if let Err(e) =
        sqlx::query("UPDATE feeds SET last_checked_at = now(), last_error = $2 WHERE id = $1")
            .bind(feed.id)
            .bind(&error)
            .execute(&st.db)
            .await
    {
        tracing::warn!(error = %e, "saving feed status failed");
    }
    if let Ok(posted) = result {
        if posted > 0 {
            tracing::info!(feed = %feed.title, posted, "posted feed items");
        }
    }
    error
}

pub struct FeedItem {
    pub guid: String,
    pub title: String,
    pub link: String,
    pub summary: String,
    pub published: Option<DateTime<Utc>>,
}

pub struct ParsedFeed {
    pub title: String,
    /// Newest first.
    pub items: Vec<FeedItem>,
}

/// Turns HTML from a feed summary into plain text.
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub fn parse_feed(bytes: &[u8]) -> anyhow::Result<ParsedFeed> {
    let feed = feed_rs::parser::parse(bytes).context("that isn't an RSS or Atom feed")?;
    let title = feed
        .title
        .map(|t| strip_html(&t.content))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "News".into());
    let mut items: Vec<FeedItem> = feed
        .entries
        .into_iter()
        .filter_map(|e| {
            let link = e.links.first().map(|l| l.href.clone()).unwrap_or_default();
            let guid = if e.id.trim().is_empty() {
                link.clone()
            } else {
                e.id.clone()
            };
            if guid.is_empty() {
                return None;
            }
            Some(FeedItem {
                guid,
                title: e.title.map(|t| strip_html(&t.content)).unwrap_or_default(),
                summary: e
                    .summary
                    .map(|s| strip_html(&s.content))
                    .unwrap_or_default(),
                published: e.published.or(e.updated),
                link,
            })
        })
        .collect();
    items.sort_by(|a, b| b.published.cmp(&a.published));
    Ok(ParsedFeed { title, items })
}

fn keywords(list: &str) -> Vec<String> {
    list.split(',')
        .map(|k| k.trim().to_lowercase())
        .filter(|k| !k.is_empty())
        .collect()
}

/// Include list (any match) and exclude list (no match), on title + summary.
pub fn passes_filters(item: &FeedItem, include: &str, exclude: &str) -> bool {
    let text = format!("{} {}", item.title, item.summary).to_lowercase();
    let inc = keywords(include);
    let exc = keywords(exclude);
    (inc.is_empty() || inc.iter().any(|k| text.contains(k)))
        && !exc.iter().any(|k| text.contains(k))
}

pub async fn fetch_feed(url: &str) -> anyhow::Result<ParsedFeed> {
    let page = safe_fetch(url, MAX_FEED_BYTES, false)
        .await?
        .ok_or_else(|| anyhow!("the feed address returned an error"))?;
    parse_feed(&page.body)
}

/// Fetches the feed and posts new items. On the first run everything is
/// marked as seen and only the newest item is posted, so a new feed
/// doesn't flood the channel.
pub async fn check_feed(st: &AppState, feed: &Feed, first_run: bool) -> anyhow::Result<usize> {
    let parsed = fetch_feed(&feed.url).await?;
    let guids: Vec<String> = parsed.items.iter().map(|i| i.guid.clone()).collect();
    let seen: Vec<String> =
        sqlx::query_scalar("SELECT guid FROM feed_items WHERE feed_id = $1 AND guid = ANY($2)")
            .bind(feed.id)
            .bind(&guids[..])
            .fetch_all(&st.db)
            .await?;

    let new_items: Vec<&FeedItem> = parsed
        .items
        .iter()
        .filter(|i| !seen.contains(&i.guid))
        .collect();
    for item in &new_items {
        sqlx::query(
            "INSERT INTO feed_items (feed_id, guid) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(feed.id)
        .bind(&item.guid)
        .execute(&st.db)
        .await?;
    }

    let limit = if first_run { 1 } else { MAX_POSTS_PER_CHECK };
    let mut to_post: Vec<&FeedItem> = new_items
        .into_iter()
        .filter(|i| passes_filters(i, &feed.include_keywords, &feed.exclude_keywords))
        .take(limit)
        .collect();
    to_post.reverse(); // oldest first, so the channel reads in order

    for item in &to_post {
        let image = if item.link.is_empty() {
            None
        } else {
            fetch_preview(&item.link)
                .await
                .ok()
                .flatten()
                .and_then(|p| p.image_url)
        };
        let embed = Embed {
            url: item.link.clone(),
            kind: "rich".into(),
            title: Some(clip(
                if item.title.is_empty() {
                    "New post".into()
                } else {
                    item.title.clone()
                },
                256,
            )),
            description: Some(clip(item.summary.clone(), 350)).filter(|d| !d.is_empty()),
            image_url: image,
            color: Some(FEED_COLOUR),
            author_name: Some(clip(parsed.title.clone(), 100)),
            timestamp: item.published.map(|p| p.to_rfc3339()),
            ..Default::default()
        };
        post_bot_message(
            st,
            feed.guild_id,
            feed.channel_id,
            feed.user_id,
            None,
            None,
            String::new(),
            vec![embed],
        )
        .await
        .map_err(|e| anyhow!("posting: {e}"))?;
    }
    Ok(to_post.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
        <rss version="2.0"><channel><title>Wowhead News</title>
        <item><title>Patch 12.0.5 Notes</title><link>https://www.wowhead.com/news/1</link>
          <guid>news-1</guid><description>&lt;p&gt;Big &lt;b&gt;changes&lt;/b&gt;&lt;/p&gt;</description>
          <pubDate>Fri, 06 Nov 2026 10:00:00 GMT</pubDate></item>
        <item><title>PTR Build Datamined</title><link>https://www.wowhead.com/news/2</link>
          <guid>news-2</guid><description>PTR stuff</description>
          <pubDate>Sat, 07 Nov 2026 10:00:00 GMT</pubDate></item>
        </channel></rss>"#;

    #[test]
    fn parses_rss_newest_first() {
        let feed = parse_feed(RSS.as_bytes()).unwrap();
        assert_eq!(feed.title, "Wowhead News");
        assert_eq!(feed.items.len(), 2);
        assert_eq!(feed.items[0].guid, "news-2");
        assert_eq!(feed.items[1].summary, "Big changes");
    }

    #[test]
    fn keyword_filters() {
        let feed = parse_feed(RSS.as_bytes()).unwrap();
        let ptr = &feed.items[0];
        assert!(passes_filters(ptr, "", ""));
        assert!(!passes_filters(ptr, "", "ptr, beta"));
        assert!(passes_filters(&feed.items[1], "patch", "ptr"));
        assert!(!passes_filters(ptr, "patch", ""));
    }

    #[test]
    fn strips_html() {
        assert_eq!(
            strip_html("<p>Hello <b>raid</b> &amp; friends</p>"),
            "Hello raid & friends"
        );
    }
}
