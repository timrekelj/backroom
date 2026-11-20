//! The SSRF-safe HTTP client of the embed fetcher.
//!
//! Every hop (the URL itself and each redirect, followed manually) is
//! screened by [`ssrf::vet`]: scheme, port, and every resolved address. The
//! connection then goes to the vetted address (pinned through the client's
//! resolver override, so nothing is resolved a second time), and the peer
//! address of the established connection is checked once more. No proxies, no
//! cookies, no automatic redirects, no compression (so no decompression
//! bombs); bodies are read incrementally up to a cap chosen per call, either
//! into memory ([`Reply::read_capped`]) or straight into object storage
//! ([`Reply::download_to_storage`]) so large media never sit in memory.

use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use reqwest::{Client, Response, StatusCode};
use url::{Host, Url};

use super::ssrf::{self, Blocked, Policy};
use crate::storage::Storage;

#[derive(Debug)]
pub enum FetchError {
    Blocked(Blocked),
    TooManyRedirects,
    BadRedirect,
    Status(StatusCode),
    NotAllowedType(String),
    TooLarge,
    Timeout,
    Http(String),
    Storage(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(b) => write!(f, "blocked: {b}"),
            Self::TooManyRedirects => write!(f, "too many redirects"),
            Self::BadRedirect => write!(f, "redirect without a usable Location"),
            Self::Status(s) => write!(f, "HTTP status {s}"),
            Self::NotAllowedType(t) => write!(f, "content type {t:?} is not accepted"),
            Self::TooLarge => write!(f, "response too large"),
            Self::Timeout => write!(f, "timed out"),
            Self::Http(e) => write!(f, "request failed: {e}"),
            Self::Storage(e) => write!(f, "storing the download failed: {e}"),
        }
    }
}

impl From<Blocked> for FetchError {
    fn from(b: Blocked) -> Self {
        Self::Blocked(b)
    }
}

/// Limits and identity of the client.
#[derive(Debug, Clone)]
pub struct Fetcher {
    pub policy: Policy,
    pub connect_timeout: Duration,
    /// Deadline for one whole fetch: all redirect hops and the body.
    pub total_timeout: Duration,
    pub max_redirects: usize,
    pub user_agent: String,
}

/// A response whose headers were accepted; the body is still unread.
pub struct Reply {
    pub response: Response,
    /// URL after redirects.
    pub url: Url,
    /// Lower-cased media type without parameters.
    pub content_type: String,
    /// Body bytes handed out so far (for the cap).
    read: usize,
}

impl Fetcher {
    /// GET `url`, following up to `max_redirects` redirects with every hop
    /// vetted. Only 2xx replies are returned.
    pub async fn get(&self, url: &Url) -> Result<Reply, FetchError> {
        let mut url = url.clone();
        for hop in 0..=self.max_redirects {
            let addr = ssrf::vet(&url, self.policy).await?;
            let response = self
                .client(&url, addr)?
                .get(url.clone())
                .header(ACCEPT, "text/html,application/xhtml+xml,image/*;q=0.8,*/*;q=0.1")
                .header(ACCEPT_LANGUAGE, "en;q=0.9,*;q=0.5")
                .send()
                .await
                .map_err(map_error)?;
            // The connected peer must be the vetted address (never trust the client alone).
            if let Some(peer) = response.remote_addr()
                && ssrf::is_blocked(peer.ip(), self.policy)
            {
                return Err(Blocked::Address(peer.ip()).into());
            }
            let status = response.status();
            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                if hop == self.max_redirects {
                    return Err(FetchError::TooManyRedirects);
                }
                let location = response.headers().get(LOCATION).and_then(|v| v.to_str().ok()).ok_or(FetchError::BadRedirect)?;
                url = url.join(location).map_err(|_| FetchError::BadRedirect)?;
                continue;
            }
            if !status.is_success() {
                return Err(FetchError::Status(status));
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .map(|v| v.trim().to_ascii_lowercase())
                .unwrap_or_default();
            return Ok(Reply { response, url, content_type, read: 0 });
        }
        Err(FetchError::TooManyRedirects)
    }

    fn client(&self, url: &Url, addr: std::net::SocketAddr) -> Result<Client, FetchError> {
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(self.connect_timeout)
            .timeout(self.total_timeout)
            .user_agent(&self.user_agent)
            .pool_max_idle_per_host(0);
        // Pin the vetted address for the host name; IP literals connect to themselves.
        if let Some(Host::Domain(name)) = url.host() {
            builder = builder.resolve(name, addr);
        }
        builder.build().map_err(|e| FetchError::Http(e.to_string()))
    }

    /// The same client with another deadline, e.g. for a large download.
    pub fn with_total_timeout(&self, total_timeout: Duration) -> Self {
        Self { total_timeout, ..self.clone() }
    }

    /// Runs `fut` under the total deadline.
    pub async fn within<T>(&self, fut: impl Future<Output = Result<T, FetchError>>) -> Result<T, FetchError> {
        tokio::time::timeout(self.total_timeout, fut).await.map_err(|_| FetchError::Timeout)?
    }
}

fn map_error(e: reqwest::Error) -> FetchError {
    if e.is_timeout() { FetchError::Timeout } else { FetchError::Http(e.without_url().to_string()) }
}

impl Reply {
    /// Reads the body of an HTML page: up to `cap` bytes, stopping once the
    /// head is complete (`</head`). A longer document is not an error: the
    /// prefix is what gets parsed.
    pub async fn read_html_prefix(mut self, cap: usize) -> Result<Vec<u8>, FetchError> {
        let mut body = Vec::new();
        while let Some(chunk) = self.response.chunk().await.map_err(map_error)? {
            let scan_from = body.len().saturating_sub(6);
            body.extend_from_slice(&chunk);
            let closed = body[scan_from..].windows(6).any(|w| w.eq_ignore_ascii_case(b"</head"));
            if closed || body.len() >= cap {
                body.truncate(cap);
                break;
            }
        }
        Ok(body)
    }

    /// The next piece of the body; fails with `TooLarge` as soon as the
    /// bytes handed out (or a declared `Content-Length`) exceed `cap`.
    pub async fn chunk(&mut self, cap: usize) -> Result<Option<Bytes>, FetchError> {
        if self.read == 0 {
            let declared = self.response.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
            if declared.is_some_and(|n| n > cap as u64) {
                return Err(FetchError::TooLarge);
            }
        }
        let Some(chunk) = self.response.chunk().await.map_err(map_error)? else { return Ok(None) };
        self.read += chunk.len();
        if self.read > cap {
            return Err(FetchError::TooLarge);
        }
        Ok(Some(chunk))
    }

    /// Reads the whole body into memory, failing if it exceeds `cap` bytes.
    pub async fn read_capped(mut self, cap: usize) -> Result<Vec<u8>, FetchError> {
        let mut body = Vec::new();
        while let Some(chunk) = self.chunk(cap).await? {
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// Streams the body into object storage under `key` without buffering it
    /// (memory use is one chunk plus the store's part buffer), failing with
    /// `TooLarge` past `cap` bytes; a failed download leaves no object behind.
    /// Returns the size stored.
    pub async fn download_to_storage(mut self, cap: usize, storage: &Storage, key: &str, content_type: &str) -> Result<u64, FetchError> {
        let storage_error = |e: anyhow::Error| FetchError::Storage(format!("{e:#}"));
        let mut upload = storage.begin_upload(key, content_type, None).await.map_err(storage_error)?;
        let mut size = 0u64;
        loop {
            match self.chunk(cap).await {
                Ok(Some(chunk)) => {
                    size += chunk.len() as u64;
                    if let Err(e) = upload.write(chunk).await {
                        let _ = upload.abort().await;
                        return Err(storage_error(e));
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = upload.abort().await;
                    return Err(e);
                }
            }
        }
        upload.finish().await.map_err(storage_error)?;
        Ok(size)
    }
}
