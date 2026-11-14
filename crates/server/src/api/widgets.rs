//! Your sidebar widgets: the stack above the member list (clock, mini
//! calendar, mentions, soundboard). Which are on, their order, which are
//! collapsed and the calendar's week/month view. Private to you, the same
//! on every device (like rail folders).
//!
//! `GET/PUT /api/users/@me/widgets` with
//! `{"widgets": [{"id": "clock", "on": true, "collapsed": false},
//!               {"id": "calendar", "on": true, "collapsed": false, "mode": "week"}, …]}`.
//! Every known widget is in the answer exactly once: unknown ones are
//! refused, missing ones are added at the end with their defaults.

use std::collections::HashSet;

use axum::extract::State;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use serde::{Deserialize, Serialize};
use sqlx::types::Json as DbJson;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

/// Every widget, in the default order, with whether it starts switched on.
pub const WIDGETS: &[(&str, bool)] = &[
    ("clock", true),
    ("calendar", true),
    ("mentions", true),
    ("soundboard", false),
];
const CALENDAR_MODES: &[&str] = &["week", "month"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Widget {
    pub id: String,
    #[serde(default)]
    pub on: bool,
    #[serde(default)]
    pub collapsed: bool,
    /// The calendar's view: "week" or "month" (other widgets: none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Widgets {
    pub widgets: Vec<Widget>,
}

fn default_widget(id: &str, on: bool) -> Widget {
    Widget {
        id: id.to_string(),
        on,
        collapsed: false,
        mode: (id == "calendar").then(|| "week".to_string()),
    }
}

pub fn defaults() -> Vec<Widget> {
    WIDGETS
        .iter()
        .map(|(id, on)| default_widget(id, *on))
        .collect()
}

/// Checks a list (strict: for saving) and fills in anything missing.
fn clean(list: Vec<Widget>, strict: bool) -> AppResult<Vec<Widget>> {
    if list.len() > WIDGETS.len() * 2 {
        return Err(AppError::BadRequest("That's too many widgets.".into()));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for mut w in list {
        let Some((id, _)) = WIDGETS.iter().find(|(id, _)| *id == w.id) else {
            if strict {
                return Err(AppError::BadRequest(format!(
                    "Unknown widget “{}”.",
                    w.id.chars().take(32).collect::<String>()
                )));
            }
            continue;
        };
        if !seen.insert(*id) {
            if strict {
                return Err(AppError::BadRequest(
                    "Each widget can only be listed once.".into(),
                ));
            }
            continue;
        }
        if *id == "calendar" {
            let mode = w.mode.as_deref().map(str::trim).unwrap_or("week");
            if !CALENDAR_MODES.contains(&mode) {
                if strict {
                    return Err(AppError::BadRequest(
                        "The calendar shows a week or a month.".into(),
                    ));
                }
                w.mode = Some("week".into());
            } else {
                w.mode = Some(mode.to_string());
            }
        } else if w.mode.is_some() {
            if strict {
                return Err(AppError::BadRequest(
                    "Only the calendar has a week or month view.".into(),
                ));
            }
            w.mode = None;
        }
        out.push(w);
    }
    for (id, on) in WIDGETS {
        if !seen.contains(id) {
            out.push(default_widget(id, *on));
        }
    }
    Ok(out)
}

pub async fn load(st: &AppState, user_id: i64) -> AppResult<Widgets> {
    let stored: Option<DbJson<Vec<Widget>>> =
        sqlx::query_scalar("SELECT widgets FROM user_widgets WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&st.db)
            .await?;
    let widgets = match stored {
        Some(DbJson(list)) => clean(list, false)?,
        None => defaults(),
    };
    Ok(Widgets { widgets })
}

/// `GET /api/users/@me/widgets`
pub async fn get(State(st): State<AppState>, AuthUser(user): AuthUser) -> AppResult<Json<Widgets>> {
    Ok(Json(load(&st, user.id).await?))
}

/// `PUT /api/users/@me/widgets`: the whole list, in order.
pub async fn set(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<Widgets>,
) -> AppResult<Json<Widgets>> {
    let widgets = clean(req.widgets, true)?;
    sqlx::query(
        "INSERT INTO user_widgets (user_id, widgets, updated_at) VALUES ($1, $2, now())
         ON CONFLICT (user_id) DO UPDATE SET widgets = EXCLUDED.widgets, updated_at = now()",
    )
    .bind(user.id)
    .bind(DbJson(&widgets))
    .execute(&st.db)
    .await?;
    st.dispatch(Audience::User(user.id), ServerEvent::WidgetsChanged);
    Ok(Json(Widgets { widgets }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: &str, on: bool) -> Widget {
        Widget {
            id: id.into(),
            on,
            collapsed: false,
            mode: None,
        }
    }

    #[test]
    fn fills_in_missing_widgets_in_default_order() {
        let got = clean(vec![w("mentions", false)], true).unwrap();
        let ids: Vec<&str> = got.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["mentions", "clock", "calendar", "soundboard"]);
        assert!(!got[0].on);
        assert_eq!(got[2].mode.as_deref(), Some("week"));
        assert!(!got[3].on);
    }

    #[test]
    fn refuses_unknown_repeated_and_bad_modes() {
        assert!(clean(vec![w("weather", true)], true).is_err());
        assert!(clean(vec![w("clock", true), w("clock", false)], true).is_err());
        let mut cal = w("calendar", true);
        cal.mode = Some("year".into());
        assert!(clean(vec![cal.clone()], true).is_err());
        // Stored lists are tidied instead.
        assert_eq!(
            clean(vec![cal], false).unwrap()[0].mode.as_deref(),
            Some("week")
        );
        let mut clock = w("clock", true);
        clock.mode = Some("month".into());
        assert!(clean(vec![clock], true).is_err());
    }
}
