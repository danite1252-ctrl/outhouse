//! Raid attendance: after a raid starts, its leader marks who was there
//! (present / late / absent / benched). Shown per member on profile cards
//! and as a table for the whole outhouse.
//!
//! Attendance % counts present, late and benched as turning up (a benched
//! player was there and ready), out of every raid they were marked for.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::id::as_string;
use outhouse_common::models::AttendanceRequest;
use serde::{Deserialize, Serialize};

use super::events::{broadcast, can_manage, load};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{member_ctx, visible_text_channels};
use crate::state::AppState;

const MARKS: &[&str] = &["present", "late", "absent", "benched"];
const MAX_RECORDS: usize = 200;

#[derive(Serialize, sqlx::FromRow)]
pub struct AttendanceRecord {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    pub status: String,
}

async fn records(st: &AppState, event_id: i64) -> AppResult<Vec<AttendanceRecord>> {
    Ok(sqlx::query_as::<_, AttendanceRecord>(
        "SELECT a.user_id, COALESCE(gm.nickname, u.display_name) AS display_name, a.status
         FROM event_attendance a
         JOIN events e ON e.id = a.event_id
         JOIN users u ON u.id = a.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = e.guild_id AND gm.user_id = a.user_id
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
                "Attendance is present, late, absent or benched.".into(),
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
    let existing: HashSet<i64> =
        sqlx::query_scalar::<_, i64>("SELECT user_id FROM event_attendance WHERE event_id = $1")
            .bind(event_id)
            .fetch_all(&st.db)
            .await?
            .into_iter()
            .collect();
    if ids
        .iter()
        .any(|id| !members.contains(id) && !existing.contains(id))
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
    broadcast(&st, event_id).await?;
    Ok(Json(records(&st, event_id).await?))
}

#[derive(Deserialize)]
pub struct LimitQuery {
    limit: Option<i64>,
}

#[derive(Serialize)]
pub struct AttendanceRaid {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    pub title: String,
    pub starts_at: DateTime<Utc>,
}

#[derive(Serialize, Default)]
pub struct Tally {
    pub present: i32,
    pub late: i32,
    pub absent: i32,
    pub benched: i32,
    /// Raids they were marked for.
    pub total: i32,
    /// Turned up (present, late or benched) out of `total`, rounded.
    pub percent: i32,
}

impl Tally {
    fn add(&mut self, status: &str) {
        match status {
            "present" => self.present += 1,
            "late" => self.late += 1,
            "absent" => self.absent += 1,
            "benched" => self.benched += 1,
            _ => return,
        }
        self.total += 1;
        let came = self.present + self.late + self.benched;
        self.percent = ((came as f64 * 100.0) / self.total as f64).round() as i32;
    }
}

#[derive(Serialize)]
pub struct MemberAttendance {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    #[serde(flatten)]
    pub tally: Tally,
    /// Their mark in each of `raids` (same order; "" = not marked).
    pub history: Vec<String>,
}

#[derive(Serialize)]
pub struct GuildAttendance {
    /// Newest first.
    pub raids: Vec<AttendanceRaid>,
    pub members: Vec<MemberAttendance>,
}

#[derive(sqlx::FromRow)]
struct RaidRow {
    id: i64,
    channel_id: i64,
    title: String,
    starts_at: DateTime<Utc>,
}

/// The last `limit` raids with attendance in channels you can see.
async fn recent_raids(
    st: &AppState,
    guild_id: i64,
    user_id: i64,
    limit: i64,
) -> AppResult<Vec<RaidRow>> {
    let ctx = member_ctx(&st.db, guild_id, user_id).await?;
    let channels =
        visible_text_channels(&st.db, &ctx, outhouse_common::permissions::VIEW_CHANNEL).await?;
    Ok(sqlx::query_as::<_, RaidRow>(
        "SELECT id, channel_id, title, starts_at FROM events
         WHERE guild_id = $1 AND attendance_taken AND channel_id = ANY($2)
         ORDER BY starts_at DESC, id DESC
         LIMIT $3",
    )
    .bind(guild_id)
    .bind(&channels)
    .bind(limit)
    .fetch_all(&st.db)
    .await?)
}

/// `GET /api/guilds/{id}/attendance?limit=N`: everyone's attendance over
/// the last N raids (default 10, up to 50).
pub async fn guild_table(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(q): Query<LimitQuery>,
) -> AppResult<Json<GuildAttendance>> {
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    let raids = recent_raids(&st, guild_id, user.id, limit).await?;
    let ids: Vec<i64> = raids.iter().map(|r| r.id).collect();
    #[derive(sqlx::FromRow)]
    struct Row {
        event_id: i64,
        user_id: i64,
        display_name: String,
        status: String,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT a.event_id, a.user_id, COALESCE(gm.nickname, u.display_name) AS display_name,
                a.status
         FROM event_attendance a
         JOIN users u ON u.id = a.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = $2 AND gm.user_id = a.user_id
         WHERE a.event_id = ANY($1)",
    )
    .bind(&ids)
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    let column: HashMap<i64, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut members: HashMap<i64, MemberAttendance> = HashMap::new();
    for r in rows {
        let m = members
            .entry(r.user_id)
            .or_insert_with(|| MemberAttendance {
                user_id: r.user_id,
                display_name: r.display_name.clone(),
                tally: Tally::default(),
                history: vec![String::new(); ids.len()],
            });
        m.tally.add(&r.status);
        if let Some(&i) = column.get(&r.event_id) {
            m.history[i] = r.status;
        }
    }
    let mut members: Vec<MemberAttendance> = members.into_values().collect();
    members.sort_by(|a, b| {
        b.tally
            .percent
            .cmp(&a.tally.percent)
            .then(b.tally.total.cmp(&a.tally.total))
            .then(
                a.display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase()),
            )
    });
    Ok(Json(GuildAttendance {
        raids: raids
            .into_iter()
            .map(|r| AttendanceRaid {
                id: r.id,
                channel_id: r.channel_id,
                title: r.title,
                starts_at: r.starts_at,
            })
            .collect(),
        members,
    }))
}

#[derive(Serialize)]
pub struct RecentMark {
    #[serde(with = "as_string")]
    pub event_id: i64,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub status: String,
}

#[derive(Serialize)]
pub struct MemberSummary {
    #[serde(flatten)]
    pub tally: Tally,
    /// Newest first, up to 10.
    pub recent: Vec<RecentMark>,
}

/// `GET /api/guilds/{id}/attendance/{user}`: one member's attendance over
/// the outhouse's last 20 raids with attendance (for their profile card).
pub async fn member(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target)): Path<(i64, i64)>,
) -> AppResult<Json<MemberSummary>> {
    let raids = recent_raids(&st, guild_id, user.id, 20).await?;
    let ids: Vec<i64> = raids.iter().map(|r| r.id).collect();
    let marks: HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(
        "SELECT event_id, status FROM event_attendance WHERE user_id = $1 AND event_id = ANY($2)",
    )
    .bind(target)
    .bind(&ids)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();
    let mut tally = Tally::default();
    let mut recent = Vec::new();
    for r in raids {
        if let Some(status) = marks.get(&r.id) {
            tally.add(status);
            if recent.len() < 10 {
                recent.push(RecentMark {
                    event_id: r.id,
                    title: r.title,
                    starts_at: r.starts_at,
                    status: status.clone(),
                });
            }
        }
    }
    Ok(Json(MemberSummary { tally, recent }))
}

#[cfg(test)]
mod tests {
    use super::Tally;

    #[test]
    fn tally_counts_bench_as_turning_up() {
        let mut t = Tally::default();
        for s in ["present", "late", "benched", "absent", "nonsense"] {
            t.add(s);
        }
        assert_eq!(t.total, 4);
        assert_eq!(t.percent, 75);
    }
}
