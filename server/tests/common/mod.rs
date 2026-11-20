#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use thehideout_server::config::S3Config;
use thehideout_server::embeds::EmbedSettings;
use thehideout_server::media::attachments::AttachmentSettings;
use thehideout_server::snowflake::Snowflake;
use thehideout_server::storage::Storage;
use thehideout_server::{AppState, build_app, migrations};
use connectrpc::client::{ClientConfig, HttpClient};
use rand::Rng;
use scylla::client::caching_session::CachingSession;
use scylla::client::execution_profile::ExecutionProfile;
use scylla::client::session_builder::SessionBuilder;
use tokio::sync::Semaphore;

/// Test apps that may create their keyspace at the same time. Scylla DDL is
/// slow (every CREATE KEYSPACE / TABLE is a schema change the node has to
/// agree on), and the default test threads start dozens of apps at once; past
/// a handful the DDL queues up and migrations time out. Only the schema setup
/// is serialised: tests themselves still run in parallel.
const CONCURRENT_STARTUPS: usize = 3;
static STARTUPS: Semaphore = Semaphore::const_new(CONCURRENT_STARTUPS);
/// Attempts at creating a keyspace and running the migrations.
const MIGRATION_ATTEMPTS: usize = 4;

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
        Self::start_inner(heartbeat_interval, None, AttachmentSettings::default(), EmbedSettings::default()).await
    }

    /// Like `start`, with media storage on the real S3-compatible service from
    /// `docker-compose.yml` (127.0.0.1:8333, or the `THEHIDEOUT_S3_*` env vars).
    pub async fn start_with_storage() -> Self {
        Self::start_with_attachments(AttachmentSettings::default()).await
    }

    /// Like `start_with_storage`, with custom attachment limits and timings.
    pub async fn start_with_attachments(settings: AttachmentSettings) -> Self {
        let config = S3Config::from_env().expect("S3 config");
        let storage = Storage::s3(&config).expect("configure S3 storage");
        storage.check().await.unwrap_or_else(|e| {
            panic!(
                "object storage at {} is not reachable ({e:#}); run `docker compose up -d` and wait for the bucket to be created",
                config.endpoint
            )
        });
        Self::start_inner(Duration::from_secs(30), Some(storage), settings, EmbedSettings::default()).await
    }

    /// Like `start_with_storage`, with custom embed settings (loopback allowed
    /// for the in-test sites, short timeouts, …).
    pub async fn start_with_embeds(embeds: EmbedSettings) -> Self {
        let config = S3Config::from_env().expect("S3 config");
        let storage = Storage::s3(&config).expect("configure S3 storage");
        storage.check().await.unwrap_or_else(|e| {
            panic!("object storage at {} is not reachable ({e:#}); run `docker compose up -d`", config.endpoint)
        });
        Self::start_inner(Duration::from_secs(30), Some(storage), AttachmentSettings::default(), embeds).await
    }

    async fn start_inner(
        heartbeat_interval: Duration,
        storage: Option<Storage>,
        settings: AttachmentSettings,
        embeds: EmbedSettings,
    ) -> Self {
        // DDL can take far longer than the driver's 30 s default request
        // timeout when many keyspaces are being created.
        let profile = ExecutionProfile::builder().request_timeout(Some(Duration::from_secs(180))).build();
        let session = SessionBuilder::new()
            .known_node("127.0.0.1:9042")
            .default_execution_profile_handle(profile.into_handle())
            .schema_agreement_timeout(Duration::from_secs(120))
            .build()
            .await
            .expect("connect to ScyllaDB at 127.0.0.1:9042 (is `docker compose up -d` running?)");
        let keyspace = Self::create_keyspace(&session).await;

        let session = Arc::new(CachingSession::from(session, 64));
        let mut state = AppState::new(session, Arc::new(Snowflake::new(0)), keyspace, heartbeat_interval);
        state.storage = storage;
        state.attachments = settings;
        state.embeds = embeds;
        let app = build_app(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });

        Self {
            base_url: format!("http://{addr}"),
            state,
        }
    }

    /// Creates a fresh `test_<random>` keyspace and runs the migrations in it.
    /// Startups are limited to [`CONCURRENT_STARTUPS`] at a time and a failed
    /// attempt (a timeout under load leaves the keyspace half-built) is
    /// dropped and retried under a new name with a growing pause.
    async fn create_keyspace(session: &scylla::client::session::Session) -> String {
        let _permit = STARTUPS.acquire().await.expect("startup semaphore is never closed");
        let mut last_error = None;
        for attempt in 0..MIGRATION_ATTEMPTS {
            let keyspace = format!("test_{:016x}", rand::rng().random::<u64>());
            match migrations::run(session, &keyspace).await {
                Ok(()) => return keyspace,
                Err(e) => {
                    eprintln!("test keyspace {keyspace}: migrations failed (attempt {}): {e:#}", attempt + 1);
                    let _ = session.query_unpaged(format!("DROP KEYSPACE IF EXISTS {keyspace}"), ()).await;
                    last_error = Some(e);
                    tokio::time::sleep(Duration::from_millis(500 << attempt)).await;
                }
            }
        }
        panic!("run migrations: {:#}", last_error.expect("at least one attempt"));
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

    /// Deletes the media objects the test stored (accounted ones and
    /// still-registered pending uploads), then drops the test keyspace.
    pub async fn cleanup(self) {
        if let Some(storage) = &self.state.storage {
            for query in ["SELECT key FROM media_objects", "SELECT key FROM pending_sweep", "SELECT thumbnail FROM pending_sweep"] {
                if let Ok(result) = self.state.session.execute_unpaged(query, ()).await
                    && let Ok(rows) = result.into_rows_result()
                    && let Ok(rows) = rows.rows::<(Option<String>,)>()
                {
                    for (key,) in rows.flatten() {
                        if let Some(key) = key {
                            let _ = storage.delete(&key).await;
                        }
                    }
                }
            }
        }
        let _ = self
            .state
            .session
            .get_session()
            .query_unpaged(format!("DROP KEYSPACE IF EXISTS {}", self.state.keyspace), ())
            .await;
    }
}

pub mod fixtures;
