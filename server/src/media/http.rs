//! `GET /media/{key}`: public, immutable, content-addressed image serving.

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::parse_key;
use crate::AppState;

/// Requests per peer IP per [`MEDIA_WINDOW`]. Generous: clients load many
/// avatars at once, and responses are immutable so browsers cache them.
const MEDIA_IP_MAX: usize = 1200;
const MEDIA_WINDOW: Duration = Duration::from_secs(60);
const CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

pub fn media_route() -> Router<AppState> {
    Router::new().route("/media/{*key}", get(serve))
}

async fn serve(State(state): State<AppState>, Path(key): Path<String>, request: Request) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(|| "unknown".to_owned(), |c| c.ip().to_string());
    if state.limiter.check(&format!("media:{peer}"), MEDIA_IP_MAX, MEDIA_WINDOW).is_err() {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    // Anything that is not exactly a key we would have produced is a 404;
    // this also rules out traversal before the key reaches the store.
    let Some(hash) = parse_key(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let etag = format!("\"{hash}\"");
    let etag_matches = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim().trim_start_matches("W/") == etag));

    let Some(storage) = &state.storage else {
        return (StatusCode::SERVICE_UNAVAILABLE, "media storage is not configured").into_response();
    };
    let object = match storage.get(&key).await {
        Ok(Some(object)) => object,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "media read failed");
            return (StatusCode::SERVICE_UNAVAILABLE, "media storage unavailable").into_response();
        }
    };
    let content_type = object.info.content_type.unwrap_or_else(|| "image/webp".to_owned());
    let mut response = if etag_matches {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut r = Response::new(Body::from_stream(object.stream));
        if let Ok(v) = HeaderValue::from_str(&content_type) {
            r.headers_mut().insert(header::CONTENT_TYPE, v);
        }
        r.headers_mut().insert(header::CONTENT_LENGTH, object.info.size.into());
        r
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE_CONTROL));
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).expect("hex etag"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}
