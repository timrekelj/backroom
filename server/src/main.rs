use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use thehideout_server::config::Config;
use thehideout_server::embeds::EmbedSettings;
use thehideout_server::media::{self, attachments::AttachmentSettings};
use thehideout_server::snowflake::Snowflake;
use thehideout_server::storage::Storage;
use thehideout_server::{AppState, build_app, migrations};
use scylla::client::caching_session::CachingSession;
use scylla::client::session_builder::SessionBuilder;
use tracing_subscriber::EnvFilter;

/// Number of prepared statements kept by the session cache.
const PREPARED_STATEMENT_CACHE_SIZE: usize = 256;

#[tokio::main]
async fn main() -> Result<()> {
    // Load `.env` (current directory or a parent) before anything reads the
    // environment, including RUST_LOG. Variables already set in the real
    // environment win over the file. Reported once logging is up.
    let dotenv = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    match dotenv {
        Ok(path) => tracing::info!(path = %path.display(), "loaded environment file"),
        Err(e) if e.not_found() => {}
        Err(e) => tracing::warn!(error = %e, "could not load .env file; continuing without it"),
    }

    let config = Config::from_env()?;
    tracing::info!(?config, "starting thehideout-server");

    let session = SessionBuilder::new()
        .known_nodes(&config.scylla_nodes)
        .build()
        .await
        .context("connecting to ScyllaDB")?;
    migrations::run(&session, &config.keyspace).await?;

    let storage = Storage::s3(&config.s3)?;
    let storage_ok = match storage.check().await {
        Ok(()) => true,
        Err(e) => {
            // Degrade instead of failing: everything but media keeps working,
            // and media starts working once the store is reachable.
            tracing::warn!(error = %format!("{e:#}"), s3 = ?config.s3, "object storage is not reachable; media uploads will fail until it is");
            false
        }
    };
    if storage_ok {
        tracing::info!(bucket = %config.s3.bucket, "object storage ready");
    }

    let state = AppState::new(
        Arc::new(CachingSession::from(session, PREPARED_STATEMENT_CACHE_SIZE)),
        Arc::new(Snowflake::new(config.worker_id)),
        config.keyspace.clone(),
        config.heartbeat_interval,
    )
    .with_storage(storage)
    .with_attachment_settings(AttachmentSettings {
        max_upload_bytes: config.max_upload_bytes,
        ..Default::default()
    })
    .with_embed_settings({
        let mut embeds = EmbedSettings::default();
        embeds.enabled = config.embeds_enabled;
        embeds.contact = config.embeds_contact.clone();
        embeds
    });
    media::pending::spawn_sweeper(state.clone());
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("binding {}", config.listen_addr))?;
    tracing::info!(addr = %config.listen_addr, "listening");
    // ConnectInfo<SocketAddr> is stamped into every request's extensions, which
    // the auth service reads for per-IP rate limiting.
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
