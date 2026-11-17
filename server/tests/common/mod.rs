#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use backroom_server::snowflake::Snowflake;
use backroom_server::{AppState, build_app, migrations};
use connectrpc::client::{ClientConfig, HttpClient};
use rand::Rng;
use scylla::client::caching_session::CachingSession;
use scylla::client::session_builder::SessionBuilder;

/// A running app backed by a throwaway keyspace.
pub struct TestApp {
    pub base_url: String,
    pub state: AppState,
}

impl TestApp {
    /// Connects to Scylla at 127.0.0.1:9042, creates a fresh `test_<random>`
    /// keyspace, runs migrations and serves the app on 127.0.0.1:0.
    pub async fn start() -> Self {
        Self::start_with_heartbeat(Duration::from_secs(30)).await
    }

    /// Like `start`, with a custom gateway heartbeat interval.
    pub async fn start_with_heartbeat(heartbeat_interval: Duration) -> Self {
        let session = SessionBuilder::new()
            .known_node("127.0.0.1:9042")
            .build()
            .await
            .expect("connect to ScyllaDB at 127.0.0.1:9042 (is `docker compose up -d` running?)");
        let keyspace = format!("test_{:016x}", rand::rng().random::<u64>());
        migrations::run(&session, &keyspace)
            .await
            .expect("run migrations");

        let session = Arc::new(CachingSession::from(session, 64));
        let state = AppState::new(session, Arc::new(Snowflake::new(0)), keyspace, heartbeat_interval);
        let app = build_app(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            base_url: format!("http://{addr}"),
            state,
        }
    }

    /// Client config for generated Connect clients, optionally with a bearer token.
    pub fn client_config(&self, token: Option<&str>) -> ClientConfig {
        let mut config = ClientConfig::new(self.base_url.parse().unwrap());
        if let Some(token) = token {
            config = config.with_default_header("authorization", format!("Bearer {token}"));
        }
        config
    }

    /// A generated client, e.g. `app.client(AuthServiceClient::new, None)`.
    pub fn client<C>(&self, new: impl FnOnce(HttpClient, ClientConfig) -> C, token: Option<&str>) -> C {
        new(HttpClient::plaintext(), self.client_config(token))
    }

    /// Drops the test keyspace.
    pub async fn cleanup(self) {
        let _ = self
            .state
            .session
            .get_session()
            .query_unpaged(format!("DROP KEYSPACE IF EXISTS {}", self.state.keyspace), ())
            .await;
    }
}
