//! The soundboard: a library of short clips per outhouse.
//!
//! Playing a clip doesn't mix it into anyone's microphone: the server tells
//! everyone in that voice channel to play it, so each person's own
//! soundboard volume and "mute soundboard" setting apply.
//!
//! Also here: everyone's favourite clips, entrance sounds (a short clip
//! of your own that plays for others when you join their voice channel), and
//! the built-in "Outhouse classics" every outhouse has (files in
//! `web/sounds/builtin`, made by `assets/sounds/synth.py`). Built-ins have
//! ids like `builtin:flush`; managers can hide them per outhouse.

use std::time::{Duration, Instant};

use std::collections::HashSet;

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{EntranceSound, Sound};
use outhouse_common::permissions as p;

use super::messages::attachment_url;
use super::voice::voice_state_of;
use crate::audio::{audio_type, duration_ms};
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, member_ctx};
use crate::state::{AppState, Audience};

pub const MAX_SOUND_BYTES: usize = 1024 * 1024;
/// Soundboard clips: up to 10 seconds (a little slack for rounding).
const MAX_SOUND_MS: u32 = 10_250;
pub const MAX_ENTRANCE_BYTES: usize = 200 * 1024;
const MAX_ENTRANCE_MS: u32 = 5_250;
const MAX_SOUNDS_PER_GUILD: i64 = 60;
const COOLDOWN: Duration = Duration::from_secs(2);
const KEY_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const MAX_FAVOURITES: i64 = 100;

/// One of the built-in sounds.
pub struct Builtin {
    pub key: &'static str,
    pub name: &'static str,
    pub emoji: &'static str,
    pub duration_ms: u32,
}

/// The "Outhouse classics", in the order the soundboard shows them.
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        key: "flush",
        name: "Flush",
        emoji: "🚽",
        duration_ms: 3_936,
    },
    Builtin {
        key: "strain",
        name: "Strain",
        emoji: "😣",
        duration_ms: 3_216,
    },
    Builtin {
        key: "plop",
        name: "Plop",
        emoji: "💩",
        duration_ms: 576,
    },
    Builtin {
        key: "door",
        name: "Door creak",
        emoji: "🚪",
        duration_ms: 2_328,
    },
    Builtin {
        key: "fly",
        name: "Blowfly",
        emoji: "🪰",
        duration_ms: 2_640,
    },
];
const BUILTIN_PREFIX: &str = "builtin:";
/// The classics are mastered loud (-16 LUFS); this sits them next to uploads.
const BUILTIN_VOLUME: f32 = 0.8;

pub fn builtin(key: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.key == key)
}

pub fn builtin_url(key: &str) -> String {
    format!("/sounds/builtin/{key}.mp3")
}

fn builtin_sound(b: &Builtin, guild_id: i64, favourite: bool, hidden: bool) -> Sound {
    Sound {
        id: format!("{BUILTIN_PREFIX}{}", b.key),
        guild_id,
        name: b.name.into(),
        emoji: b.emoji.into(),
        url: builtin_url(b.key),
        volume: BUILTIN_VOLUME,
        favourite,
        builtin: true,
        hidden,
        duration_ms: Some(b.duration_ms as i32),
    }
}

/// A sound id from a URL: a built-in or an uploaded clip.
#[derive(Clone, Copy)]
enum SoundRef {
    Builtin(&'static Builtin),
    Custom(i64),
}

fn parse_ref(id: &str) -> AppResult<SoundRef> {
    if let Some(key) = id.strip_prefix(BUILTIN_PREFIX) {
        return builtin(key)
            .map(SoundRef::Builtin)
            .ok_or(AppError::NotFound);
    }
    id.parse::<i64>()
        .map(SoundRef::Custom)
        .map_err(|_| AppError::NotFound)
}

async fn hidden_builtins(st: &AppState, guild_id: i64) -> AppResult<HashSet<String>> {
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT sound_key FROM hidden_builtin_sounds WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_all(&st.db)
            .await?;
    Ok(keys.into_iter().collect())
}

async fn builtin_favourites(st: &AppState, user_id: i64) -> AppResult<HashSet<String>> {
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT sound_key FROM builtin_sound_favourites WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&st.db)
            .await?;
    Ok(keys.into_iter().collect())
}

#[derive(sqlx::FromRow)]
struct SoundRow {
    id: i64,
    guild_id: i64,
    name: String,
    emoji: String,
    storage_key: String,
    volume: f32,
    favourite: bool,
    duration_ms: Option<i32>,
}

impl From<SoundRow> for Sound {
    fn from(r: SoundRow) -> Self {
        Sound {
            url: attachment_url(&r.storage_key, "sound"),
            id: r.id.to_string(),
            guild_id: r.guild_id,
            name: r.name,
            emoji: r.emoji,
            volume: r.volume,
            favourite: r.favourite,
            builtin: false,
            hidden: false,
            duration_ms: r.duration_ms,
        }
    }
}

const SOUND_SELECT: &str = "SELECT id, guild_id, name, emoji, storage_key, volume, false AS favourite, duration_ms FROM sounds";
const SOUND_SELECT_FAV: &str = "SELECT s.id, s.guild_id, s.name, s.emoji, s.storage_key, s.volume,
        (f.user_id IS NOT NULL) AS favourite, s.duration_ms
 FROM sounds s
 LEFT JOIN sound_favourites f ON f.sound_id = s.id AND f.user_id = $1";

async fn fetch_sound(st: &AppState, sound_id: i64) -> AppResult<SoundRow> {
    let sql = format!("{SOUND_SELECT} WHERE id = $1");
    sqlx::query_as::<_, SoundRow>(&sql)
        .bind(sound_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)
}

/// This outhouse's soundboard: the built-in classics first (hidden ones only
/// for people who manage sounds, so they can show them again), then uploads.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Sound>>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let manager = ctx.guild_perms() & p::MANAGE_SOUNDS != 0;
    let hidden = hidden_builtins(&st, guild_id).await?;
    let favs = builtin_favourites(&st, user.id).await?;
    let mut out: Vec<Sound> = BUILTINS
        .iter()
        .filter(|b| manager || !hidden.contains(b.key))
        .map(|b| builtin_sound(b, guild_id, favs.contains(b.key), hidden.contains(b.key)))
        .collect();
    let sql = format!("{SOUND_SELECT_FAV} WHERE s.guild_id = $2 ORDER BY lower(s.name)");
    let rows = sqlx::query_as::<_, SoundRow>(&sql)
        .bind(user.id)
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    out.extend(rows.into_iter().map(Sound::from));
    Ok(Json(out))
}

/// The built-in sounds (for choosing an entrance sound).
pub async fn list_builtins(AuthUser(_user): AuthUser) -> Json<Vec<Sound>> {
    Json(
        BUILTINS
            .iter()
            .map(|b| builtin_sound(b, 0, false, false))
            .collect(),
    )
}

/// Uploaded clips from every outhouse you're in (you can play them in any
/// voice channel where you're allowed to use the soundboard, like Discord).
pub async fn list_mine(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<Sound>>> {
    let sql = format!(
        "{SOUND_SELECT_FAV}
         JOIN guild_members m ON m.guild_id = s.guild_id AND m.user_id = $1
         ORDER BY s.guild_id, lower(s.name)"
    );
    let rows = sqlx::query_as::<_, SoundRow>(&sql)
        .bind(user.id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.into_iter().map(Sound::from).collect()))
}

/// Hides a built-in sound in this outhouse (`PUT`) or shows it again (`DELETE`).
pub async fn hide_builtin(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, key)): Path<(i64, String)>,
) -> AppResult<StatusCode> {
    set_builtin_hidden(&st, user.id, guild_id, &key, true).await
}

pub async fn show_builtin(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, key)): Path<(i64, String)>,
) -> AppResult<StatusCode> {
    set_builtin_hidden(&st, user.id, guild_id, &key, false).await
}

async fn set_builtin_hidden(
    st: &AppState,
    user_id: i64,
    guild_id: i64,
    key: &str,
    hide: bool,
) -> AppResult<StatusCode> {
    member_ctx(&st.db, guild_id, user_id)
        .await?
        .require(p::MANAGE_SOUNDS)?;
    let b = builtin(key).ok_or(AppError::NotFound)?;
    if hide {
        sqlx::query(
            "INSERT INTO hidden_builtin_sounds (guild_id, sound_key, hidden_by) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(guild_id)
        .bind(b.key)
        .bind(user_id)
        .execute(&st.db)
        .await?;
    } else {
        sqlx::query("DELETE FROM hidden_builtin_sounds WHERE guild_id = $1 AND sound_key = $2")
            .bind(guild_id)
            .bind(b.key)
            .execute(&st.db)
            .await?;
    }
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::SoundsChanged { guild_id },
    );
    Ok(StatusCode::NO_CONTENT)
}

fn bad(e: impl std::fmt::Display) -> AppError {
    AppError::BadRequest(format!("Upload failed: {e}"))
}

/// Multipart form: `name`, optional `emoji` and `volume` (0.1–1.0), and `file`.
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    mut form: Multipart,
) -> AppResult<Json<Sound>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_SOUNDS)?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sounds WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    if count >= MAX_SOUNDS_PER_GUILD {
        return Err(AppError::BadRequest(format!(
            "The soundboard is full ({MAX_SOUNDS_PER_GUILD} sounds). Delete one first."
        )));
    }

    let (mut name, mut emoji, mut volume) = (String::new(), String::new(), 1.0f32);
    let mut file: Option<(String, axum::body::Bytes)> = None;
    while let Some(field) = form.next_field().await.map_err(bad)? {
        let field_name = field.name().unwrap_or_default().to_string();
        match field_name.as_str() {
            "name" => name = field.text().await.map_err(bad)?,
            "emoji" => emoji = field.text().await.map_err(bad)?,
            "volume" => {
                volume = field
                    .text()
                    .await
                    .map_err(bad)?
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(1.0)
            }
            "file" => {
                let filename = field.file_name().unwrap_or("sound").to_string();
                let data = field.bytes().await.map_err(bad)?;
                file = Some((filename, data));
            }
            _ => {}
        }
    }
    let name: String = name.trim().chars().take(32).collect();
    if name.is_empty() {
        return Err(AppError::BadRequest("Give the sound a name.".into()));
    }
    let emoji: String = emoji.trim().chars().take(8).collect();
    let volume = if volume.is_finite() {
        volume.clamp(0.1, 1.0)
    } else {
        1.0
    };
    let (filename, data) =
        file.ok_or_else(|| AppError::BadRequest("Choose a sound file.".into()))?;
    let content_type = audio_type(&filename)
        .ok_or_else(|| AppError::BadRequest("Sounds can be MP3, OGG or WAV files.".into()))?;
    if data.len() > MAX_SOUND_BYTES {
        return Err(AppError::BadRequest(
            "Sounds can be up to 1 MB (about 5 seconds is plenty).".into(),
        ));
    }
    let ms = check_length(&data, content_type, MAX_SOUND_MS)?;

    let key = save_file(&st, &data).await?;

    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO sounds (id, guild_id, name, emoji, storage_key, content_type, size, volume, creator_id, duration_ms)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(guild_id)
    .bind(&name)
    .bind(&emoji)
    .bind(&key)
    .bind(content_type)
    .bind(data.len() as i64)
    .bind(volume)
    .bind(user.id)
    .bind(ms as i32)
    .execute(&st.db)
    .await?;

    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::SoundsChanged { guild_id },
    );
    crate::audit::Entry::new(guild_id, user.id, "sound_create")
        .target("sound", id, format!("{emoji} {name}").trim().to_string())
        .details(format!("{:.1} s", ms as f64 / 1000.0))
        .save(&st)
        .await;
    Ok(Json(fetch_sound(&st, id).await?.into()))
}

/// The id of an uploaded clip; built-ins get a friendly "can't" instead.
fn custom_id(id: &str, what: &str) -> AppResult<i64> {
    match parse_ref(id)? {
        SoundRef::Custom(id) => Ok(id),
        SoundRef::Builtin(_) => Err(AppError::BadRequest(format!(
            "Built-in sounds can't be {what}, but you can hide them."
        ))),
    }
}

#[derive(serde::Deserialize)]
pub struct EditSound {
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub volume: Option<f32>,
}

/// Rename a clip, change its emoji or volume.
pub async fn edit(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<EditSound>,
) -> AppResult<Json<Sound>> {
    let sound_id = custom_id(&id, "changed")?;
    let sound = fetch_sound(&st, sound_id).await?;
    member_ctx(&st.db, sound.guild_id, user.id)
        .await?
        .require(p::MANAGE_SOUNDS)?;
    let name = match body.name {
        Some(n) => {
            let n: String = n.trim().chars().take(32).collect();
            if n.is_empty() {
                return Err(AppError::BadRequest("Give the sound a name.".into()));
            }
            n
        }
        None => sound.name.clone(),
    };
    let emoji = body
        .emoji
        .map(|e| e.trim().chars().take(8).collect::<String>())
        .unwrap_or(sound.emoji.clone());
    let volume = body
        .volume
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.1, 1.0))
        .unwrap_or(sound.volume);
    sqlx::query("UPDATE sounds SET name = $2, emoji = $3, volume = $4 WHERE id = $1")
        .bind(sound_id)
        .bind(&name)
        .bind(&emoji)
        .bind(volume)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::Guild(sound.guild_id),
        ServerEvent::SoundsChanged {
            guild_id: sound.guild_id,
        },
    );
    Ok(Json(fetch_sound(&st, sound_id).await?.into()))
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let sound_id = custom_id(&id, "deleted")?;
    let sound = fetch_sound(&st, sound_id).await?;
    member_ctx(&st.db, sound.guild_id, user.id)
        .await?
        .require(p::MANAGE_SOUNDS)?;
    sqlx::query("DELETE FROM sounds WHERE id = $1")
        .bind(sound_id)
        .execute(&st.db)
        .await?;
    let _ = tokio::fs::remove_file(st.config.upload_dir.join(&sound.storage_key)).await;
    crate::audit::Entry::new(sound.guild_id, user.id, "sound_delete")
        .target(
            "sound",
            sound_id,
            format!("{} {}", sound.emoji, sound.name).trim().to_string(),
        )
        .save(&st)
        .await;
    st.dispatch(
        Audience::Guild(sound.guild_id),
        ServerEvent::SoundsChanged {
            guild_id: sound.guild_id,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Plays a clip in your voice channel. Built-ins work everywhere (unless the
/// outhouse you're in voice in has hidden them); uploaded clips can come from
/// any outhouse you're a member of.
pub async fn play(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let custom = match parse_ref(&id)? {
        SoundRef::Builtin(b) => Err(b),
        SoundRef::Custom(sound_id) => {
            let sound = fetch_sound(&st, sound_id).await?;
            // Not a member of the clip's outhouse: as if it didn't exist.
            member_ctx(&st.db, sound.guild_id, user.id)
                .await
                .map_err(|_| AppError::NotFound)?;
            Ok(sound)
        }
    };
    let voice = voice_state_of(&st, user.id)
        .ok_or_else(|| AppError::BadRequest("Join a voice channel first.".into()))?;
    let access = channel_access(&st.db, voice.channel_id, user.id).await?;
    access.require(p::USE_SOUNDBOARD)?;
    let (sound_id, url, volume) = match custom {
        Ok(c) => (
            c.id.to_string(),
            attachment_url(&c.storage_key, "sound"),
            c.volume,
        ),
        Err(b) => {
            if hidden_builtins(&st, voice.guild_id).await?.contains(b.key) {
                return Err(AppError::BadRequest(
                    "That sound is hidden in this outhouse.".into(),
                ));
            }
            (
                format!("{BUILTIN_PREFIX}{}", b.key),
                builtin_url(b.key),
                BUILTIN_VOLUME,
            )
        }
    };
    {
        let mut cooldowns = st.sound_cooldowns.lock().expect("cooldown lock");
        let now = Instant::now();
        if let Some(last) = cooldowns.get(&user.id) {
            if now.duration_since(*last) < COOLDOWN {
                return Err(AppError::BadRequest("Easy on the soundboard!".into()));
            }
        }
        cooldowns.insert(user.id, now);
    }
    st.dispatch(
        Audience::Channel {
            guild_id: voice.guild_id,
            channel_id: voice.channel_id,
        },
        ServerEvent::SoundboardPlay {
            guild_id: voice.guild_id,
            channel_id: voice.channel_id,
            user_id: user.id,
            sound_id,
            url,
            volume,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// The clip's length, or a friendly error if it's too long or unreadable.
fn check_length(data: &[u8], content_type: &str, max_ms: u32) -> AppResult<u32> {
    let ms = duration_ms(data, content_type).ok_or_else(|| {
        AppError::BadRequest(
            "Couldn't read that sound file. Try saving it again as MP3, OGG or WAV.".into(),
        )
    })?;
    if ms > max_ms {
        return Err(AppError::BadRequest(format!(
            "That sound is {:.1} seconds long; the limit is {} seconds.",
            ms as f64 / 1000.0,
            max_ms / 1000
        )));
    }
    Ok(ms)
}

async fn save_file(st: &AppState, data: &[u8]) -> AppResult<String> {
    let key = auth::random_code(24, KEY_ALPHABET);
    tokio::fs::create_dir_all(&st.config.upload_dir)
        .await
        .map_err(|e| anyhow::anyhow!("creating upload folder: {e}"))?;
    tokio::fs::write(st.config.upload_dir.join(&key), data)
        .await
        .map_err(|e| anyhow::anyhow!("saving sound: {e}"))?;
    Ok(key)
}

// ---------- favourites ----------

pub async fn add_favourite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let target = parse_ref(&id)?;
    if let SoundRef::Custom(sound_id) = target {
        let sound = fetch_sound(&st, sound_id).await?;
        member_ctx(&st.db, sound.guild_id, user.id).await?;
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM sound_favourites WHERE user_id = $1)
              + (SELECT count(*) FROM builtin_sound_favourites WHERE user_id = $1)",
    )
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if count >= MAX_FAVOURITES {
        return Err(AppError::BadRequest(format!(
            "That's a lot of favourites ({MAX_FAVOURITES}). Remove one first."
        )));
    }
    match target {
        SoundRef::Custom(sound_id) => sqlx::query(
            "INSERT INTO sound_favourites (user_id, sound_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(user.id)
        .bind(sound_id),
        SoundRef::Builtin(b) => sqlx::query(
            "INSERT INTO builtin_sound_favourites (user_id, sound_key) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(user.id)
        .bind(b.key),
    }
    .execute(&st.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_favourite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    match parse_ref(&id)? {
        SoundRef::Custom(sound_id) => {
            sqlx::query("DELETE FROM sound_favourites WHERE user_id = $1 AND sound_id = $2")
                .bind(user.id)
                .bind(sound_id)
        }
        SoundRef::Builtin(b) => sqlx::query(
            "DELETE FROM builtin_sound_favourites WHERE user_id = $1 AND sound_key = $2",
        )
        .bind(user.id)
        .bind(b.key),
    }
    .execute(&st.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- entrance sounds ----------

/// Your entrance sound is either a file you uploaded or one of the built-ins.
#[derive(sqlx::FromRow)]
struct EntranceRow {
    storage_key: Option<String>,
    builtin: Option<String>,
    duration_ms: i32,
}

impl EntranceRow {
    fn url(&self) -> Option<String> {
        match (&self.builtin, &self.storage_key) {
            (Some(key), _) => builtin(key).map(|b| builtin_url(b.key)),
            (None, Some(key)) => Some(attachment_url(key, "entrance")),
            (None, None) => None,
        }
    }
}

async fn entrance_of(st: &AppState, user_id: i64) -> AppResult<Option<EntranceRow>> {
    Ok(sqlx::query_as::<_, EntranceRow>(
        "SELECT storage_key, builtin, duration_ms FROM entrance_sounds WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?)
}

/// The URL of someone's entrance sound, if they have one.
pub(crate) async fn entrance_url(st: &AppState, user_id: i64) -> AppResult<Option<String>> {
    Ok(entrance_of(st, user_id).await?.and_then(|r| r.url()))
}

fn entrance_json(row: EntranceRow) -> Option<EntranceSound> {
    Some(EntranceSound {
        url: row.url()?,
        duration_ms: row.duration_ms,
        builtin: row.builtin,
    })
}

/// Your entrance sound (null if you haven't set one).
pub async fn get_entrance(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Option<EntranceSound>>> {
    Ok(Json(
        entrance_of(&st, user.id).await?.and_then(entrance_json),
    ))
}

/// Removes an uploaded entrance file that's no longer used.
async fn drop_entrance_file(st: &AppState, old: Option<EntranceRow>) -> AppResult<()> {
    if let Some(key) = old.and_then(|o| o.storage_key) {
        sqlx::query("DELETE FROM profile_files WHERE storage_key = $1")
            .bind(&key)
            .execute(&st.db)
            .await?;
        let _ = tokio::fs::remove_file(st.config.upload_dir.join(&key)).await;
    }
    Ok(())
}

#[derive(serde::Deserialize)]
pub struct BuiltinEntrance {
    pub builtin: String,
}

/// JSON `{"builtin": "door"}`: use one of the built-in sounds as your entrance.
pub async fn set_entrance_builtin(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<BuiltinEntrance>,
) -> AppResult<Json<EntranceSound>> {
    let key = body.builtin.trim();
    let b = builtin(key.strip_prefix(BUILTIN_PREFIX).unwrap_or(key))
        .ok_or_else(|| AppError::BadRequest("There's no built-in sound by that name.".into()))?;
    if b.duration_ms > MAX_ENTRANCE_MS {
        return Err(AppError::BadRequest(format!(
            "{} is too long for an entrance sound (the limit is {} seconds).",
            b.name,
            MAX_ENTRANCE_MS / 1000
        )));
    }
    let old = entrance_of(&st, user.id).await?;
    // Unlink the old file first, so removing it doesn't cascade to this row.
    sqlx::query(
        "INSERT INTO entrance_sounds (user_id, storage_key, builtin, duration_ms) VALUES ($1, NULL, $2, $3)
         ON CONFLICT (user_id) DO UPDATE SET storage_key = NULL, builtin = $2, duration_ms = $3, created_at = now()",
    )
    .bind(user.id)
    .bind(b.key)
    .bind(b.duration_ms as i32)
    .execute(&st.db)
    .await?;
    drop_entrance_file(&st, old).await?;
    Ok(Json(EntranceSound {
        url: builtin_url(b.key),
        duration_ms: b.duration_ms as i32,
        builtin: Some(b.key.into()),
    }))
}

/// Multipart form with `file`: MP3, OGG or WAV, up to 5 seconds and 200 KB.
pub async fn set_entrance(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    mut form: Multipart,
) -> AppResult<Json<EntranceSound>> {
    let mut file: Option<(String, axum::body::Bytes)> = None;
    while let Some(field) = form.next_field().await.map_err(bad)? {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("sound").to_string();
            file = Some((filename, field.bytes().await.map_err(bad)?));
        }
    }
    let (filename, data) =
        file.ok_or_else(|| AppError::BadRequest("Choose a sound file.".into()))?;
    let content_type = audio_type(&filename)
        .ok_or_else(|| AppError::BadRequest("Sounds can be MP3, OGG or WAV files.".into()))?;
    if data.len() > MAX_ENTRANCE_BYTES {
        return Err(AppError::BadRequest(
            "Entrance sounds can be up to 200 KB.".into(),
        ));
    }
    let ms = check_length(&data, content_type, MAX_ENTRANCE_MS)?;
    let key = save_file(&st, &data).await?;
    let old = entrance_of(&st, user.id).await?;

    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO profile_files (storage_key, user_id, filename, content_type)
         VALUES ($1, $2, 'entrance', $3)",
    )
    .bind(&key)
    .bind(user.id)
    .bind(content_type)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO entrance_sounds (user_id, storage_key, builtin, duration_ms) VALUES ($1, $2, NULL, $3)
         ON CONFLICT (user_id) DO UPDATE SET storage_key = $2, builtin = NULL, duration_ms = $3, created_at = now()",
    )
    .bind(user.id)
    .bind(&key)
    .bind(ms as i32)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    drop_entrance_file(&st, old).await?;
    Ok(Json(EntranceSound {
        url: attachment_url(&key, "entrance"),
        duration_ms: ms as i32,
        builtin: None,
    }))
}

pub async fn delete_entrance(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<StatusCode> {
    let old = entrance_of(&st, user.id).await?;
    sqlx::query("DELETE FROM entrance_sounds WHERE user_id = $1")
        .bind(user.id)
        .execute(&st.db)
        .await?;
    drop_entrance_file(&st, old).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in files exist, read as MP3, fit the soundboard limits and
    /// match the lengths listed above.
    #[test]
    fn builtin_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/sounds/builtin");
        for b in BUILTINS {
            let data = std::fs::read(dir.join(format!("{}.mp3", b.key)))
                .unwrap_or_else(|e| panic!("{}.mp3: {e}", b.key));
            assert!(data.len() <= MAX_SOUND_BYTES, "{} is too big", b.key);
            let ms = duration_ms(&data, "audio/mpeg").expect("readable mp3");
            assert!(ms <= MAX_SOUND_MS, "{} is {ms} ms", b.key);
            assert!(
                ms.abs_diff(b.duration_ms) < 120,
                "{}: file is {ms} ms, listed as {} ms",
                b.key,
                b.duration_ms
            );
            assert_eq!(
                parse_ref(&format!("builtin:{}", b.key))
                    .ok()
                    .map(|r| matches!(r, SoundRef::Builtin(_))),
                Some(true)
            );
        }
        assert!(parse_ref("builtin:nope").is_err());
        assert!(parse_ref("12x").is_err());
        assert!(matches!(parse_ref("42"), Ok(SoundRef::Custom(42))));
    }
}
