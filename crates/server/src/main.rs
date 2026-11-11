//! Outhouse chat server.

mod api;
mod armory;
mod audio;
mod audit;
mod auth;
mod automod;
mod embeds;
mod error;
mod feeds;
mod files;
mod gateway;
mod ips;
mod livekit;
mod perms;
mod push;
mod security;
mod state;
mod wowhead;
mod wowhead_loot;
mod wowhead_page;

use std::net::SocketAddr;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

use crate::state::{AppState, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("outhouse_server=debug,tower_http=info")),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .context("DATABASE_URL is not set (copy .env.example to .env)")?;
    let bind: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()
        .context("BIND_ADDR is not a valid address")?;
    let web_dir = std::env::var("WEB_DIR").unwrap_or_else(|_| "web".into());

    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .context("could not connect to Postgres (is `docker compose up -d db` running?)")?;
    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .context("running database migrations")?;

    let config = Config::from_env();
    security::init(&config);
    tokio::fs::create_dir_all(&config.upload_dir)
        .await
        .with_context(|| format!("creating upload folder {}", config.upload_dir.display()))?;
    if config.giphy_api_key.is_none() {
        tracing::info!("GIPHY_API_KEY not set: the GIF picker is switched off");
    }
    let mut state = AppState::new(db, config);
    match push::load_or_create(&state.db).await {
        Ok(vapid) => state.vapid = Some(std::sync::Arc::new(vapid)),
        Err(e) => tracing::warn!(error = %e, "Web Push is off: couldn't load the VAPID key"),
    }
    feeds::spawn_poller(state.clone());
    api::events::spawn_reminders(state.clone());
    api::polls::spawn_closer(state.clone());
    api::lfg::spawn_expirer(state.clone());
    api::scheduled::spawn_sender(state.clone());
    audit::spawn_pruner(state.clone());
    api::threads::spawn_archiver(state.clone());
    armory::spawn_refresher(state.clone());
    api::safety::spawn_digest(state.clone());
    api::detector::spawn_worker(state.clone());
    ips::spawn_pruner(state.clone());
    // `/` is the welcome page (it sends signed-in people and the apps on to
    // `/app`); `/app` and `/login` are the web client. Everything else is a
    // plain file from the web folder.
    let web_path = std::path::Path::new(&web_dir);
    let client = web_path.join("index.html");
    let app = api::router(state)
        .route_service("/", ServeFile::new(web_path.join("landing.html")))
        .route_service("/app", ServeFile::new(&client))
        .route_service("/login", ServeFile::new(&client))
        .fallback_service(ServeDir::new(&web_dir))
        .layer(axum::middleware::map_response(no_stale_files))
        .layer(axum::middleware::from_fn(security::reject_nul))
        .layer(axum::middleware::from_fn(security::headers))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("Outhouse is open for business on http://{bind}");
    // The connecting address is kept for rate limits (see security::client_ip).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

/// Makes browsers check for a newer app.js / style.css on every load instead
/// of reusing an old copy (unchanged files still come back as a quick 304).
async fn no_stale_files(mut res: axum::response::Response) -> axum::response::Response {
    res.headers_mut()
        .entry(axum::http::header::CACHE_CONTROL)
        .or_insert(axum::http::HeaderValue::from_static("no-cache"));
    res
}
