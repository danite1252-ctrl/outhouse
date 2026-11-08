//! WoW tools: characters on profiles, `/item` cards from Wowhead and `/char`
//! character cards.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::models::{channel_kind, Embed, EmbedField, Message, MessageInteraction};
use outhouse_common::permissions as p;
use outhouse_common::wow::{
    activities, class_name, fill_armory_template, game_id, game_label, games, max_level_for,
    professions_for, races_for, ruleset_name, CharacterRequest, WowCharacter, WowInfo,
    WowLookupRequest, CLASSES, PROFESSIONS, REGIONS,
};
use reqwest::Url;
use sqlx::PgPool;

use super::messages::post_bot_message_full;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, ChannelAccess};
use crate::state::AppState;
use crate::wowhead;

pub const MAX_CHARACTERS: i64 = 20;
const MAX_PROFESSIONS: usize = 5;
const MAX_QUERY_CHARS: usize = 100;

pub const CHARACTER_SELECT: &str = "SELECT id, user_id, name, realm, class, level, is_main,
    professions, armory_url, source, game, last_name, ruleset, race,
    region, blizzard_id, guild_name, item_level, last_synced_at, armory_status,
    CASE WHEN avatar_key <> '' THEN '/api/wow/avatars/' || avatar_key || '?v='
              || COALESCE(floor(extract(epoch FROM last_synced_at))::BIGINT, 0)
         ELSE '' END AS avatar,
    CASE WHEN last_name <> '' THEN name || ' ' || last_name ELSE name END AS full_name,
    CASE WHEN race IN ('Human', 'Dwarf', 'Night Elf', 'Gnome', 'Draenei', 'Skyborne (Alliance)')
              THEN 'Alliance'
         WHEN race IN ('Orc', 'Undead', 'Tauren', 'Troll', 'Blood Elf', 'Skyborne (Horde)')
              THEN 'Horde'
         ELSE '' END AS faction
    FROM wow_characters";

pub fn class_colour(class: &str) -> Option<i32> {
    Some(match class {
        "Druid" => 0xFF7C0A,
        "Hunter" => 0xAAD372,
        "Mage" => 0x3FC7EB,
        "Paladin" => 0xF48CBA,
        "Priest" => 0xFFFFFF,
        "Rogue" => 0xFFF468,
        "Shaman" => 0x0070DD,
        "Warlock" => 0x8788EE,
        "Warrior" => 0xC69B6D,
        _ => return None,
    })
}

/// Someone's characters, main first.
pub async fn characters_of(db: &PgPool, user_id: i64) -> AppResult<Vec<WowCharacter>> {
    let sql = format!(
        "{CHARACTER_SELECT} WHERE user_id = $1 ORDER BY is_main DESC, level DESC, lower(name)"
    );
    Ok(sqlx::query_as::<_, WowCharacter>(&sql)
        .bind(user_id)
        .fetch_all(db)
        .await?)
}

/// `GET /api/wow/info`
pub async fn info(State(st): State<AppState>, AuthUser(_user): AuthUser) -> Json<WowInfo> {
    let cfg = &st.config;
    Json(WowInfo {
        activities: activities(),
        classes: CLASSES.iter().map(|c| c.to_string()).collect(),
        professions: PROFESSIONS.iter().map(|c| c.to_string()).collect(),
        games: games(),
        bnet_enabled: cfg.bnet.is_some(),
        wowhead_url: if cfg.wowhead_branch.is_empty() {
            cfg.wowhead_url.clone()
        } else {
            format!("{}/{}", cfg.wowhead_url, cfg.wowhead_branch)
        },
        armory_enabled: cfg.bnet.is_some(),
        armory_region: cfg.armory_region.clone(),
        regions: REGIONS.iter().map(|r| r.to_string()).collect(),
        armory_links: cfg.armory_links.clone(),
    })
}

// ---- Characters ----------------------------------------------------------------

/// "tHRALL" → "Thrall". WoW names are 2 to 12 letters.
pub fn clean_name(raw: &str, what: &str) -> AppResult<String> {
    let name = raw.trim();
    let count = name.chars().count();
    if !(2..=12).contains(&count) || !name.chars().all(char::is_alphabetic) {
        return Err(AppError::BadRequest(format!(
            "{what} 2 to 12 letters (no spaces or numbers)."
        )));
    }
    let mut chars = name.chars();
    let first: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    Ok(first + &chars.as_str().to_lowercase())
}

pub struct CleanCharacter {
    pub game: String,
    pub name: String,
    pub last_name: String,
    pub realm: String,
    pub ruleset: String,
    pub race: String,
    pub class: String,
    pub level: i32,
    pub professions: Vec<String>,
    pub armory_url: String,
    /// "" or one of [`REGIONS`].
    pub region: String,
}

/// "US " → "us"; "" stays ""; anything else isn't a region.
pub fn clean_region(raw: &str) -> AppResult<String> {
    let r = raw.trim().to_ascii_lowercase();
    if r.is_empty() || REGIONS.contains(&r.as_str()) {
        Ok(r)
    } else {
        Err(AppError::BadRequest(
            "The region is US, EU, KR or TW.".into(),
        ))
    }
}

/// Checks a character for its game. `keep_empty_last_name`: a WoW: Forever
/// character may go without a last name (one moved over from before game
/// types, or imported from Battle.net) until its player adds one.
pub fn clean_character(
    req: &CharacterRequest,
    keep_empty_last_name: bool,
) -> AppResult<CleanCharacter> {
    let game = game_id(&req.game)
        .ok_or_else(|| {
            AppError::BadRequest("Pick a game: WoW: Forever, TBC or Classic Era.".into())
        })?
        .to_string();
    let forever = game == "forever";
    let name = if forever {
        clean_name(&req.name, "First names are")?
    } else {
        clean_name(&req.name, "Character names are")?
    };
    let (last_name, realm, ruleset) = if forever {
        let last = if req.last_name.trim().is_empty() && keep_empty_last_name {
            String::new()
        } else if req.last_name.trim().is_empty() {
            return Err(AppError::BadRequest(
                "WoW: Forever characters have a last name too (2 to 12 letters).".into(),
            ));
        } else {
            clean_name(&req.last_name, "Last names are")?
        };
        let ruleset = if req.ruleset.trim().is_empty() {
            "Normal"
        } else {
            ruleset_name(&req.ruleset).ok_or_else(|| {
                AppError::BadRequest("The ruleset is Normal, PvP, Roleplay or Roleplay PvP.".into())
            })?
        };
        (last, String::new(), ruleset.to_string())
    } else {
        let realm: String = req
            .realm
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(40)
            .collect();
        (String::new(), realm, String::new())
    };
    let race = if req.race.trim().is_empty() {
        String::new()
    } else {
        races_for(&game)
            .iter()
            .find(|r| r.eq_ignore_ascii_case(req.race.trim()))
            .ok_or_else(|| {
                AppError::BadRequest(format!("That race isn't in {}.", game_label(&game)))
            })?
            .to_string()
    };
    let class = class_name(&req.class)
        .ok_or_else(|| AppError::BadRequest("Pick one of the nine classes.".into()))?
        .to_string();
    let max_level = max_level_for(&game);
    let level = req.level.unwrap_or(max_level);
    if !(1..=max_level).contains(&level) {
        return Err(AppError::BadRequest(format!(
            "Levels in {} go from 1 to {max_level}.",
            game_label(&game)
        )));
    }
    let allowed = professions_for(&game);
    let mut professions: Vec<String> = Vec::new();
    for raw in &req.professions {
        let found = allowed
            .iter()
            .find(|p| p.eq_ignore_ascii_case(raw.trim()))
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "“{}” isn't a profession in {}.",
                    raw.trim(),
                    game_label(&game)
                ))
            })?;
        if !professions.iter().any(|p| p == found) {
            professions.push(found.to_string());
        }
    }
    if professions.len() > MAX_PROFESSIONS {
        return Err(AppError::BadRequest(format!(
            "Up to {MAX_PROFESSIONS} professions."
        )));
    }
    let armory_url = req.armory_url.trim().to_string();
    if !armory_url.is_empty() {
        let ok = armory_url.len() <= 300
            && Url::parse(&armory_url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        if !ok {
            return Err(AppError::BadRequest(
                "The armory link needs to be a full web address (https://…).".into(),
            ));
        }
    }
    let region = clean_region(&req.region)?;
    Ok(CleanCharacter {
        game,
        name,
        last_name,
        realm,
        ruleset,
        race,
        class,
        level,
        professions,
        armory_url,
        region,
    })
}

pub async fn fetch_character(db: &PgPool, id: i64) -> AppResult<WowCharacter> {
    let sql = format!("{CHARACTER_SELECT} WHERE id = $1");
    sqlx::query_as::<_, WowCharacter>(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

fn duplicate(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            AppError::Conflict("You already have a character with that name (and realm).".into())
        }
        _ => AppError::Db(e),
    }
}

/// Makes sure someone with characters has exactly one main.
async fn fix_main(db: &PgPool, user_id: i64, main: Option<i64>) -> AppResult<()> {
    if let Some(main) = main {
        sqlx::query("UPDATE wow_characters SET is_main = (id = $2) WHERE user_id = $1")
            .bind(user_id)
            .bind(main)
            .execute(db)
            .await?;
    } else {
        sqlx::query(
            "UPDATE wow_characters SET is_main = true
             WHERE id = (SELECT id FROM wow_characters WHERE user_id = $1
                         ORDER BY level DESC, created_at LIMIT 1)
               AND NOT EXISTS (SELECT 1 FROM wow_characters WHERE user_id = $1 AND is_main)",
        )
        .bind(user_id)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Gives someone with characters a main if they have none.
pub async fn ensure_main(db: &PgPool, user_id: i64) -> AppResult<()> {
    fix_main(db, user_id, None).await
}

/// `GET /api/users/@me/characters`
pub async fn list_mine(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<WowCharacter>>> {
    Ok(Json(characters_of(&st.db, user.id).await?))
}

/// `POST /api/users/@me/characters`
pub async fn create_character(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<CharacterRequest>,
) -> AppResult<(StatusCode, Json<WowCharacter>)> {
    let c = clean_character(&req, false)?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM wow_characters WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&st.db)
        .await?;
    if count >= MAX_CHARACTERS {
        return Err(AppError::BadRequest(format!(
            "That's {MAX_CHARACTERS} characters already. Remove one first."
        )));
    }
    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO wow_characters (id, user_id, name, realm, class, level, professions, armory_url,
             game, last_name, ruleset, race, region)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(user.id)
    .bind(&c.name)
    .bind(&c.realm)
    .bind(&c.class)
    .bind(c.level)
    .bind(&c.professions)
    .bind(&c.armory_url)
    .bind(&c.game)
    .bind(&c.last_name)
    .bind(&c.ruleset)
    .bind(&c.race)
    .bind(&c.region)
    .execute(&st.db)
    .await
    .map_err(duplicate)?;
    fix_main(&st.db, user.id, req.is_main.then_some(id)).await?;
    // Looked up on the Armory: bring in the portrait, guild and item level
    // too (if the Armory can't be reached, the character is saved anyway).
    if c.game == "forever" && !c.region.is_empty() && !c.last_name.is_empty() {
        crate::armory::sync_quietly(&st, id, false).await;
    }
    Ok((
        StatusCode::CREATED,
        Json(fetch_character(&st.db, id).await?),
    ))
}

/// `PATCH /api/characters/{id}` (send every field)
pub async fn update_character(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<CharacterRequest>,
) -> AppResult<Json<WowCharacter>> {
    let existing = fetch_character(&st.db, id).await?;
    if existing.user_id != user.id {
        return Err(AppError::NotFound);
    }
    // A Forever character without a last name (from before game types) can
    // be saved as it is until its player adds one.
    let keep_empty = existing.game == "forever" && existing.last_name.is_empty();
    let c = clean_character(&req, keep_empty)?;
    // A new name (or game) is a different character as far as the Armory
    // knows: forget what it said about the old one.
    let renamed = !existing.name.eq_ignore_ascii_case(&c.name)
        || !existing.last_name.eq_ignore_ascii_case(&c.last_name)
        || existing.game != c.game;
    sqlx::query(
        "UPDATE wow_characters SET name = $2, realm = $3, class = $4, level = $5,
             professions = $6, armory_url = $7, game = $8, last_name = $9, ruleset = $10,
             race = $11, region = CASE WHEN $12 = '' THEN region ELSE $12 END
         WHERE id = $1",
    )
    .bind(id)
    .bind(&c.name)
    .bind(&c.realm)
    .bind(&c.class)
    .bind(c.level)
    .bind(&c.professions)
    .bind(&c.armory_url)
    .bind(&c.game)
    .bind(&c.last_name)
    .bind(&c.ruleset)
    .bind(&c.race)
    .bind(&c.region)
    .execute(&st.db)
    .await
    .map_err(duplicate)?;
    if renamed {
        sqlx::query(
            "UPDATE wow_characters SET blizzard_id = NULL, guild_name = '', item_level = NULL,
                 avatar_url = '', avatar_key = '', last_synced_at = NULL, armory_status = ''
             WHERE id = $1",
        )
        .bind(id)
        .execute(&st.db)
        .await?;
        // Renamed to a character the Armory may know: read it for that one.
        let has_region = !c.region.is_empty() || !existing.region.is_empty();
        if c.game == "forever" && !c.last_name.is_empty() && has_region {
            crate::armory::sync_quietly(&st, id, false).await;
        }
    }
    if req.is_main {
        fix_main(&st.db, user.id, Some(id)).await?;
    } else if existing.is_main {
        // Unticking "main" hands it to another character, if there is one.
        sqlx::query("UPDATE wow_characters SET is_main = false WHERE id = $1")
            .bind(id)
            .execute(&st.db)
            .await?;
        let other: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM wow_characters WHERE user_id = $1 AND id <> $2
             ORDER BY level DESC, created_at LIMIT 1",
        )
        .bind(user.id)
        .bind(id)
        .fetch_optional(&st.db)
        .await?;
        fix_main(&st.db, user.id, Some(other.unwrap_or(id))).await?;
    }
    Ok(Json(fetch_character(&st.db, id).await?))
}

/// `DELETE /api/characters/{id}`
pub async fn delete_character(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let existing = fetch_character(&st.db, id).await?;
    if existing.user_id != user.id {
        return Err(AppError::NotFound);
    }
    sqlx::query("DELETE FROM wow_characters WHERE id = $1")
        .bind(id)
        .execute(&st.db)
        .await?;
    fix_main(&st.db, user.id, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- /item and /char -----------------------------------------------------------

fn lookup_query(req: &WowLookupRequest) -> AppResult<String> {
    let q: String = req
        .query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect();
    if q.is_empty() {
        return Err(AppError::BadRequest("What should I look up?".into()));
    }
    Ok(q)
}

async fn post_card(
    st: &AppState,
    access: &ChannelAccess,
    user_id: i64,
    display_name: String,
    command: &str,
    embed: Embed,
) -> AppResult<Message> {
    post_bot_message_full(
        st,
        access.channel.guild_id,
        access.channel.id,
        user_id,
        None,
        None,
        String::new(),
        vec![embed],
        Some(MessageInteraction {
            user_id,
            display_name,
            command: command.into(),
        }),
    )
    .await
}

async fn writable_text_channel(
    st: &AppState,
    channel_id: i64,
    user_id: i64,
) -> AppResult<ChannelAccess> {
    let access = channel_access(&st.db, channel_id, user_id).await?;
    access.require(p::SEND_MESSAGES)?;
    if access.channel.kind == channel_kind::VOICE {
        return Err(AppError::BadRequest("That goes in a text channel.".into()));
    }
    Ok(access)
}

/// `POST /api/channels/{id}/wow/item` `{"query": "Thunderfury"}`: posts an
/// item card (or, if Wowhead can't be reached, a search link).
pub async fn item_command(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<WowLookupRequest>,
) -> AppResult<Json<Message>> {
    let access = writable_text_channel(&st, channel_id, user.id).await?;
    let query = lookup_query(&req)?;
    let cfg = &st.config;
    let mut embed =
        match tokio::time::timeout(Duration::from_secs(9), wowhead::lookup(&st, &query)).await {
            Ok(Ok(Some(item))) => Ok(wowhead::item_embed(cfg, &item, None)),
            Ok(Ok(None)) => Err(wowhead::search_embed(
                cfg,
                &query,
                "Nothing on Wowhead matched that exactly, so here's a search instead.",
            )),
            Ok(Err(e)) => {
                tracing::info!(error = %e, "item lookup failed");
                Err(wowhead::search_embed(
                    cfg,
                    &query,
                    "Couldn't reach Wowhead just now, so here's a search link instead.",
                ))
            }
            Err(_) => Err(wowhead::search_embed(
                cfg,
                &query,
                "Wowhead took too long to answer, so here's a search link instead.",
            )),
        };
    // Wowhead didn't have it: WoW: Forever's own item data on Battle.net
    // (when the keys are set) may.
    if embed.is_err() && cfg.bnet.is_some() {
        if let Ok(Some(found)) = tokio::time::timeout(
            Duration::from_secs(6),
            crate::armory::item_embed(&st, &query),
        )
        .await
        {
            embed = Ok(found);
        }
    }
    let embed = embed.unwrap_or_else(|e| e);
    let name = display_name(&st, &access, user.id, &user.display_name).await?;
    Ok(Json(
        post_card(&st, &access, user.id, name, "item", embed).await?,
    ))
}

async fn display_name(
    st: &AppState,
    access: &ChannelAccess,
    user_id: i64,
    fallback: &str,
) -> AppResult<String> {
    let nick: Option<String> = sqlx::query_scalar(
        "SELECT nickname FROM guild_members WHERE guild_id = $1 AND user_id = $2",
    )
    .bind(access.channel.guild_id)
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?
    .flatten();
    Ok(nick.unwrap_or_else(|| fallback.to_string()))
}

#[derive(sqlx::FromRow)]
struct FoundCharacter {
    id: i64,
    user_id: i64,
    owner_name: String,
}

/// `POST /api/channels/{id}/wow/char` `{"query": "Thrall"}` (or "Thrall-Realm"):
/// posts the card of a character belonging to someone here.
pub async fn char_command(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<WowLookupRequest>,
) -> AppResult<Json<Message>> {
    let access = writable_text_channel(&st, channel_id, user.id).await?;
    let query = lookup_query(&req)?;
    // "Name-Realm" (TBC / Classic Era), or a first, last or full name
    // ("Thrall", "Stormborn" or "Thrall Stormborn" in WoW: Forever).
    let (name, realm) = match query.split_once('-') {
        Some((n, r)) => (n.trim().to_string(), r.trim().to_string()),
        None => (query.clone(), String::new()),
    };
    // People in this outhouse, or in this DM.
    let people_sql = if access.channel.guild_id == outhouse_common::models::DM_GUILD {
        "SELECT user_id, NULL::TEXT AS nickname FROM dm_participants
         WHERE channel_id = $2 AND $1::BIGINT IS NOT NULL"
    } else {
        "SELECT user_id, nickname FROM guild_members
         WHERE guild_id = $1 AND $2::BIGINT IS NOT NULL"
    };
    let sql = format!(
        "WITH people AS ({people_sql}),
         chars AS (SELECT c.*, lower(CASE WHEN c.last_name <> '' THEN c.name || ' ' || c.last_name
                                          ELSE c.name END) AS full_lc
                   FROM wow_characters c)
         SELECT c.id, c.user_id, COALESCE(pp.nickname, u.display_name) AS owner_name
         FROM chars c
         JOIN people pp ON pp.user_id = c.user_id
         JOIN users u ON u.id = c.user_id
         WHERE ($4 = '' OR lower(c.realm) = lower($4))
           AND (c.full_lc = lower($3)
                OR lower(c.name) = lower($3)
                OR (c.last_name <> '' AND lower(c.last_name) = lower($3))
                OR c.full_lc LIKE lower($3) || '%'
                OR (c.last_name <> '' AND lower(c.last_name) LIKE lower($3) || '%'))
         ORDER BY (c.full_lc = lower($3)) DESC, (lower(c.name) = lower($3)) DESC,
                  (lower(c.last_name) = lower($3)) DESC, c.is_main DESC, c.level DESC
         LIMIT 1"
    );
    let found = sqlx::query_as::<_, FoundCharacter>(&sql)
        .bind(access.channel.guild_id)
        .bind(channel_id)
        .bind(name.replace(['%', '_'], ""))
        .bind(&realm)
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "Nobody here has a character called {name}. Characters are added in Settings → Profile & picture."
            ))
        })?;
    let character = fetch_character(&st.db, found.id).await?;
    let others: Vec<WowCharacter> = characters_of(&st.db, found.user_id)
        .await?
        .into_iter()
        .filter(|c| c.id != character.id)
        .collect();
    let embed = character_embed(&st.config, &character, &found.owner_name, &others);
    let me = display_name(&st, &access, user.id, &user.display_name).await?;
    Ok(Json(
        post_card(&st, &access, user.id, me, "char", embed).await?,
    ))
}

/// "View on Armory" links for a character: (label, address). WoW: Forever
/// characters need both names; TBC and Classic Era ones a realm.
pub fn armory_links(cfg: &crate::state::Config, c: &WowCharacter) -> Vec<(String, String)> {
    let ready = match c.game.as_str() {
        "forever" => !c.last_name.is_empty(),
        "tbc" | "era" => !c.realm.trim().is_empty() && !c.name.is_empty(),
        _ => false,
    };
    if !ready {
        return Vec::new();
    }
    let region = if c.region.is_empty() {
        cfg.armory_region.as_str()
    } else {
        c.region.as_str()
    };
    cfg.armory_links
        .iter()
        .filter(|t| t.game == c.game)
        .map(|t| {
            (
                t.label.clone(),
                fill_armory_template(&t.template, region, &c.name, &c.last_name, &c.realm),
            )
        })
        .collect()
}

pub fn character_embed(
    cfg: &crate::state::Config,
    c: &WowCharacter,
    owner: &str,
    others: &[WowCharacter],
) -> Embed {
    let mut fields = vec![EmbedField {
        name: "Played by".into(),
        value: format!("{owner}{}", if c.is_main { " (main)" } else { " (alt)" }),
        inline: true,
    }];
    if !c.guild_name.is_empty() {
        fields.push(EmbedField {
            name: "Guild".into(),
            value: format!("<{}>", c.guild_name),
            inline: true,
        });
    }
    if let Some(ilvl) = c.item_level {
        fields.push(EmbedField {
            name: "Item level".into(),
            value: ilvl.to_string(),
            inline: true,
        });
    }
    if !c.professions.is_empty() {
        fields.push(EmbedField {
            name: "Professions".into(),
            value: c.professions.join(", "),
            inline: true,
        });
    }
    if !others.is_empty() {
        fields.push(EmbedField {
            name: "Also plays".into(),
            value: others
                .iter()
                .take(6)
                .map(|o| format!("{} ({} {})", o.full_name, o.level, o.class))
                .collect::<Vec<_>>()
                .join(", "),
            inline: false,
        });
    }
    let mut about = vec![if c.race.is_empty() {
        format!("Level {} {}", c.level, c.class)
    } else {
        format!("Level {} {} {}", c.level, c.race, c.class)
    }];
    about.push(game_label(&c.game).to_string());
    if c.game == "forever" {
        if !c.ruleset.is_empty() {
            about.push(c.ruleset.clone());
        }
    } else if !c.realm.is_empty() {
        about.push(c.realm.clone());
    }
    about.retain(|a| !a.is_empty());
    let links = armory_links(cfg, c);
    if !links.is_empty() {
        fields.push(EmbedField {
            name: "Armory".into(),
            value: links
                .iter()
                .map(|(label, url)| format!("{label}: {url}"))
                .collect::<Vec<_>>()
                .join("\n"),
            inline: false,
        });
    }
    let url = if c.armory_url.is_empty() {
        links.first().map(|(_, u)| u.clone()).unwrap_or_default()
    } else {
        c.armory_url.clone()
    };
    Embed {
        url,
        thumbnail_url: (!c.avatar.is_empty()).then(|| c.avatar.clone()),
        kind: "wow-char".into(),
        title: Some(c.full_name.clone()),
        description: Some(about.join(" · ")),
        site_name: Some("Character".into()),
        color: class_colour(&c.class),
        author_name: Some(owner.to_string()),
        footer: Some(match (c.source.as_str(), c.last_synced_at) {
            (_, Some(at)) => format!("From the Armory, {}", at.format("%-d %b %Y")),
            ("bnet", None) => "Imported from Battle.net".into(),
            _ => "Added on Outhouse".into(),
        }),
        fields,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidies_characters() {
        let c = clean_character(
            &CharacterRequest {
                name: "tHRALL".into(),
                last_name: "stormBORN".into(),
                realm: "  Wow   Forever ".into(),
                class: "shaman".into(),
                level: Some(60),
                ruleset: "pvp".into(),
                professions: vec!["mining".into(), "Mining".into(), "Fishing".into()],
                ..Default::default()
            },
            false,
        )
        .unwrap();
        assert_eq!(c.game, "forever");
        assert_eq!(c.name, "Thrall");
        assert_eq!(c.last_name, "Stormborn");
        assert_eq!(c.realm, "");
        assert_eq!(c.ruleset, "PvP");
        assert_eq!(c.class, "Shaman");
        assert_eq!(c.professions, vec!["Mining", "Fishing"]);
        for bad in ["Death Knight", "Monk", "Evoker", "Demon Hunter"] {
            assert!(clean_character(
                &CharacterRequest {
                    name: "Arthas".into(),
                    last_name: "Menethil".into(),
                    class: bad.into(),
                    ..Default::default()
                },
                false
            )
            .is_err());
        }
        let base = CharacterRequest {
            name: "Jaina".into(),
            last_name: "Proudmoore".into(),
            class: "Mage".into(),
            ..Default::default()
        };
        // Forever: a last name is needed (unless keeping an old one without),
        // names are 2-12 letters, level 60 tops, no Jewelcrafting.
        assert!(clean_character(
            &CharacterRequest {
                last_name: "".into(),
                ..base.clone()
            },
            false
        )
        .is_err());
        assert!(clean_character(
            &CharacterRequest {
                last_name: "".into(),
                ..base.clone()
            },
            true
        )
        .is_ok());
        assert!(clean_character(
            &CharacterRequest {
                name: "Has Space".into(),
                ..base.clone()
            },
            false
        )
        .is_err());
        assert!(clean_character(
            &CharacterRequest {
                name: "Thirteenchars".into(),
                ..base.clone()
            },
            false
        )
        .is_err());
        assert!(clean_character(
            &CharacterRequest {
                level: Some(70),
                ..base.clone()
            },
            false
        )
        .is_err());
        assert!(clean_character(
            &CharacterRequest {
                ruleset: "Retail".into(),
                ..base.clone()
            },
            false
        )
        .is_err());
        assert!(clean_character(
            &CharacterRequest {
                professions: vec!["Jewelcrafting".into()],
                ..base.clone()
            },
            false
        )
        .is_err());
        assert!(clean_character(
            &CharacterRequest {
                race: "Blood Elf".into(),
                ..base.clone()
            },
            false
        )
        .is_err());
        // TBC: one name and a realm, level 70, Jewelcrafting, Blood Elves.
        let tbc = clean_character(
            &CharacterRequest {
                game: "tbc".into(),
                name: "kaelthas".into(),
                last_name: "Ignored".into(),
                realm: "Spineshatter".into(),
                class: "Mage".into(),
                race: "blood elf".into(),
                level: Some(70),
                professions: vec!["jewelcrafting".into()],
                ..Default::default()
            },
            false,
        )
        .unwrap();
        assert_eq!(
            (
                tbc.name.as_str(),
                tbc.last_name.as_str(),
                tbc.realm.as_str()
            ),
            ("Kaelthas", "", "Spineshatter")
        );
        assert_eq!(
            (tbc.race.as_str(), tbc.level, tbc.ruleset.as_str()),
            ("Blood Elf", 70, "")
        );
        assert!(clean_character(
            &CharacterRequest {
                game: "era".into(),
                level: Some(61),
                ..base.clone()
            },
            false
        )
        .is_err());
        assert!(clean_character(
            &CharacterRequest {
                game: "retail".into(),
                ..base.clone()
            },
            false
        )
        .is_err());
        // Skyborne: WoW: Forever only, any of the nine classes (Forever's
        // race/class combinations aren't enforced).
        for (race, class) in [
            ("skyborne (alliance)", "Shaman"),
            ("Skyborne (Horde)", "Mage"),
        ] {
            let c = clean_character(
                &CharacterRequest {
                    race: race.into(),
                    class: class.into(),
                    ..base.clone()
                },
                false,
            )
            .unwrap();
            assert!(c.race.starts_with("Skyborne ("));
        }
        assert!(clean_character(
            &CharacterRequest {
                game: "era".into(),
                race: "Skyborne (Horde)".into(),
                ..base
            },
            false
        )
        .is_err());
    }

    #[test]
    fn factions_match_the_sql() {
        use outhouse_common::wow::{ALLIANCE_RACES, HORDE_RACES};
        let (alliance, horde) = CHARACTER_SELECT
            .split_once("THEN 'Alliance'")
            .expect("alliance case");
        for r in ALLIANCE_RACES {
            assert!(alliance.contains(&format!("'{r}'")), "{r}");
        }
        for r in HORDE_RACES {
            assert!(horde.contains(&format!("'{r}'")), "{r}");
        }
    }
}
