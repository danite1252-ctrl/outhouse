//! Audit log: who changed what in an outhouse (moderation and admin
//! actions). Writing an entry never fails the request that caused it.

use std::time::Duration;

use outhouse_common::models::DM_GUILD;
use outhouse_common::permissions::all_permissions;

use crate::state::AppState;

/// How long entries are kept unless `AUDIT_RETENTION_DAYS` says otherwise.
pub const DEFAULT_RETENTION_DAYS: i64 = 90;

/// One audit log entry, built up and then saved:
/// `Entry::new(guild, actor, "role_update").target("role", id, name).details(..).save(&st).await`.
pub struct Entry {
    guild_id: i64,
    actor_id: Option<i64>,
    action: &'static str,
    target_kind: &'static str,
    target_id: Option<i64>,
    target_name: String,
    details: String,
}

impl Entry {
    pub fn new(guild_id: i64, actor_id: i64, action: &'static str) -> Self {
        Self {
            guild_id,
            actor_id: Some(actor_id),
            action,
            target_kind: "",
            target_id: None,
            target_name: String::new(),
            details: String::new(),
        }
    }

    pub fn target(mut self, kind: &'static str, id: i64, name: impl Into<String>) -> Self {
        self.target_kind = kind;
        self.target_id = Some(id);
        self.target_name = name.into().chars().take(100).collect();
        self
    }

    pub fn details(mut self, details: impl Into<String>) -> Self {
        self.details = details.into().chars().take(1000).collect();
        self
    }

    /// Saves the entry (errors are logged, never returned).
    pub async fn save(self, st: &AppState) {
        if self.guild_id == DM_GUILD {
            return;
        }
        let result = sqlx::query(
            "INSERT INTO audit_log
                 (id, guild_id, actor_id, action, target_kind, target_id, target_name, details)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(st.ids.next_id())
        .bind(self.guild_id)
        .bind(self.actor_id)
        .bind(self.action)
        .bind(self.target_kind)
        .bind(self.target_id)
        .bind(&self.target_name)
        .bind(&self.details)
        .execute(&st.db)
        .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, action = self.action, "couldn't write the audit log");
        }
    }
}

/// Someone's name for the log: their nickname in the outhouse if they're
/// still in it, otherwise their display name.
pub async fn user_label(st: &AppState, guild_id: i64, user_id: i64) -> String {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE(m.nickname, u.display_name) FROM users u
         LEFT JOIN guild_members m ON m.user_id = u.id AND m.guild_id = $2
         WHERE u.id = $1",
    )
    .bind(user_id)
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten();
    name.unwrap_or_else(|| "someone".into())
}

/// Every action the log records, with a label for the settings screen.
pub const ACTIONS: &[(&str, &str)] = &[
    ("channel_create", "Created a channel"),
    ("channel_update", "Changed a channel"),
    ("channel_delete", "Deleted a channel"),
    ("channel_permissions", "Changed channel permissions"),
    ("channel_permissions_reset", "Removed channel permissions"),
    ("role_create", "Created a role"),
    ("role_update", "Changed a role"),
    ("role_delete", "Deleted a role"),
    ("member_role_add", "Gave a role"),
    ("member_role_remove", "Took a role away"),
    ("member_kick", "Kicked someone"),
    ("member_ban", "Banned someone"),
    ("member_unban", "Unbanned someone"),
    ("member_timeout", "Timed someone out"),
    ("member_timeout_remove", "Ended a timeout"),
    ("member_nickname", "Changed nickname"),
    ("message_delete", "Deleted someone's message"),
    ("message_pin", "Pinned a message"),
    ("message_unpin", "Unpinned a message"),
    ("emoji_create", "Added an emoji"),
    ("emoji_rename", "Renamed an emoji"),
    ("emoji_delete", "Deleted an emoji"),
    ("sound_create", "Added a sound"),
    ("sound_delete", "Deleted a sound"),
    ("voice_moderate", "Server muted / deafened"),
    ("voice_move", "Moved someone in voice"),
    ("voice_disconnect", "Disconnected someone from voice"),
    ("bot_create", "Made a bot"),
    ("bot_delete", "Deleted a bot"),
    ("bot_token_reset", "Reset a bot's token"),
    ("webhook_create", "Made a webhook"),
    ("webhook_delete", "Deleted a webhook"),
    ("feed_create", "Followed a news feed"),
    ("feed_delete", "Removed a news feed"),
    ("raid_delete", "Cancelled a raid"),
    ("raid_repeat_update", "Changed a repeating raid's days"),
    ("raid_settings_update", "Changed raid settings"),
    ("loot_reserve_remove", "Removed someone's soft reserve"),
    ("loot_reserves_lock", "Locked or unlocked soft reserves"),
    ("loot_item_add", "Added an item to a loot list"),
    ("loot_item_remove", "Removed an item from a loot list"),
    ("loot_award", "Gave out loot"),
    ("loot_award_remove", "Removed a loot log entry"),
    ("badge_create", "Made a badge"),
    ("badge_update", "Changed a badge"),
    ("badge_delete", "Deleted a badge"),
    ("badge_award", "Gave a badge"),
    ("badge_revoke", "Took a badge away"),
    ("invite_delete", "Revoked someone's invite"),
];

/// "Kick members, Ban members" for a set of permission bits.
pub fn perm_names(bits: u64) -> String {
    let names: Vec<&str> = all_permissions()
        .into_iter()
        .filter(|p| bits & p.bit != 0)
        .map(|p| p.name)
        .collect();
    if names.is_empty() {
        "nothing".into()
    } else {
        names.join(", ")
    }
}

/// "Allowed: +Kick members, −Ban members" (None if nothing changed).
pub fn perm_diff(label: &str, old: u64, new: u64) -> Option<String> {
    if old == new {
        return None;
    }
    let mut parts = Vec::new();
    for p in all_permissions() {
        if new & p.bit != 0 && old & p.bit == 0 {
            parts.push(format!("+{}", p.name));
        } else if old & p.bit != 0 && new & p.bit == 0 {
            parts.push(format!("−{}", p.name));
        }
    }
    Some(format!("{label}: {}", parts.join(", ")))
}

/// "Name: Raider → Core raider" (None if it didn't change).
pub fn change<T: std::fmt::Display + PartialEq>(label: &str, old: T, new: T) -> Option<String> {
    (old != new).then(|| format!("{label}: {old} → {new}"))
}

/// Joins the parts that are there with "; ".
pub fn join(parts: impl IntoIterator<Item = Option<String>>) -> String {
    parts.into_iter().flatten().collect::<Vec<_>>().join("; ")
}

/// Deletes old entries once at start-up and then every six hours.
pub fn spawn_pruner(st: AppState) {
    let days = std::env::var("AUDIT_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(DEFAULT_RETENTION_DAYS);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(6 * 3600));
        loop {
            tick.tick().await;
            let result = sqlx::query(
                "DELETE FROM audit_log WHERE created_at < now() - make_interval(days => $1)",
            )
            .bind(days as i32)
            .execute(&st.db)
            .await;
            match result {
                Ok(r) if r.rows_affected() > 0 => {
                    tracing::info!(removed = r.rows_affected(), "pruned old audit log entries")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "couldn't prune the audit log"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use outhouse_common::permissions as p;

    #[test]
    fn describes_permission_changes() {
        assert_eq!(perm_names(0), "nothing");
        assert_eq!(perm_names(p::KICK_MEMBERS), "Kick members");
        let d = perm_diff("Allowed", p::KICK_MEMBERS, p::BAN_MEMBERS).unwrap();
        assert!(
            d.contains("+Ban members") && d.contains("−Kick members"),
            "{d}"
        );
        assert!(perm_diff("Allowed", 5, 5).is_none());
        assert_eq!(
            join([change("Name", "a", "b"), None, change("Colour", 1, 1)]),
            "Name: a → b"
        );
    }
}
