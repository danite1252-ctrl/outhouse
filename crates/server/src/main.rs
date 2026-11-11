//! Outhouse chat server.

mod api;
mod armory;
mod audio;
mod audit;
mod auth;
mod automod;
mod cli;
mod embeds;
mod error;
mod feeds;
mod files;
mod gateway;
mod handover;
mod ipblock;
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
    // `outhouse-server <command> ...`: one of the owner's server-side
    // commands (see cli.rs), not the web server.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(command) = args.first().filter(|c| cli::COMMANDS.contains(&c.as_str())) {
        let code = run_command(command, &args[1..]).await?;
        std::process::exit(code);
    }
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
    api::police::spawn_cleaner(state.clone());
    if let Err(e) = ipblock::reload(&state.db).await {
        tracing::warn!(error = %e, "couldn't load the blocked networks");
    }
    ipblock::spawn_refresher(state.clone());
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
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
                // Police download links carry a secret token: never log it.
                tracing::debug_span!(
                    "request",
                    method = %req.method(),
                    uri = %redact_uri(req.uri()),
                )
            }),
        );

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

/// Runs a server-side command against the database and upload folder the
/// server uses. Logs go to stderr so stdout carries only the output.
async fn run_command(command: &str, args: &[String]) -> anyhow::Result<i32> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::new("outhouse_server=warn"))
        .init();
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is not set")?;
    let db = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .context("could not connect to Postgres")?;
    let config = Config::from_env();
    security::init(&config);
    let mut state = AppState::new(db, config);
    // A different ID worker from the running server's (0), so IDs made here
    // (safety log entries) can never clash with its.
    state.ids = std::sync::Arc::new(outhouse_common::id::SnowflakeGen::new(31));
    cli::run(state, command, args).await
}

/// A request path for the log, without secrets (`/evidence/<token>`).
fn redact_uri(uri: &axum::http::Uri) -> String {
    match uri.path().strip_prefix("/evidence/") {
        Some(token) => format!("/evidence/{}…", token.chars().take(8).collect::<String>()),
        None => uri.to_string(),
    }
}

/// Makes browsers check for a newer app.js / style.css on every load instead
/// of reusing an old copy (unchanged files still come back as a quick 304).
async fn no_stale_files(mut res: axum::response::Response) -> axum::response::Response {
    res.headers_mut()
        .entry(axum::http::header::CACHE_CONTROL)
        .or_insert(axum::http::HeaderValue::from_static("no-cache"));
    res
}
