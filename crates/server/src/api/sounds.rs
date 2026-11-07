//! The soundboard: a library of short clips per outhouse.
//!
//! Playing a clip doesn't mix it into anyone's microphone: the server tells
//! everyone in that voice channel to play it, so each person's own
//! soundboard volume and "mute soundboard" setting apply.

use std::time::{Duration, Instant};

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::Sound;
use outhouse_common::permissions as p;

use super::messages::attachment_url;
use super::voice::voice_state_of;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, member_ctx};
use crate::state::{AppState, Audience};

const MAX_SOUND_BYTES: usize = 1024 * 1024;
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
        }
    }
}

const SOUND_SELECT: &str = "SELECT id, guild_id, name, emoji, storage_key, volume FROM sounds";

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
    let sql = format!("{SOUND_SELECT} WHERE guild_id = $1 ORDER BY lower(name)");
    let rows = sqlx::query_as::<_, SoundRow>(&sql)
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.into_iter().map(Sound::from).collect()))
}

fn audio_type(filename: &str) -> Option<&'static str> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        _ => return None,
    })
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

    let key = auth::random_code(24, KEY_ALPHABET);
    tokio::fs::create_dir_all(&st.config.upload_dir)
        .await
        .map_err(|e| anyhow::anyhow!("creating upload folder: {e}"))?;
    tokio::fs::write(st.config.upload_dir.join(&key), &data)
        .await
        .map_err(|e| anyhow::anyhow!("saving sound: {e}"))?;

    let id = st.ids.next_id();
    sqlx::query(
        "INSERT INTO sounds (id, guild_id, name, emoji, storage_key, content_type, size, volume, creator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
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
    .execute(&st.db)
    .await?;

    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::SoundsChanged { guild_id },
    );
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
