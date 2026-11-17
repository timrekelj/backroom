use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderName, Method};
use axum::routing::get;
use connectrpc::Router as ConnectRouter;
use scylla::client::caching_session::CachingSession;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::services::{AuthSvc, GatewaySvc, GuildSvc, MessageSvc};
use crate::snowflake::Snowflake;

/// Shared application state handed to every service.
#[derive(Clone)]
pub struct AppState {
    pub session: Arc<CachingSession>,
    pub snowflake: Arc<Snowflake>,
    pub keyspace: String,
}

impl AppState {
    pub fn new(session: Arc<CachingSession>, snowflake: Arc<Snowflake>, keyspace: String) -> Self {
        Self {
            session,
            snowflake,
            keyspace,
        }
    }
}

/// Builds the HTTP app: all Connect services, `GET /health`, and permissive CORS.
pub fn build_app(state: AppState) -> Router {
    let connect = ConnectRouter::new()
        .add_service(Arc::new(AuthSvc::new(state.clone())))
        .add_service(Arc::new(GuildSvc::new(state.clone())))
        .add_service(Arc::new(MessageSvc::new(state.clone())))
        .add_service(Arc::new(GatewaySvc::new(state)));

    Router::new()
        .route("/health", get(|| async { "ok" }))
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
