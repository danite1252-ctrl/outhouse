//! Optional Battle.net sign-in, to import your WoW characters.
//!
//! Switched on by `BNET_CLIENT_ID` and `BNET_CLIENT_SECRET` (and
//! `BNET_REGION`). The flow: the app asks for a sign-in link
//! (`POST /api/bnet/authorize`), opens it, Battle.net sends the browser
//! back to `/api/bnet/callback` with a code, and the server swaps that for
//! a token, reads the BattleTag and the Classic character list, and adds
//! the characters to the person's profile. The token isn't kept.
//! Blizzard's Classic profile API may not list WoW: Forever characters; if
//! it lists nothing, the link still works and people add characters by hand.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Html;
use axum::Json;
use outhouse_common::wow::{BnetStatus, CharacterRequest};
use serde::Deserialize;
use serde_json::{json, Value};

use super::wow::{clean_character, MAX_CHARACTERS};
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, BnetConfig};

const STATE_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

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

fn config(st: &AppState) -> AppResult<&BnetConfig> {
    st.config.bnet.as_ref().ok_or_else(|| {
        AppError::BadRequest("Battle.net linking isn't set up on this server.".into())
    })
}

/// `GET /api/bnet/status`
pub async fn status(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<BnetStatus>> {
    let battletag: Option<String> =
        sqlx::query_scalar("SELECT battletag FROM bnet_links WHERE user_id = $1")
            .bind(user.id)
            .fetch_optional(&st.db)
            .await?;
    Ok(Json(BnetStatus {
        enabled: st.config.bnet.is_some(),
        linked: battletag.is_some(),
        battletag: battletag.filter(|b| !b.is_empty()),
    }))
}

/// Where Battle.net sends people back to.
fn redirect_uri(st: &AppState, headers: &HeaderMap) -> String {
    if let Some(base) = &st.config.public_url {
        return format!("{base}/api/bnet/callback");
    }
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let host = get("x-forwarded-host")
        .or_else(|| get("host"))
        .unwrap_or_else(|| "localhost:8080".into());
    let proto = get("x-forwarded-proto").unwrap_or_else(|| "http".into());
    format!("{proto}://{host}/api/bnet/callback")
}

/// `POST /api/bnet/authorize` → `{"url": "https://oauth.battle.net/authorize?…"}`
pub async fn authorize(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    let cfg = config(&st)?;
    sqlx::query("DELETE FROM bnet_oauth_states WHERE created_at < now() - interval '10 minutes'")
        .execute(&st.db)
        .await?;
    let state = auth::random_code(32, STATE_ALPHABET);
    let redirect = redirect_uri(&st, &headers);
    sqlx::query("INSERT INTO bnet_oauth_states (state, user_id, redirect_uri) VALUES ($1, $2, $3)")
        .bind(&state)
        .bind(user.id)
        .bind(&redirect)
        .execute(&st.db)
        .await?;
    let url = format!(
        "{}/authorize?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        cfg.oauth_url,
        url_encode(&cfg.client_id),
        url_encode(&redirect),
        url_encode("wow.profile openid"),
        state
    );
    Ok(Json(json!({ "url": url })))
}

/// `DELETE /api/bnet/link` (imported characters stay).
pub async fn unlink(State(st): State<AppState>, AuthUser(user): AuthUser) -> AppResult<StatusCode> {
    sqlx::query("DELETE FROM bnet_links WHERE user_id = $1")
        .bind(user.id)
        .execute(&st.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{t} · Outhouse</title>
<style>body{{background:#16110f;color:#f3e9e2;font:16px/1.5 system-ui,sans-serif;display:grid;place-items:center;min-height:90vh;margin:0}}
main{{max-width:420px;padding:28px;border:1px solid #3a2c25;border-radius:14px;background:#211915;text-align:center}}
h1{{font-size:20px;margin:0 0 8px}}p{{color:#c9b8ad}}a{{color:#ff8a4c}}</style></head>
<body><main><h1>{t}</h1><p>{b}</p><p><a href="/">Back to Outhouse</a></p></main>
<script>try{{window.opener&&window.opener.postMessage({{outhouse:"bnet"}},location.origin)}}catch(e){{}}</script>
</body></html>"#,
        t = escape(title),
        b = body
    ))
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .or_else(|| v["en_US"].as_str())
        .or_else(|| v["en_GB"].as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Characters in a `/profile/user/wow` answer: (name, realm, class, level).
pub fn characters_in(profile: &Value) -> Vec<CharacterRequest> {
    let mut out = Vec::new();
    for account in profile["wow_accounts"].as_array().into_iter().flatten() {
        for c in account["characters"].as_array().into_iter().flatten() {
            let Some(name) = text(&c["name"]) else {
                continue;
            };
            out.push(CharacterRequest {
                name,
                realm: text(&c["realm"]["name"])
                    .or_else(|| text(&c["realm"]["slug"]))
                    .unwrap_or_default(),
                class: text(&c["playable_class"]["name"]).unwrap_or_default(),
                level: c["level"].as_i64().map(|l| l as i32),
                ..Default::default()
            });
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.level.unwrap_or(0)));
    out
}

/// `GET /api/bnet/callback?code=…&state=…` (Battle.net sends the browser here).
pub async fn callback(State(st): State<AppState>, Query(q): Query<CallbackQuery>) -> Html<String> {
    match finish(&st, q).await {
        Ok((tag, imported)) => {
            let who = if tag.is_empty() {
                String::new()
            } else {
                format!(" as <strong>{}</strong>", escape(&tag))
            };
            let chars = match imported {
                0 => "Battle.net didn't list any Classic characters, so add yours by hand in Settings → Profile &amp; picture.".to_string(),
                1 => "Imported 1 character.".to_string(),
                n => format!("Imported {n} characters."),
            };
            page(
                "Battle.net linked",
                &format!("Linked{who}. {chars} You can close this window."),
            )
        }
        Err(msg) => page("Couldn't link Battle.net", &escape(&msg)),
    }
}

async fn finish(st: &AppState, q: CallbackQuery) -> Result<(String, usize), String> {
    let cfg = st
        .config
        .bnet
        .as_ref()
        .ok_or("Battle.net linking isn't set up on this server.")?;
    if let Some(err) = q.error {
        return Err(format!(
            "Battle.net said no ({err}). Try again from Outhouse."
        ));
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return Err("That link is missing its code. Try again from Outhouse.".into());
    };
    let row: Option<(i64, String)> = sqlx::query_as(
        "DELETE FROM bnet_oauth_states
         WHERE state = $1 AND created_at > now() - interval '10 minutes'
         RETURNING user_id, redirect_uri",
    )
    .bind(&state)
    .fetch_optional(&st.db)
    .await
    .map_err(|e| e.to_string())?;
    let Some((user_id, redirect)) = row else {
        return Err("That sign-in link has expired. Try again from Outhouse.".into());
    };

    let fail = |what: &str, e: &dyn std::fmt::Display| {
        tracing::warn!(error = %e, "Battle.net {what} failed");
        format!("Couldn't {what} with Battle.net just now. Try again in a bit.")
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Outhouse/0.4")
        .build()
        .map_err(|e| fail("connect", &e))?;
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}",
        url_encode(&code),
        url_encode(&redirect)
    );
    let token: Value = client
        .post(format!("{}/token", cfg.oauth_url))
        .basic_auth(&cfg.client_id, Some(&cfg.client_secret))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| fail("sign in", &e))?
        .json()
        .await
        .map_err(|e| fail("sign in", &e))?;
    let access = token["access_token"]
        .as_str()
        .ok_or_else(|| fail("sign in", &"no access token"))?
        .to_string();

    let battletag = match client
        .get(format!("{}/userinfo", cfg.oauth_url))
        .bearer_auth(&access)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => r
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| text(&v["battletag"]))
            .unwrap_or_default(),
        Err(e) => {
            tracing::info!(error = %e, "Battle.net userinfo failed");
            String::new()
        }
    };

    // Whatever Classic character list is available (it may be none).
    let mut found = Vec::new();
    for ns in &cfg.namespaces {
        let url = format!(
            "{}/profile/user/wow?namespace={}&locale=en_US",
            cfg.api_url,
            url_encode(ns)
        );
        match client.get(&url).bearer_auth(&access).send().await {
            Ok(r) if r.status().is_success() => {
                if let Ok(v) = r.json::<Value>().await {
                    found.extend(characters_in(&v));
                }
            }
            Ok(r) => {
                tracing::info!(namespace = %ns, status = %r.status(), "no Battle.net characters")
            }
            Err(e) => tracing::info!(namespace = %ns, error = %e, "Battle.net profile failed"),
        }
    }

    sqlx::query(
        "INSERT INTO bnet_links (user_id, battletag) VALUES ($1, $2)
         ON CONFLICT (user_id) DO UPDATE SET battletag = EXCLUDED.battletag, linked_at = now()",
    )
    .bind(user_id)
    .bind(&battletag)
    .execute(&st.db)
    .await
    .map_err(|e| e.to_string())?;
    let imported = import(st, user_id, &found)
        .await
        .map_err(|e| e.to_string())?;
    Ok((battletag, imported))
}

/// Adds (or refreshes) characters; returns how many were taken in.
async fn import(st: &AppState, user_id: i64, found: &[CharacterRequest]) -> AppResult<usize> {
    let mut count = 0;
    for req in found {
        // Skips anything that isn't one of the nine classes or has an odd name.
        let Ok(c) = clean_character(req) else {
            continue;
        };
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM wow_characters
             WHERE user_id = $1 AND lower(name) = lower($2) AND lower(realm) = lower($3)",
        )
        .bind(user_id)
        .bind(&c.name)
        .bind(&c.realm)
        .fetch_optional(&st.db)
        .await?;
        if let Some(id) = existing {
            sqlx::query("UPDATE wow_characters SET class = $2, level = $3 WHERE id = $1")
                .bind(id)
                .bind(&c.class)
                .bind(c.level)
                .execute(&st.db)
                .await?;
        } else {
            let total: i64 =
                sqlx::query_scalar("SELECT count(*) FROM wow_characters WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_one(&st.db)
                    .await?;
            if total >= MAX_CHARACTERS {
                continue;
            }
            sqlx::query(
                "INSERT INTO wow_characters (id, user_id, name, realm, class, level, source)
                 VALUES ($1, $2, $3, $4, $5, $6, 'bnet')",
            )
            .bind(st.ids.next_id())
            .bind(user_id)
            .bind(&c.name)
            .bind(&c.realm)
            .bind(&c.class)
            .bind(c.level)
            .execute(&st.db)
            .await?;
        }
        count += 1;
    }
    super::wow::ensure_main(&st.db, user_id).await?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_character_lists() {
        let v: Value = serde_json::from_str(
            r#"{"wow_accounts":[{"characters":[
                {"name":"Lowbie","level":12,"realm":{"name":"Forever","slug":"forever"},"playable_class":{"name":"Mage"}},
                {"name":"Tankadin","level":60,"realm":{"name":{"en_US":"Forever"}},"playable_class":{"name":{"en_US":"Paladin"}}}
            ]}]}"#,
        )
        .unwrap();
        let list = characters_in(&v);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Tankadin");
        assert_eq!(list[0].class, "Paladin");
        assert_eq!(list[1].realm, "Forever");
    }
}
