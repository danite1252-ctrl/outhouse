//! GIF search, proxied through the server so the API key stays secret.
//!
//! Uses Giphy (set GIPHY_API_KEY in .env; free keys at developers.giphy.com).
//! Google shut down the Tenor API in June 2026. Klipy is another option
//! that can be added behind the same endpoints.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::Json;
use outhouse_common::models::Gif;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct GifQuery {
    q: Option<String>,
}

/// How long a search (or trending) result is reused. Giphy's free keys allow
/// 100 calls an hour, so repeats come from memory instead.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Most different searches kept at once.
const CACHE_MAX_ENTRIES: usize = 500;

/// Recent Giphy results by normalised query ("" = trending).
#[derive(Default)]
struct GifCache {
    entries: HashMap<String, (Instant, Vec<Gif>)>,
}

impl GifCache {
    fn get(&mut self, key: &str, now: Instant) -> Option<Vec<Gif>> {
        match self.entries.get(key) {
            Some((at, gifs)) if now.duration_since(*at) < CACHE_TTL => Some(gifs.clone()),
            Some(_) => {
                self.entries.remove(key);
                None
            }
            None => None,
        }
    }

    fn put(&mut self, key: String, gifs: Vec<Gif>, now: Instant) {
        if self.entries.len() >= CACHE_MAX_ENTRIES && !self.entries.contains_key(&key) {
            self.entries
                .retain(|_, (at, _)| now.duration_since(*at) < CACHE_TTL);
            while self.entries.len() >= CACHE_MAX_ENTRIES {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(k, _)| k.clone());
                match oldest {
                    Some(k) => {
                        self.entries.remove(&k);
                    }
                    None => break,
                }
            }
        }
        self.entries.insert(key, (now, gifs));
    }
}

fn cache() -> &'static Mutex<GifCache> {
    static CACHE: OnceLock<Mutex<GifCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(GifCache::default()))
}

/// "  Leeroy   JENKINS " and "leeroy jenkins" are the same search.
fn normalise_query(q: &str) -> String {
    q.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .chars()
        .take(100)
        .collect()
}

fn parse_giphy(body: &Value) -> Vec<Gif> {
    let num = |v: &Value| -> u32 {
        v.as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| v.as_u64().map(|n| n as u32))
            .unwrap_or(0)
    };
    body["data"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let images = &item["images"];
                    let preview = &images["fixed_width"];
                    let full = if images["downsized_medium"]["url"].is_string() {
                        &images["downsized_medium"]
                    } else {
                        &images["original"]
                    };
                    Some(Gif {
                        id: item["id"].as_str()?.to_string(),
                        title: item["title"].as_str().unwrap_or("").to_string(),
                        preview_url: preview["url"].as_str()?.to_string(),
                        url: full["url"].as_str()?.to_string(),
                        width: num(&preview["width"]),
                        height: num(&preview["height"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /api/gifs?q=...` (no `q` = trending).
pub async fn search(
    State(st): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(query): Query<GifQuery>,
) -> AppResult<Json<Vec<Gif>>> {
    let key = st.config.giphy_api_key.as_deref().ok_or_else(|| {
        AppError::BadRequest("GIFs aren't set up yet: add GIPHY_API_KEY to .env.".into())
    })?;
    let q = normalise_query(query.q.as_deref().unwrap_or(""));
    let cached = cache()
        .lock()
        .expect("gif cache lock")
        .get(&q, Instant::now());
    if let Some(hit) = cached {
        return Ok(Json(hit));
    }
    let request = if q.is_empty() {
        st.http
            .get("https://api.giphy.com/v1/gifs/trending")
            .query(&[("api_key", key), ("limit", "24"), ("rating", "pg-13")])
    } else {
        st.http.get("https://api.giphy.com/v1/gifs/search").query(&[
            ("api_key", key),
            ("q", q.as_str()),
            ("limit", "24"),
            ("rating", "pg-13"),
        ])
    };
    let response = request
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("GIF search failed: {e}"))?;
    if !response.status().is_success() {
        return Err(AppError::BadRequest(format!(
            "GIF search failed ({}). Check GIPHY_API_KEY.",
            response.status()
        )));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("reading GIF results: {e}"))?;
    let gifs = parse_giphy(&body);
    cache()
        .lock()
        .expect("gif cache lock")
        .put(q, gifs.clone(), Instant::now());
    Ok(Json(gifs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(id: &str) -> Gif {
        Gif {
            id: id.into(),
            title: String::new(),
            preview_url: String::new(),
            url: String::new(),
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn queries_are_normalised() {
        assert_eq!(normalise_query("  Leeroy   JENKINS "), "leeroy jenkins");
        assert_eq!(normalise_query(""), "");
    }

    #[test]
    fn cache_expires_and_stays_bounded() {
        let start = Instant::now();
        let mut c = GifCache::default();
        c.put("murloc".into(), vec![gif("a")], start);
        assert_eq!(
            c.get("murloc", start + Duration::from_secs(60)).unwrap()[0].id,
            "a"
        );
        assert!(c
            .get("murloc", start + CACHE_TTL + Duration::from_secs(1))
            .is_none());
        for i in 0..CACHE_MAX_ENTRIES + 20 {
            c.put(
                format!("q{i}"),
                vec![gif("x")],
                start + Duration::from_millis(i as u64),
            );
        }
        assert!(c.entries.len() <= CACHE_MAX_ENTRIES);
        // The oldest searches went first.
        assert!(c.get("q0", start + Duration::from_secs(1)).is_none());
        assert!(c
            .get(
                &format!("q{}", CACHE_MAX_ENTRIES + 19),
                start + Duration::from_secs(1)
            )
            .is_some());
    }

    #[test]
    fn parses_giphy_results() {
        let body = serde_json::json!({
            "data": [{
                "id": "abc",
                "title": "Leeroy",
                "images": {
                    "fixed_width": {"url": "https://media.giphy.com/a/200w.gif", "width": "200", "height": "150"},
                    "original": {"url": "https://media.giphy.com/a/giphy.gif"}
                }
            }]
        });
        let gifs = parse_giphy(&body);
        assert_eq!(gifs.len(), 1);
        assert_eq!(gifs[0].url, "https://media.giphy.com/a/giphy.gif");
        assert_eq!(gifs[0].width, 200);
    }
}
