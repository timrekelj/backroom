//! `POST /attachments`: authenticated, streaming upload of one message
//! attachment (any file type) to object storage. See `proto/attachment.proto`
//! for the wire contract and `pending.rs` for the upload lifecycle.
//!
//! The body is never held in memory whole: it is hashed and forwarded to the
//! store part by part (see `Storage::begin_upload`). The streaming work runs in
//! its own task, so a client that disconnects mid-upload makes the body
//! stream fail and the task aborts the multipart upload instead of the handler
//! being dropped with the upload half open.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use bytes::Bytes;
use chrono::Utc;
use connectrpc::ErrorCode;
use futures::{StreamExt, TryStreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::pending::{self, Pending};
use super::sniff::{self, SNIFF_LEN};
use super::{ATTACHMENT_IMAGE_MAX_BYTES, acquire_decode_slot, attachment_key, attachment_thumbnail_key, inspect_attachment_image, random_attachment_id};
use crate::AppState;
use crate::access::require_channel_access;
use crate::auth::user_id_for_token;
use crate::proto::thehideout::v1::PendingAttachment;
use crate::storage::Storage;

/// Tunables of the attachment pipeline. `Default` is production.
#[derive(Debug, Clone, Copy)]
pub struct AttachmentSettings {
    /// Largest accepted upload (`THEHIDEOUT_MAX_UPLOAD_BYTES`).
    pub max_upload_bytes: u64,
    /// How long an upload waits for a SendMessage before it expires.
    pub pending_ttl: Duration,
    /// Unexpired pending uploads one user may hold.
    pub max_pending_per_user: usize,
    /// Uploads one user may start per minute.
    pub uploads_per_minute: usize,
    /// How often the sweeper runs.
    pub sweep_interval: Duration,
    /// Extra time an expired upload's objects survive before deletion, so a
    /// SendMessage that validated just before expiry cannot lose its files.
    pub sweep_grace: Duration,
    /// Longest wait for the next piece of an upload body.
    pub idle_timeout: Duration,
}

pub const DEFAULT_MAX_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;

impl Default for AttachmentSettings {
    fn default() -> Self {
        Self {
            max_upload_bytes: DEFAULT_MAX_UPLOAD_BYTES,
            pending_ttl: Duration::from_secs(3600),
            max_pending_per_user: 20,
            uploads_per_minute: 30,
            sweep_interval: Duration::from_secs(60),
            sweep_grace: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(30),
        }
    }
}

pub fn attachments_route() -> Router<AppState> {
    Router::new().route("/attachments", post(upload))
}

#[derive(Deserialize)]
struct UploadQuery {
    channel_id: u64,
    filename: Option<String>,
}

/// An HTTP error with a Connect-style JSON body.
struct HttpError(StatusCode, &'static str, String);

impl HttpError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self(status, code, message.into())
    }

    fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "code": self.1, "message": self.2 });
        (self.0, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
    }
}

fn from_connect(e: connectrpc::ConnectError) -> HttpError {
    match e.code {
        ErrorCode::Unauthenticated => HttpError::new(StatusCode::UNAUTHORIZED, "unauthenticated", "invalid or expired session token"),
        ErrorCode::NotFound => HttpError::new(StatusCode::NOT_FOUND, "not_found", "channel not found"),
        ErrorCode::ResourceExhausted => HttpError::new(StatusCode::TOO_MANY_REQUESTS, "resource_exhausted", "too many uploads, try again later"),
        _ => HttpError::internal(),
    }
}

async fn upload(State(state): State<AppState>, request: Request) -> Response {
    match handle(state, request).await {
        Ok(pending) => match serde_json::to_vec(&pending) {
            Ok(body) => (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], body).into_response(),
            Err(_) => HttpError::internal().into_response(),
        },
        Err(e) => e.into_response(),
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

async fn handle(state: AppState, request: Request) -> Result<PendingAttachment, HttpError> {
    let settings = state.attachments;
    let (parts, body) = request.into_parts();

    let token = bearer(&parts.headers)
        .ok_or_else(|| HttpError::new(StatusCode::UNAUTHORIZED, "unauthenticated", "missing or malformed authorization header"))?;
    let user_id = user_id_for_token(&state, token).await.map_err(from_connect)?;

    let query = Query::<UploadQuery>::try_from_uri(&parts.uri)
        .map_err(|_| HttpError::new(StatusCode::BAD_REQUEST, "invalid_argument", "channel_id is required"))?
        .0;
    // Not found for strangers too, so channels are not leaked.
    require_channel_access(&state, user_id, query.channel_id).await.map_err(from_connect)?;
    let Some(storage) = state.storage.clone() else {
        return Err(HttpError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "media storage is not available"));
    };

    let declared_len: u64 = parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| HttpError::new(StatusCode::LENGTH_REQUIRED, "invalid_argument", "Content-Length is required"))?;
    if declared_len == 0 {
        return Err(HttpError::new(StatusCode::BAD_REQUEST, "invalid_argument", "the file is empty"));
    }
    if declared_len > settings.max_upload_bytes {
        return Err(HttpError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_argument",
            format!("files may be at most {} bytes", settings.max_upload_bytes),
        ));
    }

    state
        .limiter
        .check(&format!("attachment-upload:{user_id}"), settings.uploads_per_minute, Duration::from_secs(60))
        .map_err(from_connect)?;
    // Racing uploads can overshoot the cap by a few; the per-minute limit bounds that.
    if pending::count(&state, user_id).await.map_err(from_connect)? >= settings.max_pending_per_user {
        return Err(HttpError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "resource_exhausted",
            "too many pending uploads; send a message with them or wait for them to expire",
        ));
    }

    let job = UploadJob {
        user_id,
        channel_id: query.channel_id,
        filename: sniff::sanitize_filename(query.filename.as_deref().unwrap_or("")),
        declared_type: parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(sniff::sane_mime),
        declared_len,
    };
    // Detached on purpose, see the module docs.
    tokio::spawn(run_upload(state, storage, job, body))
        .await
        .map_err(|_| HttpError::internal())?
}

struct UploadJob {
    user_id: u64,
    channel_id: u64,
    filename: String,
    declared_type: Option<String>,
    declared_len: u64,
}

/// Next piece of the body, enforcing the idle timeout and the declared length.
async fn next_chunk(
    stream: &mut (impl futures::Stream<Item = Result<Bytes, axum::Error>> + Unpin),
    idle: Duration,
    size: &mut u64,
    declared_len: u64,
    hasher: &mut Sha256,
) -> Result<Option<Bytes>, HttpError> {
    let chunk = match tokio::time::timeout(idle, stream.next()).await {
        Err(_) => return Err(HttpError::new(StatusCode::REQUEST_TIMEOUT, "deadline_exceeded", "upload stalled")),
        Ok(None) => return Ok(None),
        Ok(Some(Err(_))) => return Err(HttpError::new(StatusCode::BAD_REQUEST, "invalid_argument", "upload interrupted")),
        Ok(Some(Ok(chunk))) => chunk,
    };
    *size += chunk.len() as u64;
    if *size > declared_len {
        return Err(HttpError::new(StatusCode::BAD_REQUEST, "invalid_argument", "body is longer than Content-Length"));
    }
    hasher.update(&chunk);
    Ok(Some(chunk))
}

async fn run_upload(state: AppState, storage: Storage, job: UploadJob, body: Body) -> Result<PendingAttachment, HttpError> {
    let settings = state.attachments;
    let id = state.snowflake.next_id();
    let hex = random_attachment_id();
    let key = attachment_key(job.channel_id, &hex);
    let thumbnail_key = attachment_thumbnail_key(job.channel_id, &hex);
    let ttl = chrono::Duration::from_std(settings.pending_ttl).unwrap_or_else(|_| chrono::Duration::hours(1));

    // Registry row first: whatever happens next, the sweeper knows the keys.
    let sweep_hour = pending::register_sweep(&state, id, job.user_id, &key, &thumbnail_key, Utc::now() + ttl)
        .await
        .map_err(|_| HttpError::internal())?;

    let result = stream_and_finish(&state, &storage, &job, body, &key, &thumbnail_key, id, sweep_hour).await;
    if result.is_err() {
        // The upload failed or was refused: nothing will ever reference it.
        let _ = storage.delete(&key).await;
        let _ = storage.delete(&thumbnail_key).await;
        pending::forget_sweep(&state, sweep_hour, id).await;
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn stream_and_finish(
    state: &AppState,
    storage: &Storage,
    job: &UploadJob,
    body: Body,
    key: &str,
    thumbnail_key: &str,
    id: u64,
    sweep_hour: i64,
) -> Result<PendingAttachment, HttpError> {
    let settings = state.attachments;
    let mut stream = body.into_data_stream();
    let mut hasher = Sha256::new();
    let mut size = 0u64;

    // The content type is stored with the object, so read enough to sniff first.
    let mut head: Vec<Bytes> = Vec::new();
    let mut head_len = 0;
    while head_len < SNIFF_LEN {
        match next_chunk(&mut stream, settings.idle_timeout, &mut size, job.declared_len, &mut hasher).await? {
            Some(chunk) => {
                head_len += chunk.len();
                head.push(chunk);
            }
            None => break,
        }
    }
    let head_bytes: Vec<u8> = head.iter().flat_map(|b| b.iter().copied()).take(SNIFF_LEN).collect();
    let sniffed = sniff::sniff(&head_bytes);
    let content_type = sniffed
        .map(str::to_owned)
        .or_else(|| job.declared_type.clone())
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    let serving = sniff::serving(&content_type, sniffed.is_some());
    let disposition = sniff::content_disposition(serving.inline, &job.filename);

    let mut upload = storage
        .begin_upload(key, &serving.content_type, Some(&disposition))
        .await
        .map_err(|e| {
            tracing::warn!(error = %format!("{e:#}"), "starting attachment upload failed");
            HttpError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "media storage unavailable")
        })?;

    let streamed: Result<(), HttpError> = async {
        for chunk in head.drain(..) {
            upload.write(chunk).await.map_err(storage_error)?;
        }
        while let Some(chunk) = next_chunk(&mut stream, settings.idle_timeout, &mut size, job.declared_len, &mut hasher).await? {
            upload.write(chunk).await.map_err(storage_error)?;
        }
        if size != job.declared_len {
            return Err(HttpError::new(StatusCode::BAD_REQUEST, "invalid_argument", "body is shorter than Content-Length"));
        }
        Ok(())
    }
    .await;
    if let Err(e) = streamed {
        let _ = upload.abort().await;
        return Err(e);
    }
    upload.finish().await.map_err(storage_error)?;

    let sha256: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let image = if matches!(content_type.as_str(), "image/png" | "image/jpeg" | "image/webp" | "image/gif") && size <= ATTACHMENT_IMAGE_MAX_BYTES {
        inspect_stored_image(storage, key, thumbnail_key).await
    } else {
        None
    };

    let expires_at = Utc::now() + chrono::Duration::from_std(settings.pending_ttl).unwrap_or_else(|_| chrono::Duration::hours(1));
    let pending = Pending {
        id,
        channel_id: job.channel_id,
        key: key.to_owned(),
        filename: job.filename.clone(),
        content_type,
        size,
        width: image.as_ref().map(|i| i.width),
        height: image.as_ref().map(|i| i.height),
        thumbnail: image.as_ref().and_then(|i| i.thumbnail_bytes.map(|_| thumbnail_key.to_owned())),
        thumbnail_bytes: image.as_ref().and_then(|i| i.thumbnail_bytes).unwrap_or(0),
        sha256,
        expires_at,
        sweep_hour: Some(sweep_hour),
    };
    let ttl_secs = i32::try_from(settings.pending_ttl.as_secs().max(1)).unwrap_or(3600);
    pending::insert(state, job.user_id, &pending, ttl_secs)
        .await
        .map_err(|_| HttpError::internal())?;

    Ok(PendingAttachment {
        id,
        filename: pending.filename,
        content_type: pending.content_type,
        size,
        expires_at: crate::convert::datetime_to_proto(expires_at).into(),
        ..Default::default()
    })
}

fn storage_error(e: anyhow::Error) -> HttpError {
    tracing::warn!(error = %format!("{e:#}"), "attachment upload to object storage failed");
    HttpError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "media storage unavailable")
}

struct StoredImage {
    width: u32,
    height: u32,
    /// Size of the thumbnail object, `None` if there is none.
    thumbnail_bytes: Option<u64>,
}

/// Reads the uploaded image back, measures it and stores its thumbnail. Any
/// failure just means no dimensions/thumbnail; the upload itself stands.
async fn inspect_stored_image(storage: &Storage, key: &str, thumbnail_key: &str) -> Option<StoredImage> {
    let _slot = acquire_decode_slot().await?;
    let object = storage.get(key).await.ok()??;
    let chunks: Vec<Bytes> = object.stream.try_collect().await.ok()?;
    let data = chunks.concat();
    let info = tokio::task::spawn_blocking(move || inspect_attachment_image(&data)).await.ok()??;
    let mut thumbnail_bytes = None;
    if let Some(thumb) = info.thumbnail {
        let len = thumb.len() as u64;
        match storage.put(thumbnail_key, thumb, "image/webp").await {
            Ok(()) => thumbnail_bytes = Some(len),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "storing attachment thumbnail failed"),
        }
    }
    Some(StoredImage {
        width: info.width,
        height: info.height,
        thumbnail_bytes,
    })
}
