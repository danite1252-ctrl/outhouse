//! Your left rail: the order of your outhouses and the folders you put them
//! in (name, colour, collapsed). Private to you, the same on every device.
//!
//! `GET/PUT /api/users/@me/guild-layout` with
//! `{"items": [{"type": "guild", "id": "1"},
//!             {"type": "folder", "id": "2", "name": "Raids", "color": "ember",
//!              "collapsed": false, "guild_ids": ["3", "4"]}]}`.
//! Outhouses you're not in are dropped, and ones the layout doesn't mention
//! (you joined since) go at the end, outside any folder.

use std::collections::{HashMap, HashSet};

use axum::extract::State;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::id::{as_string, vec_as_string};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

pub const MAX_FOLDERS: usize = 50;
pub const FOLDER_NAME_MAX: usize = 32;
/// The swatches the client offers (NULL/absent: ember).
pub const COLOURS: &[&str] = &[
    "ember", "glow", "red", "pink", "purple", "blue", "teal", "green", "grey",
];
/// More than anyone could be in (keeps a silly request small).
const MAX_ITEMS: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RailItem {
    Guild {
        #[serde(with = "as_string")]
        id: i64,
    },
    Folder {
        /// Absent or unknown for a new folder (the server picks one).
        #[serde(default)]
        id: Option<String>,
        name: String,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        collapsed: bool,
        #[serde(default, with = "vec_as_string")]
        guild_ids: Vec<i64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Layout {
    pub items: Vec<RailItem>,
}

#[derive(sqlx::FromRow)]
struct FolderRow {
    id: i64,
    name: String,
    color: Option<String>,
    collapsed: bool,
    position: i32,
}

#[derive(sqlx::FromRow)]
struct PosRow {
    guild_id: i64,
    folder_id: Option<i64>,
    position: i32,
}

/// Your outhouses, oldest membership first.
async fn my_guilds(st: &AppState, user_id: i64) -> AppResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT guild_id FROM guild_members WHERE user_id = $1 ORDER BY joined_at, guild_id",
    )
    .bind(user_id)
    .fetch_all(&st.db)
    .await?)
}

/// Your layout as stored, tidied against the outhouses you're in now.
pub async fn load(st: &AppState, user_id: i64) -> AppResult<Layout> {
    let guilds = my_guilds(st, user_id).await?;
    let folders = sqlx::query_as::<_, FolderRow>(
        "SELECT id, name, color, collapsed, position FROM guild_folders
         WHERE user_id = $1 ORDER BY position, id",
    )
    .bind(user_id)
    .fetch_all(&st.db)
    .await?;
    let positions = sqlx::query_as::<_, PosRow>(
        "SELECT guild_id, folder_id, position FROM guild_rail_positions
         WHERE user_id = $1 ORDER BY position, guild_id",
    )
    .bind(user_id)
    .fetch_all(&st.db)
    .await?;
    let member: HashSet<i64> = guilds.iter().copied().collect();
    let folder_ids: HashSet<i64> = folders.iter().map(|f| f.id).collect();
    let mut in_folder: HashMap<i64, Vec<i64>> = HashMap::new();
    // (position, is_folder, id) for the top level.
    let mut top: Vec<(i32, u8, i64)> = folders.iter().map(|f| (f.position, 0, f.id)).collect();
    let mut placed = HashSet::new();
    for p in &positions {
        if !member.contains(&p.guild_id) || !placed.insert(p.guild_id) {
            continue;
        }
        match p.folder_id.filter(|f| folder_ids.contains(f)) {
            Some(f) => in_folder.entry(f).or_default().push(p.guild_id),
            None => top.push((p.position, 1, p.guild_id)),
        }
    }
    top.sort();
    let by_id: HashMap<i64, &FolderRow> = folders.iter().map(|f| (f.id, f)).collect();
    let mut items: Vec<RailItem> = top
        .into_iter()
        .map(|(_, kind, id)| {
            if kind == 1 {
                RailItem::Guild { id }
            } else {
                let f = by_id[&id];
                RailItem::Folder {
                    id: Some(f.id.to_string()),
                    name: f.name.clone(),
                    color: f.color.clone(),
                    collapsed: f.collapsed,
                    guild_ids: in_folder.remove(&id).unwrap_or_default(),
                }
            }
        })
        .collect();
    // Joined since: at the end, outside folders.
    for g in guilds {
        if !placed.contains(&g) {
            items.push(RailItem::Guild { id: g });
        }
    }
    Ok(Layout { items })
}

/// `GET /api/users/@me/guild-layout`
pub async fn get(State(st): State<AppState>, AuthUser(user): AuthUser) -> AppResult<Json<Layout>> {
    Ok(Json(load(&st, user.id).await?))
}

fn clean_folder_name(raw: &str) -> AppResult<String> {
    let name = raw.trim();
    let n = name.chars().count();
    if n == 0 || n > FOLDER_NAME_MAX || name.chars().any(|c| c.is_control()) {
        return Err(AppError::BadRequest(format!(
            "Folder names are 1–{FOLDER_NAME_MAX} characters."
        )));
    }
    Ok(name.to_string())
}

/// Checks a layout from the client. Outhouses you're not in are dropped
/// (and repeats after the first); folders and their names are checked.
pub fn validate(req: &Layout, member: &HashSet<i64>) -> AppResult<Layout> {
    if req.items.len() > MAX_ITEMS {
        return Err(AppError::BadRequest(
            "That's too many things for the rail.".into(),
        ));
    }
    let folders = req
        .items
        .iter()
        .filter(|i| matches!(i, RailItem::Folder { .. }))
        .count();
    if folders > MAX_FOLDERS {
        return Err(AppError::BadRequest(format!(
            "You can have up to {MAX_FOLDERS} folders."
        )));
    }
    let mut seen = HashSet::new();
    let mut keep = |id: &i64| member.contains(id) && seen.insert(*id);
    let mut items = Vec::new();
    for item in &req.items {
        match item {
            RailItem::Guild { id } => {
                if keep(id) {
                    items.push(RailItem::Guild { id: *id });
                }
            }
            RailItem::Folder {
                id,
                name,
                color,
                collapsed,
                guild_ids,
            } => {
                if guild_ids.len() > MAX_ITEMS {
                    return Err(AppError::BadRequest("That folder's too full.".into()));
                }
                let color = match color.as_deref().map(str::trim) {
                    None | Some("") => None,
                    Some(c) if COLOURS.contains(&c) => Some(c.to_string()),
                    Some(_) => return Err(AppError::BadRequest("Unknown folder colour.".into())),
                };
                items.push(RailItem::Folder {
                    id: id.clone(),
                    name: clean_folder_name(name)?,
                    color,
                    collapsed: *collapsed,
                    guild_ids: guild_ids.iter().filter(|g| keep(g)).copied().collect(),
                });
            }
        }
    }
    Ok(Layout { items })
}

/// `PUT /api/users/@me/guild-layout`: replaces your whole layout.
pub async fn put(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<Layout>,
) -> AppResult<Json<Layout>> {
    let member: HashSet<i64> = my_guilds(&st, user.id).await?.into_iter().collect();
    let layout = validate(&req, &member)?;
    let existing: HashSet<i64> =
        sqlx::query_scalar("SELECT id FROM guild_folders WHERE user_id = $1")
            .bind(user.id)
            .fetch_all(&st.db)
            .await?
            .into_iter()
            .collect();
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM guild_rail_positions WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM guild_folders WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let mut used = HashSet::new();
    for (pos, item) in layout.items.iter().enumerate() {
        match item {
            RailItem::Guild { id } => {
                // Only while you're still in it (you may have left a moment ago).
                sqlx::query(
                    "INSERT INTO guild_rail_positions (user_id, guild_id, folder_id, position)
                     SELECT $1, $2, NULL, $3
                     WHERE EXISTS (SELECT 1 FROM guild_members WHERE guild_id = $2 AND user_id = $1)",
                )
                .bind(user.id)
                .bind(id)
                .bind(pos as i32)
                .execute(&mut *tx)
                .await?;
            }
            RailItem::Folder {
                id,
                name,
                color,
                collapsed,
                guild_ids,
            } => {
                // Keep a folder's ID if it's one of yours (so other devices
                // keep track of it); otherwise it's new.
                let folder_id = id
                    .as_deref()
                    .and_then(|s| s.parse::<i64>().ok())
                    .filter(|f| existing.contains(f) && used.insert(*f))
                    .unwrap_or_else(|| st.ids.next_id());
                used.insert(folder_id);
                sqlx::query(
                    "INSERT INTO guild_folders (id, user_id, name, color, collapsed, position)
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(folder_id)
                .bind(user.id)
                .bind(name)
                .bind(color)
                .bind(collapsed)
                .bind(pos as i32)
                .execute(&mut *tx)
                .await?;
                for (i, g) in guild_ids.iter().enumerate() {
                    sqlx::query(
                        "INSERT INTO guild_rail_positions (user_id, guild_id, folder_id, position)
                         SELECT $1, $2, $3, $4
                         WHERE EXISTS (SELECT 1 FROM guild_members WHERE guild_id = $2 AND user_id = $1)",
                    )
                    .bind(user.id)
                    .bind(g)
                    .bind(folder_id)
                    .bind(i as i32)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
    }
    tx.commit().await?;
    st.dispatch(Audience::User(user.id), ServerEvent::GuildLayoutChanged);
    Ok(Json(load(&st, user.id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, color: Option<&str>, guilds: &[i64]) -> RailItem {
        RailItem::Folder {
            id: None,
            name: name.into(),
            color: color.map(Into::into),
            collapsed: false,
            guild_ids: guilds.to_vec(),
        }
    }

    #[test]
    fn drops_strangers_and_repeats() {
        let member: HashSet<i64> = [1, 2, 3].into_iter().collect();
        let req = Layout {
            items: vec![
                RailItem::Guild { id: 9 },
                folder("Raids", Some("blue"), &[1, 9, 1]),
                RailItem::Guild { id: 1 },
                RailItem::Guild { id: 2 },
            ],
        };
        let out = validate(&req, &member).unwrap();
        assert_eq!(
            out.items,
            vec![
                folder("Raids", Some("blue"), &[1]),
                RailItem::Guild { id: 2 }
            ]
        );
    }

    #[test]
    fn checks_folders() {
        let member = HashSet::new();
        let bad_name = Layout {
            items: vec![folder("  ", None, &[])],
        };
        assert!(validate(&bad_name, &member).is_err());
        let long = Layout {
            items: vec![folder(&"x".repeat(33), None, &[])],
        };
        assert!(validate(&long, &member).is_err());
        let colour = Layout {
            items: vec![folder("A", Some("#123456"), &[])],
        };
        assert!(validate(&colour, &member).is_err());
        let many = Layout {
            items: (0..51)
                .map(|i| folder(&format!("F{i}"), None, &[]))
                .collect(),
        };
        assert!(validate(&many, &member).is_err());
        let fifty = Layout {
            items: (0..50)
                .map(|i| folder(&format!("F{i}"), None, &[]))
                .collect(),
        };
        assert!(validate(&fifty, &member).is_ok());
    }

    #[test]
    fn parses_client_json() {
        let l: Layout = serde_json::from_str(
            r#"{"items":[{"type":"guild","id":"5"},{"type":"folder","name":"A","guild_ids":["6"]}]}"#,
        )
        .unwrap();
        assert_eq!(l.items.len(), 2);
        assert!(serde_json::from_str::<Layout>(r#"{"items":[{"type":"nope"}]}"#).is_err());
    }
}
