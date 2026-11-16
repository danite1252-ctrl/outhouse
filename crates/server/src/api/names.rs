//! Usernames: people can't change their own. Staff with `force_rename` (or
//! the site owner) can force a change when one breaks the rules:
//!
//! - the username is swapped at once for a neutral placeholder
//!   (`renamed-7f3k2`; the hyphen means nobody can ever sign up with one),
//!   so the bad name disappears everywhere (gateway `USER_UPDATE`);
//! - the old one is reserved for good (`reserved_usernames`): nobody, them
//!   included, can register it or pick it again (case and accents ignored);
//! - they're blocked (403 `must_choose_username`) until they pick a new
//!   username, which they can do exactly once for this change;
//! - Outhouse Safety DMs them (never naming the staff member) and the
//!   safety log records who did it, with their tier and permission.
//!
//! The site owner also keeps a list of blocked words for usernames
//! (substrings, compared after `blocked_form`), refused at sign-up and in
//! the forced choice. Existing accounts aren't touched automatically;
//! People has a filter to review them.
//!
//! Name-history rows from a forced change are `staff_only`: public
//! profiles never show the old name or the placeholder.

use axum::extract::{Query, State};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::User;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use unicode_normalization::UnicodeNormalization;

use super::staff::{Actor, StaffActor};
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

/// The username rules (sign-up and the forced choice).
pub const RULES: &str = "Usernames are 2–32 characters: letters, numbers, dots and underscores.";

/// Preset reasons for a forced change: (key, label).
pub const REASONS: &[(&str, &str)] = &[
    ("inappropriate", "Inappropriate"),
    ("offensive", "Offensive"),
    ("impersonation", "Impersonation"),
    ("other", "Other"),
];

/// Allowed characters and length.
pub fn valid_username(name: &str) -> bool {
    (2..=32).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Lower-case without accents: how reserved usernames are compared.
pub fn name_key(name: &str) -> String {
    name.trim()
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

/// How blocked words are compared: `name_key`, then light leetspeak
/// (0→o, 1→i, 3→e, 4→a, 5→s, @→a, $→s), without dots, underscores,
/// hyphens or spaces. Keep in step with `BLOCKED_SQL`.
pub fn blocked_form(text: &str) -> String {
    name_key(text)
        .chars()
        .filter_map(|c| match c {
            '0' => Some('o'),
            '1' => Some('i'),
            '3' => Some('e'),
            '4' => Some('a'),
            '5' => Some('s'),
            '@' => Some('a'),
            '$' => Some('s'),
            '.' | '_' | '-' | ' ' => None,
            c => Some(c),
        })
        .collect()
}

/// `blocked_form` of `u.username` in SQL (usernames are ASCII).
pub const BLOCKED_SQL: &str = "translate(lower(u.username), '01345@$._- ', 'oieasas')";

/// The blocked word this name contains, if any.
pub async fn blocked_word_in(db: &PgPool, name: &str) -> AppResult<Option<String>> {
    let form = blocked_form(name);
    Ok(sqlx::query_scalar(
        "SELECT word FROM username_blocked_words WHERE strpos($1, word) > 0 ORDER BY word LIMIT 1",
    )
    .bind(&form)
    .fetch_optional(db)
    .await?)
}

pub async fn is_reserved(db: &PgPool, name: &str) -> AppResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM reserved_usernames WHERE name_key = $1)")
            .bind(name_key(name))
            .fetch_one(db)
            .await?,
    )
}

/// Reserved and blocked-word checks for a new username (sign-up and the
/// forced choice). Rules and "taken" are checked by the caller.
pub async fn check_allowed(db: &PgPool, name: &str) -> AppResult<()> {
    if is_reserved(db, name).await? {
        return Err(AppError::BadRequest(
            "That username isn't available. Try another.".into(),
        ));
    }
    if blocked_word_in(db, name).await?.is_some() {
        return Err(AppError::BadRequest(
            "That username isn't allowed here. Try another.".into(),
        ));
    }
    Ok(())
}

/// The account behind a reserved username that's still choosing a new one
/// (so they can sign in with the name they know).
pub async fn pending_owner_of(db: &PgPool, name: &str) -> AppResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT r.user_id FROM reserved_usernames r JOIN users u ON u.id = r.user_id
         WHERE r.name_key = $1 AND u.must_choose_username
         ORDER BY r.created_at DESC LIMIT 1",
    )
    .bind(name_key(name))
    .fetch_optional(db)
    .await?)
}

/// Why their username was changed (the newest forced change), for the dialog.
pub async fn forced_reason(db: &PgPool, user_id: i64) -> AppResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT reason FROM reserved_usernames WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?)
}

/// Tells everyone who might be showing this person (their outhouses, DMs,
/// friends, and themselves) about their new name.
pub async fn announce(st: &AppState, user_id: i64) -> AppResult<()> {
    let user = auth::fetch_user(&st.db, user_id).await?;
    let guilds: Vec<i64> =
        sqlx::query_scalar("SELECT guild_id FROM guild_members WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&st.db)
            .await?;
    for guild_id in guilds {
        st.dispatch(
            Audience::Guild(guild_id),
            ServerEvent::UserUpdate(user.clone()),
        );
        st.dispatch(
            Audience::Guild(guild_id),
            ServerEvent::GuildRefresh { guild_id },
        );
    }
    let people: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT p2.user_id FROM dm_participants p1
           JOIN dm_participants p2 ON p2.channel_id = p1.channel_id
          WHERE p1.user_id = $1
         UNION
         SELECT CASE WHEN user_a = $1 THEN user_b ELSE user_a END FROM friendships
          WHERE user_a = $1 OR user_b = $1
         UNION SELECT $1",
    )
    .bind(user_id)
    .fetch_all(&st.db)
    .await?;
    for id in people {
        st.dispatch(Audience::User(id), ServerEvent::UserUpdate(user.clone()));
    }
    Ok(())
}

// ---- The forced change (staff) ------------------------------------------------------------

const PLACEHOLDER_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

fn placeholder() -> String {
    format!("renamed-{}", auth::random_code(5, PLACEHOLDER_ALPHABET))
}

pub struct ForceRename<'a> {
    pub reason: &'a str,
    pub text: &'a str,
    pub reset_display_name: bool,
}

/// Forces a username change (permission, target and cap checks are done by
/// the caller, `safety::user_action`). Returns the line for the safety log.
pub async fn force(st: &AppState, a: &Actor, target: i64, f: ForceRename<'_>) -> AppResult<String> {
    let label = REASONS
        .iter()
        .find(|(k, _)| *k == f.reason)
        .map(|(_, l)| *l)
        .ok_or_else(|| AppError::BadRequest("Pick a reason.".into()))?;
    let text: String = f.text.trim().chars().take(300).collect();
    if f.reason == "other" && text.is_empty() {
        return Err(AppError::BadRequest("Say why (for “Other”).".into()));
    }
    let (old, display, bot, pending): (String, String, bool, bool) = sqlx::query_as(
        "SELECT username, display_name, is_bot, must_choose_username FROM users WHERE id = $1",
    )
    .bind(target)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    if bot {
        return Err(AppError::BadRequest(
            "Bots' names are changed by their owner.".into(),
        ));
    }
    if pending {
        return Err(AppError::BadRequest(
            "Their username was already changed; they're choosing a new one.".into(),
        ));
    }
    let reason_stored = if text.is_empty() {
        label.to_string()
    } else {
        format!("{label}: {text}")
    };
    // A placeholder nobody has (the hyphen keeps them out of sign-ups).
    let mut new_name = String::new();
    for _ in 0..20 {
        let candidate = placeholder();
        let used: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE username = $1)")
                .bind(&candidate)
                .fetch_one(&st.db)
                .await?;
        if !used {
            new_name = candidate;
            break;
        }
    }
    if new_name.is_empty() {
        return Err(AppError::Conflict(
            "Couldn't pick a placeholder. Try again.".into(),
        ));
    }
    let mut tx = st.db.begin().await?;
    let changed = sqlx::query(
        "UPDATE users SET username = $2, must_choose_username = true,
                display_name = CASE WHEN $3 THEN $2 ELSE display_name END
         WHERE id = $1 AND username = $4 AND NOT must_choose_username",
    )
    .bind(target)
    .bind(&new_name)
    .bind(f.reset_display_name)
    .bind(&old)
    .execute(&mut *tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            AppError::Conflict("Couldn't pick a placeholder. Try again.".into())
        }
        e => e.into(),
    })?
    .rows_affected();
    if changed != 1 {
        return Err(AppError::Conflict(
            "Their username just changed. Look again.".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO reserved_usernames (name_key, username, reason, user_id, reserved_by)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (name_key) DO UPDATE SET reason = EXCLUDED.reason, user_id = EXCLUDED.user_id,
                reserved_by = EXCLUDED.reserved_by, created_at = now()",
    )
    .bind(name_key(&old))
    .bind(&old)
    .bind(&reason_stored)
    .bind(target)
    .bind(a.id())
    .execute(&mut *tx)
    .await?;
    // The history rows the trigger just wrote, and any other that shows the
    // old username (say a display name that was the same): staff and the
    // owner only.
    sqlx::query(
        "UPDATE name_history SET staff_only = true
         WHERE user_id = $1 AND (
               (kind IN ('username', 'display_name') AND new_value = $2)
               OR lower(old_value) = lower($3) OR lower(new_value) = lower($3))",
    )
    .bind(target)
    .bind(&new_name)
    .bind(&old)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    announce(st, target).await?;
    st.dispatch(Audience::User(target), ServerEvent::StaffChanged);
    if let Err(e) = super::safety::safety_note(
        st,
        target,
        format!(
            "✏️ Your username was changed by Outhouse staff (reason: {label}). Please choose a new one.\n\nYour old username is no longer available. {}",
            if f.reset_display_name {
                "Your display name was reset too (you can change it in User settings); your outhouse nicknames haven't changed."
            } else {
                "Your display name and outhouse nicknames haven't changed."
            }
        ),
    )
    .await
    {
        tracing::warn!(error = %e, "couldn't DM about a forced username change");
    }
    let mut line =
        format!("Forced a username change: @{old} → @{new_name}. Reason: {reason_stored}");
    if f.reset_display_name {
        line.push_str(&format!(". Display name “{display}” reset"));
    }
    Ok(line)
}

// ---- The person picks a new one (once) ------------------------------------------------------

#[derive(Deserialize)]
pub struct CheckQuery {
    #[serde(default)]
    username: String,
}

async fn must_choose(db: &PgPool, user_id: i64) -> AppResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT must_choose_username FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(db)
            .await?,
    )
}

pub const NO_CHANGES: &str = "Usernames can't be changed. You can change your display name, or your nickname in each outhouse.";

/// Everything a new pick has to pass: rules, reserved, blocked words, taken.
async fn check_pick(db: &PgPool, user_id: i64, name: &str) -> AppResult<()> {
    if !valid_username(name) {
        return Err(AppError::BadRequest(RULES.into()));
    }
    check_allowed(db, name).await?;
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE username = $1 AND id <> $2)")
            .bind(name)
            .bind(user_id)
            .fetch_one(db)
            .await?;
    if taken {
        return Err(AppError::Conflict("That username is taken.".into()));
    }
    Ok(())
}

/// `GET /api/users/@me/username/check?username=`: live availability while
/// choosing after a forced change.
pub async fn check(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<CheckQuery>,
) -> AppResult<Json<Value>> {
    if !must_choose(&st.db, user.id).await? {
        return Err(AppError::Refused(NO_CHANGES.into()));
    }
    let name = q.username.trim().to_lowercase();
    Ok(Json(match check_pick(&st.db, user.id, &name).await {
        Ok(()) => json!({ "ok": true, "username": name }),
        Err(e) => json!({ "ok": false, "username": name, "error": e.to_string() }),
    }))
}

#[derive(Deserialize)]
pub struct ChooseBody {
    #[serde(default)]
    username: String,
}

/// `POST /api/users/@me/username` `{username}`: only while a forced change
/// is waiting (once; the flag is cleared).
pub async fn choose(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<ChooseBody>,
) -> AppResult<Json<User>> {
    if user.bot || !must_choose(&st.db, user.id).await? {
        return Err(AppError::Refused(NO_CHANGES.into()));
    }
    let name = body.username.trim().to_lowercase();
    check_pick(&st.db, user.id, &name).await?;
    let mut tx = st.db.begin().await?;
    let placeholder: Option<String> = sqlx::query_scalar(
        "SELECT username FROM users WHERE id = $1 AND must_choose_username FOR UPDATE",
    )
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(placeholder) = placeholder else {
        return Err(AppError::Refused(NO_CHANGES.into()));
    };
    let r = sqlx::query(
        "UPDATE users SET username = $2, must_choose_username = false,
                display_name = CASE WHEN display_name = username THEN $2 ELSE display_name END
         WHERE id = $1",
    )
    .bind(user.id)
    .bind(&name)
    .execute(&mut *tx)
    .await;
    match r {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(AppError::Conflict("That username is taken.".into()))
        }
        other => {
            other?;
        }
    }
    sqlx::query(
        "UPDATE name_history SET staff_only = true
         WHERE user_id = $1 AND kind IN ('username', 'display_name') AND old_value = $2",
    )
    .bind(user.id)
    .bind(&placeholder)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    super::safety::log_action(
        &st,
        Some(user.id),
        "chose_username",
        None,
        Some(user.id),
        format!("Chose a new username after a forced change: @{placeholder} → @{name}"),
    )
    .await;
    announce(&st, user.id).await?;
    Ok(Json(auth::fetch_user(&st.db, user.id).await?))
}

// ---- The site owner's blocked words ------------------------------------------------------------

async fn words_json(db: &PgPool) -> AppResult<Value> {
    let rows: Vec<(String, chrono::DateTime<chrono::Utc>, Option<String>)> = sqlx::query_as(
        "SELECT w.word, w.added_at, u.username FROM username_blocked_words w
         LEFT JOIN users u ON u.id = w.added_by ORDER BY w.word",
    )
    .fetch_all(db)
    .await?;
    let matching: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM users u WHERE NOT u.is_bot AND EXISTS (
             SELECT 1 FROM username_blocked_words w WHERE strpos({BLOCKED_SQL}, w.word) > 0)"
    ))
    .fetch_one(db)
    .await?;
    Ok(json!({
        "words": rows.iter().map(|(w, at, by)| json!({ "word": w, "added_at": at, "added_by": by })).collect::<Vec<_>>(),
        "matching_users": matching,
    }))
}

/// `GET /api/safety/username-words` (site owner)
pub async fn list_words(
    State(st): State<AppState>,
    StaffActor(a): StaffActor,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    Ok(Json(words_json(&st.db).await?))
}

#[derive(Deserialize)]
pub struct WordBody {
    #[serde(default)]
    word: String,
}

pub const MAX_WORDS: i64 = 500;

/// `POST /api/safety/username-words` `{word}` (site owner)
pub async fn add_word(
    State(st): State<AppState>,
    StaffActor(a): StaffActor,
    Json(body): Json<WordBody>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let word = blocked_form(&body.word);
    if word.chars().count() < 2 || word.chars().count() > 32 {
        return Err(AppError::BadRequest(
            "Blocked words are 2–32 characters.".into(),
        ));
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM username_blocked_words")
        .fetch_one(&st.db)
        .await?;
    if n >= MAX_WORDS {
        return Err(AppError::BadRequest(format!(
            "That's the most there can be ({MAX_WORDS})."
        )));
    }
    let added = sqlx::query(
        "INSERT INTO username_blocked_words (word, added_by) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(&word)
    .bind(a.id())
    .execute(&st.db)
    .await?
    .rows_affected();
    if added == 1 {
        super::safety::log_action(
            &st,
            Some(a.id()),
            "username_word",
            None,
            None,
            format!("Blocked “{word}” in usernames"),
        )
        .await;
    }
    Ok(Json(words_json(&st.db).await?))
}

/// `DELETE /api/safety/username-words` `{word}` (site owner)
pub async fn remove_word(
    State(st): State<AppState>,
    StaffActor(a): StaffActor,
    Json(body): Json<WordBody>,
) -> AppResult<Json<Value>> {
    a.require_owner()?;
    let word = blocked_form(&body.word);
    let n = sqlx::query("DELETE FROM username_blocked_words WHERE word = $1")
        .bind(&word)
        .execute(&st.db)
        .await?
        .rows_affected();
    if n == 1 {
        super::safety::log_action(
            &st,
            Some(a.id()),
            "username_word",
            None,
            None,
            format!("Unblocked “{word}” in usernames"),
        )
        .await;
    }
    Ok(Json(words_json(&st.db).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_case_and_accents() {
        assert_eq!(name_key("Zoë.Bad"), "zoe.bad");
        assert_eq!(name_key("  ÅNGSTRÖM "), "angstrom");
    }

    #[test]
    fn blocked_form_is_leet_light() {
        assert_eq!(blocked_form("B4d_W0rd"), "badword");
        assert_eq!(blocked_form("$1lly.3"), "sillye");
        assert_eq!(blocked_form("@ss"), "ass");
        assert_eq!(blocked_form("Crème 5"), "cremes");
    }

    #[test]
    fn placeholders_cant_be_registered() {
        let p = placeholder();
        assert!(p.starts_with("renamed-") && p.len() == 13);
        assert!(!valid_username(&p));
        assert!(valid_username("good_name.2"));
        assert!(!valid_username("a"));
    }
}
