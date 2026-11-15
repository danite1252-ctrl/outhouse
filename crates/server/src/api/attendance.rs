//! Raid attendance marked by hand: once a raid has started, its leaders
//! (Edit attendance in the raid card's 📋 Attendance section) mark members
//! present, late, benched, excused or absent. A mark overrides what an MRT
//! import says about that member on that raid; clearing it goes back to
//! the import. Marks live apart from imports (undoing an import never takes
//! them out) and every change is in the audit log.
//!
//! How marks count (with imports) is in `mrt.rs` and the
//! `raid_attendance_outcomes` view (migration 0047): present, late and
//! benched are attended; excused is neither attended nor a no-show; absent
//! is a no-show if they signed up yes, late or tentative.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::id::as_string;
use outhouse_common::models::AttendanceRequest;
use serde::Serialize;

use super::events::{broadcast, can_manage, load};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub const MARKS: &[&str] = &["present", "late", "benched", "excused", "absent"];
const MAX_RECORDS: usize = 200;

#[derive(Serialize, sqlx::FromRow)]
pub struct AttendanceRecord {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    pub status: String,
    pub marked_by_name: Option<String>,
    pub marked_at: DateTime<Utc>,
}

pub(crate) async fn records(st: &AppState, event_id: i64) -> AppResult<Vec<AttendanceRecord>> {
    Ok(sqlx::query_as::<_, AttendanceRecord>(
        "SELECT a.user_id, COALESCE(gm.nickname, u.display_name) AS display_name, a.status,
                COALESCE(bm.nickname, b.display_name) AS marked_by_name, a.marked_at
         FROM event_attendance a
         JOIN events e ON e.id = a.event_id
         JOIN users u ON u.id = a.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = e.guild_id AND gm.user_id = a.user_id
         LEFT JOIN users b ON b.id = a.marked_by
         LEFT JOIN guild_members bm ON bm.guild_id = e.guild_id AND bm.user_id = a.marked_by
         WHERE a.event_id = $1
         ORDER BY lower(COALESCE(gm.nickname, u.display_name)), a.user_id",
    )
    .bind(event_id)
    .fetch_all(&st.db)
    .await?)
}

/// `GET /api/events/{id}/attendance`
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<Vec<AttendanceRecord>>> {
    load(&st, event_id, user.id).await?;
    Ok(Json(records(&st, event_id).await?))
}

/// `PUT /api/events/{id}/attendance`: raid leaders mark who turned up.
pub async fn set(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<AttendanceRequest>,
) -> AppResult<Json<Vec<AttendanceRecord>>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&st.db, &row, &access).await? {
        return Err(AppError::Forbidden);
    }
    if row.starts_at > Utc::now() {
        return Err(AppError::BadRequest(
            "You can take attendance once the raid has started.".into(),
        ));
    }
    if req.records.len() > MAX_RECORDS {
        return Err(AppError::BadRequest("Too many people at once.".into()));
    }
    for r in &req.records {
        if !r.status.is_empty() && !MARKS.contains(&r.status.as_str()) {
            return Err(AppError::BadRequest(
                "Attendance is present, late, benched, excused or absent.".into(),
            ));
        }
    }
    let ids: Vec<i64> = req.records.iter().map(|r| r.user_id).collect();
    let members: HashSet<i64> = sqlx::query_scalar::<_, i64>(
        "SELECT user_id FROM guild_members WHERE guild_id = $1 AND user_id = ANY($2)",
    )
    .bind(row.guild_id)
    .bind(&ids)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();
    // People who have left can still be cleared or keep their old mark.
    let before: HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(
        "SELECT user_id, status FROM event_attendance WHERE event_id = $1",
    )
    .bind(event_id)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();
    if ids
        .iter()
        .any(|id| !members.contains(id) && !before.contains_key(id))
    {
        return Err(AppError::BadRequest(
            "Everyone marked has to be in this outhouse.".into(),
        ));
    }

    let mut tx = st.db.begin().await?;
    if req.all_signed_up {
        sqlx::query(
            "INSERT INTO event_attendance (event_id, user_id, status, marked_by)
             SELECT s.event_id, s.user_id,
                    CASE WHEN s.status = 'bench' THEN 'benched' ELSE 'present' END, $2
             FROM event_signups s
             WHERE s.event_id = $1 AND s.status IN ('yes', 'late', 'bench')
             ON CONFLICT (event_id, user_id) DO UPDATE
                 SET status = EXCLUDED.status, marked_by = EXCLUDED.marked_by, marked_at = now()",
        )
        .bind(event_id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    }
    for r in &req.records {
        if r.status.is_empty() {
            sqlx::query("DELETE FROM event_attendance WHERE event_id = $1 AND user_id = $2")
                .bind(event_id)
                .bind(r.user_id)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO event_attendance (event_id, user_id, status, marked_by)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (event_id, user_id) DO UPDATE
                     SET status = EXCLUDED.status, marked_by = EXCLUDED.marked_by,
                         marked_at = now()",
            )
            .bind(event_id)
            .bind(r.user_id)
            .bind(&r.status)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        "UPDATE events SET attendance_taken =
             EXISTS(SELECT 1 FROM event_attendance WHERE event_id = $1)
         WHERE id = $1",
    )
    .bind(event_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let after = records(&st, event_id).await?;
    log_changes(&st, &row, user.id, &before, &after).await;
    broadcast(&st, event_id).await?;
    Ok(Json(after))
}

/// "Edited attendance for BT Sunday (15 Nov): Kit present, Sham excused,
/// Dani cleared (back to MRT)", when anything changed.
async fn log_changes(
    st: &AppState,
    row: &super::events::EventRow,
    actor: i64,
    before: &HashMap<i64, String>,
    after: &[AttendanceRecord],
) {
    let now: HashMap<i64, &AttendanceRecord> = after.iter().map(|r| (r.user_id, r)).collect();
    let mut changes: Vec<String> = Vec::new();
    for r in after {
        match before.get(&r.user_id) {
            Some(old) if *old == r.status => {}
            Some(old) => changes.push(format!("{} {} → {}", r.display_name, old, r.status)),
            None => changes.push(format!("{} {}", r.display_name, r.status)),
        }
    }
    let mut cleared: Vec<i64> = before
        .keys()
        .filter(|id| !now.contains_key(id))
        .copied()
        .collect();
    cleared.sort_unstable();
    for id in cleared {
        let name = crate::audit::user_label(st, row.guild_id, id).await;
        changes.push(format!("{name} cleared (back to MRT)"));
    }
    if changes.is_empty() {
        return;
    }
    let total = changes.len();
    if total > 25 {
        changes.truncate(25);
        changes.push(format!("and {} more", total - 25));
    }
    crate::audit::Entry::new(row.guild_id, actor, "attendance_edit")
        .target("raid", row.id, row.title.clone())
        .details(format!(
            "Edited attendance for {} ({}): {}",
            row.title,
            row.starts_at.format("%-d %b"),
            changes.join(", ")
        ))
        .save(st)
        .await;
}
