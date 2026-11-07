//! Outhouse chat server.

mod api;
mod auth;
mod embeds;
mod error;
mod feeds;
mod files;
mod gateway;
mod perms;
mod state;

use std::net::SocketAddr;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tower_http::{services::ServeDir, trace::TraceLayer};
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
    tokio::fs::create_dir_all(&config.upload_dir)
        .await
        .with_context(|| format!("creating upload folder {}", config.upload_dir.display()))?;
    if config.giphy_api_key.is_none() {
        tracing::info!("GIPHY_API_KEY not set: the GIF picker is switched off");
    }
    let state = AppState::new(db, config);
    feeds::spawn_poller(state.clone());
    api::events::spawn_reminders(state.clone());
    api::polls::spawn_closer(state.clone());
    let app = api::router(state)
        .fallback_service(ServeDir::new(&web_dir))
        .layer(axum::middleware::map_response(no_stale_files))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("Outhouse is open for business on http://{bind}");
    axum::serve(listener, app)
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
