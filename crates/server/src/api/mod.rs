//! REST API routes.

mod account;
mod attendance;
pub(crate) mod audit;
mod badges;
mod bnet;
mod bots;
mod channels;
pub mod components;
pub mod detector;
mod dms;
pub mod emojis;
pub mod events;
mod feeds;
mod forward;
pub mod friends;
mod gifs;
pub mod guilds;
pub mod invites;
mod ipblocks;
pub mod lfg;
mod loot;
pub mod members;
pub mod messages;
mod notifications;
pub mod polls;
mod profiles;
pub mod raid_settings;
pub mod requests;
mod roles;
pub mod safety;
pub mod scheduled;
mod search;
mod softres;
pub mod sounds;
mod stickers;
pub mod threads;
pub mod voice;
mod webhooks;
pub mod wow;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};
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
        .route("/api/health", get(audit::health))
        .route("/api/permissions", get(permission_list))
        // Accounts
        .route("/api/auth/register", post(account::register))
        .route("/api/auth/registration", get(account::registration_mode))
        .route("/api/auth/login", post(account::login))
        .route("/api/auth/logout", post(account::logout))
        .route("/api/users/@me", get(account::me).patch(account::update_me))
        .route("/api/users/@me/unread", get(search::unread))
        // Outhouses
        .route("/api/guilds", get(guilds::list).post(guilds::create))
        .route("/api/guilds/{id}", delete(guilds::delete))
        .route("/api/guilds/{id}/leave", post(guilds::leave))
        .route("/api/guilds/{id}/permissions", get(guilds::my_permissions))
        .route(
            "/api/guilds/{id}/invites",
            get(invites::list).post(invites::create),
        )
        .route(
            "/api/invites/{code}",
            get(invites::preview)
                .post(invites::join)
                .delete(invites::revoke),
        )
        .route("/invite/{code}", get(invites::page))
        .route("/.well-known/assetlinks.json", get(invites::asset_links))
        // Channels
        .route(
            "/api/guilds/{id}/channels",
            get(channels::list).post(channels::create),
        )
        .route(
            "/api/channels/{id}",
            patch(channels::update).delete(channels::delete),
        )
        .route(
            "/api/channels/{id}/age-gate",
            put(channels::accept_age_gate).delete(channels::reset_age_gate),
        )
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
            patch(roles::update).delete(roles::delete),
        )
        // Members and moderation
        .route("/api/guilds/{id}/members", get(members::list))
        .route("/api/guilds/{id}/nickname", put(members::set_nickname))
        .route(
            "/api/guilds/{id}/members/{user}/nickname",
            put(members::set_member_nickname),
        )
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
            patch(messages::edit).delete(messages::delete),
        )
        .route(
            "/api/channels/{id}/messages/{message}/reactions/{emoji}",
            put(messages::add_reaction).delete(messages::remove_reaction),
        )
        .route("/api/channels/{id}/typing", post(messages::typing))
        .route("/api/channels/{id}/pins", get(messages::list_pins))
        .route(
            "/api/channels/{id}/pins/{message}",
            put(messages::pin).delete(messages::unpin),
        )
        .route("/api/guilds/{id}/search", get(search::search))
        // Threads and polls
        .route(
            "/api/channels/{id}/threads",
            get(threads::list).post(threads::create),
        )
        .route("/api/threads/{id}/ack", post(threads::ack))
        .route(
            "/api/threads/{id}",
            get(threads::get)
                .patch(threads::rename)
                .delete(threads::delete),
        )
        .route("/api/channels/{id}/polls", post(polls::create))
        .route("/api/channels/{id}/polls/{message}/votes", put(polls::vote))
        .route(
            "/api/channels/{id}/polls/{message}/close",
            post(polls::close),
        )
        // Direct messages
        .route("/api/dms", get(dms::list).post(dms::create))
        .route("/api/dms/requests", get(requests::list))
        .route(
            "/api/dms/requests/{id}/accept",
            post(requests::accept_route),
        )
        .route(
            "/api/dms/requests/{id}/ignore",
            post(requests::ignore_route),
        )
        .route("/api/dms/{id}", patch(dms::update))
        .route("/api/dms/{id}/recipients", post(dms::add_recipient))
        .route("/api/dms/{id}/recipients/@me", delete(dms::leave))
        .route("/api/users/@me/contacts", get(dms::contacts))
        // Custom emoji
        .route(
            "/api/guilds/{id}/emojis",
            get(emojis::list)
                .post(emojis::create)
                .layer(DefaultBodyLimit::max(emojis::MAX_EMOJI_BYTES + 64 * 1024)),
        )
        .route(
            "/api/emojis/{id}",
            patch(emojis::rename).delete(emojis::delete),
        )
        .route("/api/users/@me/emojis", get(emojis::list_mine))
        .route("/emojis/{id}", get(files::serve_emoji))
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
            "/api/guilds/{id}/voice/{user}",
            patch(voice::moderate).delete(voice::disconnect),
        )
        .route(
            "/api/guilds/{id}/sounds",
            get(sounds::list)
                .post(sounds::create)
                .layer(DefaultBodyLimit::max(sounds::MAX_SOUND_BYTES + 64 * 1024)),
        )
        .route("/api/users/@me/sounds", get(sounds::list_mine))
        .route("/api/builtin-sounds", get(sounds::list_builtins))
        .route(
            "/api/guilds/{id}/hidden-sounds/{key}",
            put(sounds::hide_builtin).delete(sounds::show_builtin),
        )
        .route(
            "/api/sounds/{id}",
            patch(sounds::edit).delete(sounds::delete),
        )
        .route("/api/sounds/{id}/play", post(sounds::play))
        .route(
            "/api/sounds/{id}/favourite",
            put(sounds::add_favourite).delete(sounds::remove_favourite),
        )
        .route(
            "/api/users/@me/entrance-sound",
            get(sounds::get_entrance)
                .post(sounds::set_entrance)
                .put(sounds::set_entrance_builtin)
                .delete(sounds::delete_entrance)
                .layer(DefaultBodyLimit::max(
                    sounds::MAX_ENTRANCE_BYTES + 64 * 1024,
                )),
        )
        // Profiles
        .route("/api/profiles/{id}", get(profiles::get))
        .route("/api/users/{id}/profile/full", get(profiles::full))
        .route(
            "/api/users/@me/avatar",
            post(profiles::upload_avatar)
                .delete(profiles::delete_avatar)
                .layer(DefaultBodyLimit::max(
                    profiles::MAX_AVATAR_BYTES + 64 * 1024,
                )),
        )
        .route(
            "/api/users/@me/banner",
            post(profiles::upload_banner)
                .delete(profiles::delete_banner)
                .layer(DefaultBodyLimit::max(
                    profiles::MAX_BANNER_BYTES + 64 * 1024,
                )),
        )
        // Raid planner
        .route("/api/channels/{id}/events", post(events::create))
        .route("/api/guilds/{id}/events", get(events::list_upcoming))
        .route(
            "/api/events/{id}",
            get(events::get)
                .patch(events::update)
                .delete(events::delete),
        )
        .route(
            "/api/events/{id}/repeat-preview",
            post(events::repeat_preview),
        )
        .route("/api/events/{id}/signup", put(events::signup))
        .route("/api/events/{id}/export", get(events::export))
        .route(
            "/api/guilds/{id}/raid-settings",
            get(raid_settings::get).put(raid_settings::set),
        )
        .route("/api/events/{id}/signups/{user}", delete(events::withdraw))
        .route(
            "/api/events/{id}/attendance",
            get(attendance::get).put(attendance::set),
        )
        .route("/api/guilds/{id}/attendance", get(attendance::guild_table))
        // Raid composition, soft reserves and the loot log
        .route("/api/events/{id}/composition", get(events::composition))
        .route("/api/events/{id}/loot-table", get(loot::get_table))
        .route("/api/events/{id}/loot-items", post(loot::add_item))
        .route(
            "/api/events/{id}/loot-table/refresh",
            post(loot::refresh_table),
        )
        .route(
            "/api/events/{id}/loot-sources",
            post(loot::add_source).delete(loot::remove_source),
        )
        .route("/api/wow/item-icons/{id}", get(loot::item_icon))
        .route(
            "/api/events/{id}/loot-items/{item}",
            delete(loot::remove_item),
        )
        .route("/api/events/{id}/reserves", put(loot::reserve))
        .route("/api/events/{id}/reserves/lock", put(loot::set_lock))
        .route("/api/events/{id}/reserves/export", get(softres::export))
        .route("/api/events/{id}/softres/gargul", get(softres::gargul))
        .route("/api/events/{id}/reserves/{item}", delete(loot::unreserve))
        .route(
            "/api/events/{id}/loot",
            get(loot::raid_awards).post(loot::award),
        )
        .route("/api/loot/{id}", delete(loot::remove_award))
        .route("/api/guilds/{id}/loot", get(loot::guild_log))
        .route("/api/guilds/{id}/loot-instances", get(loot::instances))
        .route("/api/wow/items", get(loot::search_item))
        .route("/api/wow/items/{id}", get(loot::get_item))
        .route(
            "/api/guilds/{id}/attendance/{user}",
            get(attendance::member),
        )
        // WoW tools: group finder, items, characters, Battle.net
        .route("/api/wow/info", get(wow::info))
        .route("/api/channels/{id}/lfg", post(lfg::create))
        .route("/api/channels/{id}/lfg/{message}/join", put(lfg::join))
        .route(
            "/api/channels/{id}/lfg/{message}/members/{user}",
            delete(lfg::remove_member),
        )
        .route("/api/channels/{id}/lfg/{message}/close", post(lfg::close))
        .route("/api/channels/{id}/wow/item", post(wow::item_command))
        .route("/api/channels/{id}/wow/char", post(wow::char_command))
        .route(
            "/api/users/@me/characters",
            get(wow::list_mine).post(wow::create_character),
        )
        .route(
            "/api/characters/{id}",
            patch(wow::update_character).delete(wow::delete_character),
        )
        .route("/api/wow/armory/lookup", post(crate::armory::lookup))
        .route("/api/wow/avatars/{key}", get(crate::armory::avatar))
        .route("/api/characters/{id}/refresh", post(crate::armory::refresh))
        .route("/api/bnet/status", get(bnet::status))
        .route("/api/bnet/authorize", post(bnet::authorize))
        .route("/api/bnet/link", delete(bnet::unlink))
        .route("/api/bnet/callback", get(bnet::callback))
        // Bots and slash commands
        .route("/api/guilds/{id}/bots", get(bots::list).post(bots::create))
        .route("/api/bots/{id}", delete(bots::delete))
        .route("/api/bots/{id}/token", post(bots::reset_token))
        .route(
            "/api/guilds/{id}/commands",
            get(bots::list_commands).put(bots::set_commands),
        )
        .route("/api/channels/{id}/interactions", post(bots::interact))
        .route("/api/interactions/{id}/reply", post(bots::reply))
        // Bot message components (buttons, select menus), automod, thread defaults
        .route(
            "/api/channels/{id}/messages/{message}/interactions",
            post(components::use_component),
        )
        .route("/api/interactions/{id}/callback", post(bots::callback))
        .route(
            "/api/channels/{id}/automod",
            get(crate::automod::get_channel).put(crate::automod::set_channel),
        )
        .route(
            "/api/guilds/{id}/automod",
            get(crate::automod::get_guild).put(crate::automod::set_guild),
        )
        .route(
            "/api/channels/{id}/thread-defaults",
            get(threads::get_defaults).put(threads::set_defaults),
        )
        // Audit log, server status, badges
        .route("/api/guilds/{id}/audit-log", get(audit::list))
        .route("/api/guilds/{id}/server-status", get(audit::server_status))
        .route("/api/users/@me/site-owner", get(audit::site_owner))
        .route(
            "/api/guilds/{id}/badges",
            get(badges::list).post(badges::create),
        )
        .route(
            "/api/badges/{id}",
            patch(badges::update).delete(badges::delete),
        )
        .route(
            "/api/badges/{id}/members/{user}",
            put(badges::award).delete(badges::revoke),
        )
        // Notifications and Web Push
        .route("/api/users/@me/notifications", get(notifications::get))
        .route(
            "/api/users/@me/notifications/guilds/{id}",
            put(notifications::set_level),
        )
        .route(
            "/api/users/@me/notifications/channels/{id}",
            put(notifications::set_mute),
        )
        .route(
            "/api/push/subscriptions",
            post(notifications::subscribe).delete(notifications::unsubscribe),
        )
        .route("/api/push/test", post(notifications::test))
        // Friends, blocks and privacy
        .route(
            "/api/users/@me/relationships",
            get(friends::list).post(friends::add_by_username),
        )
        .route(
            "/api/users/@me/relationships/{id}",
            put(friends::add_by_id).delete(friends::remove),
        )
        .route(
            "/api/users/@me/blocks/{id}",
            put(friends::block).delete(friends::unblock),
        )
        .route(
            "/api/users/@me/privacy",
            get(friends::get_privacy).put(friends::set_privacy),
        )
        // Stickers
        .route(
            "/api/guilds/{id}/stickers",
            get(stickers::list)
                .post(stickers::create)
                .layer(DefaultBodyLimit::max(
                    stickers::MAX_STICKER_BYTES + 64 * 1024,
                )),
        )
        .route(
            "/api/stickers/{id}",
            patch(stickers::update).delete(stickers::delete),
        )
        .route("/api/users/@me/stickers", get(stickers::list_mine))
        .route("/api/channels/{id}/stickers", post(stickers::send))
        .route("/stickers/{id}", get(stickers::serve))
        // Forwarding and scheduled messages
        .route(
            "/api/channels/{id}/messages/{message}/forward",
            post(forward::forward),
        )
        .route("/api/users/@me/scheduled", get(scheduled::list))
        .route("/api/channels/{id}/scheduled", post(scheduled::create))
        .route(
            "/api/scheduled/{id}",
            patch(scheduled::update).delete(scheduled::delete),
        )
        .route("/api/scheduled/{id}/send", post(scheduled::send_now))
        // Safety: reports, the site owner's console, teen protections
        .route("/api/reports", post(safety::create))
        .route("/api/users/{id}/contact", get(safety::contact))
        .route("/api/safety/summary", get(safety::summary))
        .route("/api/safety/reports", get(safety::list))
        .route("/api/safety/reports/{id}", get(safety::get))
        .route(
            "/api/safety/reports/{id}/actions",
            post(safety::report_action),
        )
        .route("/api/safety/reports/{id}/export", get(safety::export))
        .route(
            "/api/safety/reports/{id}/files/{key}",
            get(safety::report_file),
        )
        .route("/api/safety/users", get(safety::find_users))
        .route("/api/safety/users/{id}", get(safety::get_user))
        .route("/api/safety/users/{id}/ips", get(safety::user_ips))
        .route("/api/safety/ips/purge", post(safety::purge_ips_now))
        .route(
            "/api/safety/users/{id}/actions",
            post(safety::user_action_route),
        )
        .route("/api/safety/log", get(safety::get_log))
        .route(
            "/api/safety/settings",
            get(safety::get_settings).put(safety::set_settings),
        )
        .route("/api/safety/guilds/{id}/family", put(safety::set_family))
        .route("/api/safety/digest", get(safety::digest))
        // Blocked networks (permanent suspensions; site owner only)
        .route(
            "/api/safety/users/{id}/ip-block-preview",
            get(ipblocks::preview),
        )
        .route("/api/safety/users/{id}/ip-blocks", post(ipblocks::create))
        .route("/api/safety/ip-blocks", get(ipblocks::list))
        .route("/api/safety/ip-blocks/reload", post(ipblocks::reload_now))
        .route("/api/safety/ip-blocks/{id}/remove", post(ipblocks::remove))
        .route("/api/safety/ip-blocks/{id}/extend", post(ipblocks::extend))
        // The image detector (site owner's settings; revealing held pictures)
        .route(
            "/api/safety/detector",
            get(detector::get_settings).put(detector::set_settings),
        )
        .route(
            "/api/safety/guilds/{id}/detector",
            get(detector::get_guild_settings).put(detector::set_guild_settings),
        )
        .route("/api/attachments/{id}/reveal", get(detector::reveal))
        // GIFs, files, gateway
        .route("/api/gifs", get(gifs::search))
        .route("/files/{key}/{name}", get(files::serve))
        .route("/api/gateway", get(gateway::gateway))
        .with_state(state)
}
