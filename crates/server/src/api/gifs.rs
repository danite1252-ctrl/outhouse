//! GIF search, proxied through the server so the API key stays secret.
//!
//! Uses Giphy (set GIPHY_API_KEY in .env; free keys at developers.giphy.com).
//! Google shut down the Tenor API in June 2026. Klipy is another option
//! that can be added behind the same endpoints.

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
    let q = query.q.as_deref().map(str::trim).unwrap_or("");
    let request = if q.is_empty() {
        st.http
            .get("https://api.giphy.com/v1/gifs/trending")
            .query(&[("api_key", key), ("limit", "24"), ("rating", "pg-13")])
    } else {
        st.http.get("https://api.giphy.com/v1/gifs/search").query(&[
            ("api_key", key),
            ("q", q),
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
    Ok(Json(parse_giphy(&body)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
