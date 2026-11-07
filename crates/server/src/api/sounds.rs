//! The soundboard: a library of short clips per outhouse.
//!
//! Playing a clip doesn't mix it into anyone's microphone: the server tells
//! everyone in that voice channel to play it, so each person's own
//! soundboard volume and "mute soundboard" setting apply.
//!
//! Also here: everyone's favourite clips, and entrance sounds (a short clip
//! of your own that plays for others when you join their voice channel).

use std::time::{Duration, Instant};

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

#[derive(sqlx::FromRow)]
struct SoundRow {
    id: i64,
    guild_id: i64,
    name: String,
    emoji: String,
    storage_key: String,
    volume: f32,
    favourite: bool,
}

impl From<SoundRow> for Sound {
    fn from(r: SoundRow) -> Self {
        Sound {
            url: attachment_url(&r.storage_key, "sound"),
            id: r.id,
            guild_id: r.guild_id,
            name: r.name,
            emoji: r.emoji,
            volume: r.volume,
            favourite: r.favourite,
        }
    }
}

const SOUND_SELECT: &str =
    "SELECT id, guild_id, name, emoji, storage_key, volume, false AS favourite FROM sounds";

async fn fetch_sound(st: &AppState, sound_id: i64) -> AppResult<SoundRow> {
    let sql = format!("{SOUND_SELECT} WHERE id = $1");
    sqlx::query_as::<_, SoundRow>(&sql)
        .bind(sound_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Sound>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let rows = sqlx::query_as::<_, SoundRow>(
        "SELECT s.id, s.guild_id, s.name, s.emoji, s.storage_key, s.volume,
                (f.user_id IS NOT NULL) AS favourite
         FROM sounds s
         LEFT JOIN sound_favourites f ON f.sound_id = s.id AND f.user_id = $2
         WHERE s.guild_id = $1 ORDER BY lower(s.name)",
    )
    .bind(guild_id)
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.into_iter().map(Sound::from).collect()))
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

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(sound_id): Path<i64>,
) -> AppResult<StatusCode> {
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

pub async fn play(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(sound_id): Path<i64>,
) -> AppResult<StatusCode> {
    let sound = fetch_sound(&st, sound_id).await?;
    let voice = voice_state_of(&st, user.id)
        .filter(|v| v.guild_id == sound.guild_id)
        .ok_or_else(|| {
            AppError::BadRequest("Join a voice channel in this outhouse first.".into())
        })?;
    let access = channel_access(&st.db, voice.channel_id, user.id).await?;
    access.require(p::USE_SOUNDBOARD)?;
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
            guild_id: sound.guild_id,
            channel_id: voice.channel_id,
        },
        ServerEvent::SoundboardPlay {
            guild_id: sound.guild_id,
            channel_id: voice.channel_id,
            user_id: user.id,
            sound_id,
            url: attachment_url(&sound.storage_key, "sound"),
            volume: sound.volume,
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
    Path(sound_id): Path<i64>,
) -> AppResult<StatusCode> {
    let sound = fetch_sound(&st, sound_id).await?;
    member_ctx(&st.db, sound.guild_id, user.id).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sound_favourites WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&st.db)
        .await?;
    if count >= 100 {
        return Err(AppError::BadRequest(
            "That's a lot of favourites (100). Remove one first.".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO sound_favourites (user_id, sound_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(user.id)
    .bind(sound_id)
    .execute(&st.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_favourite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(sound_id): Path<i64>,
) -> AppResult<StatusCode> {
    sqlx::query("DELETE FROM sound_favourites WHERE user_id = $1 AND sound_id = $2")
        .bind(user.id)
        .bind(sound_id)
        .execute(&st.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- entrance sounds ----------

#[derive(sqlx::FromRow)]
struct EntranceRow {
    storage_key: String,
    duration_ms: i32,
}

async fn entrance_of(st: &AppState, user_id: i64) -> AppResult<Option<EntranceRow>> {
    Ok(sqlx::query_as::<_, EntranceRow>(
        "SELECT storage_key, duration_ms FROM entrance_sounds WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?)
}

fn entrance_json(row: EntranceRow) -> EntranceSound {
    EntranceSound {
        url: attachment_url(&row.storage_key, "entrance"),
        duration_ms: row.duration_ms,
    }
}

/// Your entrance sound (null if you haven't set one).
pub async fn get_entrance(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Option<EntranceSound>>> {
    Ok(Json(entrance_of(&st, user.id).await?.map(entrance_json)))
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
        "INSERT INTO entrance_sounds (user_id, storage_key, duration_ms) VALUES ($1, $2, $3)
         ON CONFLICT (user_id) DO UPDATE SET storage_key = $2, duration_ms = $3, created_at = now()",
    )
    .bind(user.id)
    .bind(&key)
    .bind(ms as i32)
    .execute(&mut *tx)
    .await?;
    if let Some(old) = &old {
        sqlx::query("DELETE FROM profile_files WHERE storage_key = $1")
            .bind(&old.storage_key)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    if let Some(old) = old {
        let _ = tokio::fs::remove_file(st.config.upload_dir.join(&old.storage_key)).await;
    }
    Ok(Json(EntranceSound {
        url: attachment_url(&key, "entrance"),
        duration_ms: ms as i32,
    }))
}

pub async fn delete_entrance(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<StatusCode> {
    if let Some(old) = entrance_of(&st, user.id).await? {
        // Removing the file row removes the entrance sound with it.
        sqlx::query("DELETE FROM profile_files WHERE storage_key = $1")
            .bind(&old.storage_key)
            .execute(&st.db)
            .await?;
        let _ = tokio::fs::remove_file(st.config.upload_dir.join(&old.storage_key)).await;
    }
    Ok(StatusCode::NO_CONTENT)
}
