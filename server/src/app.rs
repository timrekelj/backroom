use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderName, Method};
use axum::routing::get;
use connectrpc::Router as ConnectRouter;
use scylla::client::caching_session::CachingSession;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::embeds::EmbedSettings;
use crate::events::EventBus;
use crate::media::attachments::AttachmentSettings;
use crate::media::{self, MAX_INPUT_BYTES};
use crate::ratelimit::RateLimiter;
use crate::services::{AdminSvc, AuthSvc, DmSvc, EmojiSvc, GatewaySvc, GuildSvc, MediaSvc, MessageSvc, RoleSvc};
use crate::snowflake::Snowflake;
use crate::storage::Storage;

/// Shared application state handed to every service.
#[derive(Clone)]
pub struct AppState {
    pub session: Arc<CachingSession>,
    pub snowflake: Arc<Snowflake>,
    pub keyspace: String,
    pub events: Arc<EventBus>,
    pub limiter: Arc<RateLimiter>,
    /// How often the gateway sends a heartbeat on each open stream.
    pub heartbeat_interval: Duration,
    /// Object storage for media; `None` makes the media RPCs and `/media`
    /// answer UNAVAILABLE while everything else works.
    pub storage: Option<Storage>,
    /// Limits and timings of message attachments.
    pub attachments: AttachmentSettings,
    /// Link embed pipeline (SSRF-safe unfurling of URLs in messages).
    pub embeds: EmbedSettings,
}

impl AppState {
    pub fn new(
        session: Arc<CachingSession>,
        snowflake: Arc<Snowflake>,
        keyspace: String,
        heartbeat_interval: Duration,
    ) -> Self {
        Self {
            session,
            snowflake,
            keyspace,
            events: Arc::new(EventBus::new()),
            limiter: Arc::new(RateLimiter::new()),
            heartbeat_interval,
            storage: None,
            attachments: AttachmentSettings::default(),
            embeds: EmbedSettings::default(),
        }
    }

    /// Overrides the attachment limits (`THEHIDEOUT_MAX_UPLOAD_BYTES`, tests).
    pub fn with_attachment_settings(mut self, attachments: AttachmentSettings) -> Self {
        self.attachments = attachments;
        self
    }

    /// Overrides the embed settings (`THEHIDEOUT_EMBEDS`, tests).
    pub fn with_embed_settings(mut self, embeds: EmbedSettings) -> Self {
        self.embeds = embeds;
        self
    }

    /// Enables media storage.
    pub fn with_storage(mut self, storage: Storage) -> Self {
        self.storage = Some(storage);
        self
    }
}

/// Largest Connect request body of the media RPCs: the image as protobuf
/// bytes, or base64 inside Connect JSON (4/3 of the size), plus the other
/// fields.
const MEDIA_REQUEST_LIMIT: usize = MAX_INPUT_BYTES / 3 * 4 + 64 * 1024;

/// Builds the HTTP app: all Connect services, `GET /health`, `GET /media/*`,
/// `POST /attachments`, and permissive CORS.
pub fn build_app(state: AppState) -> Router {
    let connect = ConnectRouter::new()
        .add_service(Arc::new(AuthSvc::new(state.clone())))
        .add_service(Arc::new(GuildSvc::new(state.clone())))
        .add_service(Arc::new(MessageSvc::new(state.clone())))
        .add_service(Arc::new(DmSvc::new(state.clone())))
        .add_service(Arc::new(RoleSvc::new(state.clone())))
        .add_service(Arc::new(AdminSvc::new(state.clone())))
        .add_service(Arc::new(EmojiSvc::new(state.clone())))
        .add_service(Arc::new(GatewaySvc::new(state.clone())));

    // Media RPCs get their own router so only they accept large bodies; the
    // library default (4 MB) keeps applying to everything else. A body over
    // the limit is refused with a Connect error.
    let media_limits = connectrpc::Limits::default()
        .with_max_request_body_size(MEDIA_REQUEST_LIMIT)
        .with_max_message_size(MEDIA_REQUEST_LIMIT);
    let media_connect = ConnectRouter::new()
        .add_service(Arc::new(MediaSvc::new(state.clone())))
        .into_axum_service()
        .with_limits(media_limits);

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route_service("/thehideout.v1.MediaService/{method}", media_connect)
        .merge(media::media_route().with_state(state.clone()))
        .merge(media::attachments::attachments_route().with_state(state))
        .fallback_service(connect.into_axum_service())
        .layer(cors())
}

fn cors() -> CorsLayer {
    let headers = [
        "content-type",
        "authorization",
        "connect-protocol-version",
        "connect-timeout-ms",
        "connect-content-encoding",
        "connect-accept-encoding",
        "grpc-timeout",
        "x-grpc-web",
        "x-user-agent",
    ]
    .map(HeaderName::from_static);
    let exposed = [
        "grpc-status",
        "grpc-message",
        "grpc-status-details-bin",
        "connect-content-encoding",
        "connect-accept-encoding",
    ]
    .map(HeaderName::from_static);
    CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(headers)
        .expose_headers(exposed)
        .max_age(Duration::from_secs(600))
}
