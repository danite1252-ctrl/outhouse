//! REST API routes.

mod account;
mod channels;
mod feeds;
mod gifs;
pub mod guilds;
pub mod members;
pub mod messages;
mod roles;
mod sounds;
pub mod voice;
mod webhooks;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use outhouse_common::permissions::{all_permissions, PermissionInfo};

use crate::state::AppState;
use crate::{files, gateway};

async fn permission_list() -> Json<Vec<PermissionInfo>> {
    Json(all_permissions())
}

pub fn router(state: AppState) -> Router {
    let upload_limit = messages::MAX_FILES * messages::MAX_FILE_BYTES + 1024 * 1024;
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/permissions", get(permission_list))
        // Accounts
        .route("/api/auth/register", post(account::register))
        .route("/api/auth/login", post(account::login))
        .route("/api/auth/logout", post(account::logout))
        .route("/api/users/@me", get(account::me).patch(account::update_me))
        // Outhouses
        .route("/api/guilds", get(guilds::list).post(guilds::create))
        .route("/api/guilds/{id}", delete(guilds::delete))
        .route("/api/guilds/{id}/leave", post(guilds::leave))
        .route("/api/guilds/{id}/permissions", get(guilds::my_permissions))
        .route("/api/guilds/{id}/invites", post(guilds::create_invite))
        .route("/api/invites/{code}", post(guilds::join_invite))
        // Channels
        .route(
            "/api/guilds/{id}/channels",
            get(channels::list).post(channels::create),
        )
        .route("/api/channels/{id}", delete(channels::delete))
        .route("/api/channels/{id}/ack", post(channels::ack))
        .route(
            "/api/channels/{id}/permissions",
            get(channels::list_overwrites),
        )
        .route(
            "/api/channels/{id}/permissions/{target}",
            put(channels::set_overwrite).delete(channels::delete_overwrite),
        )
        // Roles
        .route(
            "/api/guilds/{id}/roles",
            get(roles::list).post(roles::create),
        )
        .route(
            "/api/guilds/{id}/roles/{role}",
            axum::routing::patch(roles::update).delete(roles::delete),
        )
        // Members and moderation
        .route("/api/guilds/{id}/members", get(members::list))
        .route("/api/guilds/{id}/nickname", put(members::set_nickname))
        .route("/api/guilds/{id}/members/{user}", delete(members::kick))
        .route(
            "/api/guilds/{id}/members/{user}/timeout",
            post(members::timeout),
        )
        .route(
            "/api/guilds/{id}/members/{user}/roles/{role}",
            put(members::add_role).delete(members::remove_role),
        )
        .route("/api/guilds/{id}/bans", get(members::list_bans))
        .route(
            "/api/guilds/{id}/bans/{user}",
            put(members::ban).delete(members::unban),
        )
        // Messages
        .route(
            "/api/channels/{id}/messages",
            get(messages::list).post(messages::send),
        )
        .route(
            "/api/channels/{id}/messages/upload",
            post(messages::upload).layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route(
            "/api/channels/{id}/messages/{message}",
            axum::routing::patch(messages::edit).delete(messages::delete),
        )
        .route(
            "/api/channels/{id}/messages/{message}/reactions/{emoji}",
            put(messages::add_reaction).delete(messages::remove_reaction),
        )
        .route("/api/channels/{id}/typing", post(messages::typing))
        // Webhooks and news feeds
        .route("/api/guilds/{id}/webhooks", get(webhooks::list))
        .route("/api/channels/{id}/webhooks", post(webhooks::create))
        .route("/api/webhooks/{id}", delete(webhooks::delete))
        .route("/api/webhooks/{id}/{token}", post(webhooks::execute))
        .route("/api/guilds/{id}/feeds", get(feeds::list))
        .route("/api/channels/{id}/feeds", post(feeds::create))
        .route("/api/feeds/{id}", delete(feeds::delete))
        .route("/api/feeds/{id}/check", post(feeds::check_now))
        // Voice and soundboard
        .route("/api/channels/{id}/voice/join", post(voice::join))
        .route("/api/voice/leave", post(voice::leave))
        .route("/api/voice/state", post(voice::update_state))
        .route("/api/guilds/{id}/voice", get(voice::list))
        .route(
            "/api/guilds/{id}/sounds",
            get(sounds::list).post(sounds::create),
        )
        .route("/api/sounds/{id}", delete(sounds::delete))
        .route("/api/sounds/{id}/play", post(sounds::play))
        // GIFs, files, gateway
        .route("/api/gifs", get(gifs::search))
        .route("/files/{key}/{name}", get(files::serve))
        .route("/api/gateway", get(gateway::gateway))
        .with_state(state)
}
