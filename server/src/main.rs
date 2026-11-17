use std::sync::Arc;

use anyhow::{Context, Result};
use backroom_server::config::Config;
use backroom_server::snowflake::Snowflake;
use backroom_server::{AppState, build_app, migrations};
use scylla::client::caching_session::CachingSession;
use scylla::client::session_builder::SessionBuilder;
use tracing_subscriber::EnvFilter;

/// Number of prepared statements kept by the session cache.
const PREPARED_STATEMENT_CACHE_SIZE: usize = 256;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env()?;
    tracing::info!(?config, "starting backroom-server");

    let session = SessionBuilder::new()
        .known_nodes(&config.scylla_nodes)
        .build()
        .await
        .context("connecting to ScyllaDB")?;
    migrations::run(&session, &config.keyspace).await?;

    let state = AppState::new(
        Arc::new(CachingSession::from(session, PREPARED_STATEMENT_CACHE_SIZE)),
        Arc::new(Snowflake::new(config.worker_id)),
        config.keyspace.clone(),
        config.heartbeat_interval,
    );
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("binding {}", config.listen_addr))?;
    tracing::info!(addr = %config.listen_addr, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
