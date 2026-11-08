//! Optional Battle.net sign-in, to import your WoW characters.
//!
//! Switched on by `BNET_CLIENT_ID` and `BNET_CLIENT_SECRET` (and
//! `BNET_REGION`). The flow: the app asks for a sign-in link
//! (`POST /api/bnet/authorize`), opens it, Battle.net sends the browser
//! back to `/api/bnet/callback` with a code, and the server swaps that for
//! a token, reads the BattleTag and the character lists (WoW: Forever's
//! `profile-forever-{region}` first, then Classic Era and TBC), and adds the
//! characters to the person's profile, or updates the ones already there
//! (matched by Blizzard's character id, else game + name + last name / realm).
//! The token is used for that and not kept. WoW: Forever characters are then
//! read from the Armory (with the server's app token) for their portrait,
//! guild and item level, in the background.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Html;
use axum::Json;
use outhouse_common::wow::{races_for, BnetStatus, CharacterRequest};
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

/// A character from Battle.net's account list.
#[derive(Debug, Clone)]
pub struct Listed {
    pub req: CharacterRequest,
    pub blizzard_id: Option<i64>,
    /// The ruleset Battle.net gave ("" = it didn't say).
    pub ruleset: String,
}

/// Characters in a `/profile/user/wow` answer, from `game`. WoW: Forever's
/// list has first and last names (`name`, `surname`) and no realm.
pub fn characters_in(profile: &Value, game: &str, region: &str) -> Vec<Listed> {
    let forever = game == "forever";
    let mut out = Vec::new();
    for account in profile["wow_accounts"].as_array().into_iter().flatten() {
        for c in account["characters"].as_array().into_iter().flatten() {
            let p = crate::armory::parse_character(c, None, None);
            if p.name.is_empty() {
                continue;
            }
            // A class we don't know stays as Blizzard wrote it, so it's
            // turned away (not quietly made into something else).
            let class = if p.class.is_empty() {
                text(&c["playable_class"]["name"]).unwrap_or_default()
            } else {
                p.class.clone()
            };
            let race = if races_for(game).contains(&p.race.as_str()) {
                p.race.clone()
            } else {
                String::new()
            };
            out.push(Listed {
                req: CharacterRequest {
                    name: p.name.clone(),
                    last_name: if forever {
                        p.surname.clone()
                    } else {
                        String::new()
                    },
                    realm: if forever {
                        String::new()
                    } else {
                        text(&c["realm"]["name"])
                            .or_else(|| text(&c["realm"]["slug"]))
                            .unwrap_or_default()
                    },
                    class,
                    race,
                    level: p.level,
                    game: game.to_string(),
                    ruleset: if forever {
                        p.ruleset.clone()
                    } else {
                        String::new()
                    },
                    region: region.to_string(),
                    ..Default::default()
                },
                blizzard_id: p.blizzard_id,
                ruleset: if forever { p.ruleset } else { String::new() },
            });
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.req.level.unwrap_or(0)));
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
                0 => "Battle.net didn't list any WoW: Forever or Classic characters, so add yours by hand in Settings → Profile &amp; picture.".to_string(),
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
                    found.extend(characters_in(&v, cfg.game_for(ns), &cfg.region));
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

/// Adds (or refreshes) characters; returns how many were taken in. WoW:
/// Forever ones are read from the Armory afterwards, in the background.
async fn import(st: &AppState, user_id: i64, found: &[Listed]) -> AppResult<usize> {
    let mut count = 0;
    let mut forever_ids = Vec::new();
    for listed in found {
        // Skips anything that isn't one of the nine classes, has an odd name
        // or a level the game doesn't have. A WoW: Forever character without
        // a last name (older lists) is kept; its player can add it.
        let Ok(c) = clean_character(&listed.req, true) else {
            continue;
        };
        // The same character: Blizzard's id if we have it, else the same
        // game and name, and last name (WoW: Forever; or none yet) or realm.
        let mut existing: Option<i64> = None;
        if let Some(bid) = listed.blizzard_id {
            existing = sqlx::query_scalar(
                "SELECT id FROM wow_characters
                 WHERE user_id = $1 AND game = $2 AND blizzard_id = $3
                 ORDER BY created_at LIMIT 1",
            )
            .bind(user_id)
            .bind(&c.game)
            .bind(bid)
            .fetch_optional(&st.db)
            .await?;
        }
        if existing.is_none() {
            existing = sqlx::query_scalar(
                "SELECT id FROM wow_characters
                 WHERE user_id = $1 AND game = $4 AND lower(name) = lower($2)
                   AND CASE WHEN $4 = 'forever'
                            THEN lower(last_name) = lower($5) OR last_name = '' OR $5 = ''
                            ELSE lower(realm) = lower($3) END
                   AND (blizzard_id IS NULL OR $6::BIGINT IS NULL OR blizzard_id = $6)
                 ORDER BY (lower(last_name) = lower($5)) DESC, created_at LIMIT 1",
            )
            .bind(user_id)
            .bind(&c.name)
            .bind(&c.realm)
            .bind(&c.game)
            .bind(&c.last_name)
            .bind(listed.blizzard_id)
            .fetch_optional(&st.db)
            .await?;
        }
        let id = if let Some(id) = existing {
            sqlx::query(
                "UPDATE wow_characters SET class = $2, level = $3,
                     last_name = CASE WHEN last_name = '' THEN $4 ELSE last_name END,
                     race = CASE WHEN $5 = '' THEN race ELSE $5 END,
                     ruleset = CASE WHEN $6 = '' OR game <> 'forever' THEN ruleset ELSE $6 END,
                     blizzard_id = COALESCE($7, blizzard_id),
                     region = CASE WHEN $8 = '' THEN region ELSE $8 END
                 WHERE id = $1",
            )
            .bind(id)
            .bind(&c.class)
            .bind(c.level)
            .bind(&c.last_name)
            .bind(&c.race)
            .bind(&listed.ruleset)
            .bind(listed.blizzard_id)
            .bind(&c.region)
            .execute(&st.db)
            .await?;
            id
        } else {
            let total: i64 =
                sqlx::query_scalar("SELECT count(*) FROM wow_characters WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_one(&st.db)
                    .await?;
            if total >= MAX_CHARACTERS {
                continue;
            }
            let id = st.ids.next_id();
            sqlx::query(
                "INSERT INTO wow_characters (id, user_id, name, realm, class, level, source, game,
                     ruleset, last_name, race, region, blizzard_id)
                 VALUES ($1, $2, $3, $4, $5, $6, 'bnet', $7, $8, $9, $10, $11, $12)",
            )
            .bind(id)
            .bind(user_id)
            .bind(&c.name)
            .bind(&c.realm)
            .bind(&c.class)
            .bind(c.level)
            .bind(&c.game)
            .bind(&c.ruleset)
            .bind(&c.last_name)
            .bind(&c.race)
            .bind(&c.region)
            .bind(listed.blizzard_id)
            .execute(&st.db)
            .await?;
            id
        };
        if c.game == "forever" && !c.last_name.is_empty() {
            forever_ids.push(id);
        }
        count += 1;
    }
    super::wow::ensure_main(&st.db, user_id).await?;
    if !forever_ids.is_empty() && st.config.bnet.is_some() {
        let st = st.clone();
        tokio::spawn(async move {
            for id in forever_ids.into_iter().take(MAX_CHARACTERS as usize) {
                crate::armory::sync_quietly(&st, id, true).await;
            }
        });
    }
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
        let list = characters_in(&v, "tbc", "us");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].req.game, "tbc");
        assert_eq!(list[0].req.name, "Tankadin");
        assert_eq!(list[0].req.class, "Paladin");
        assert_eq!(list[1].req.realm, "Forever");
    }

    #[test]
    fn reads_forever_lists() {
        let v: Value = serde_json::from_str(
            r#"{"wow_accounts":[{"characters":[
                {"id": 11, "name":"Breezy","surname":"Cloudsong","level":37,
                 "playable_class":{"name":"Shaman","id":7},"playable_race":{"name":"Skyborne"},
                 "faction":{"type":"HORDE"},"ruleset":{"id":1}},
                {"id": 12, "name":"Deathy","surname":"Grim","level":60,"playable_class":{"name":"Death Knight"}}
            ]}]}"#,
        )
        .unwrap();
        let list = characters_in(&v, "forever", "us");
        assert_eq!(list.len(), 2);
        let b = &list[1].req;
        assert_eq!(
            (b.name.as_str(), b.last_name.as_str(), b.realm.as_str()),
            ("Breezy", "Cloudsong", "")
        );
        assert_eq!(
            (b.race.as_str(), b.class.as_str(), b.ruleset.as_str()),
            ("Skyborne (Horde)", "Shaman", "PvP")
        );
        assert_eq!(list[1].blizzard_id, Some(11));
        assert!(clean_character(b, true).is_ok());
        assert!(clean_character(&list[0].req, true).is_err());
    }
}
