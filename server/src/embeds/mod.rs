//! Link embeds: after a message is created, the URLs in it are unfurled in
//! the background (title, description, site name, a WebP thumbnail), stored in
//! `message_embeds` and announced with `message_updated`.
//!
//! - [`extract`]: which URLs of a message count (only code spans and fences are skipped).
//! - [`ssrf`] + [`fetch`]: the SSRF-safe HTTP client every request goes through.
//! - [`html`]: metadata of the page head.
//! - [`provider`]: how a URL becomes an [`provider::Unfurled`] (text + media
//!   sources); the generic OpenGraph provider today, the seam for site-specific ones.
//!
//! Pipeline per URL: choose provider → (cache) → provider describes the link →
//! the media step downloads the sources through the same safe client (today:
//! the first image, re-encoded as the thumbnail) → rows are stored.
//!
//! Failure is silent by design: a URL that cannot be fetched, is blocked,
//! times out or has nothing to show simply gets no embed; if none of the
//! message's URLs yields one, nothing is stored or published.
//!
//! Thumbnails are never shared between embeds: each embed owns an object
//! (`embeds/{channel_id}/{id}.webp`) with its own accounting row, counted
//! against the guild (DMs: global only, with the author as owner). The per-URL
//! cache (`embed_cache`) therefore holds only metadata and the thumbnail's
//! source URL; a cache hit still fetches and re-encodes the image.
//!
//! Crash window: the thumbnail object and its accounting row are written
//! before the embed rows; a crash in between leaves an accounted object no
//! message points at, which the admin file list shows and can delete.

pub mod extract;
pub mod fetch;
pub mod html;
pub mod provider;
pub mod ssrf;

use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use tokio::sync::Semaphore;
use url::Url;

use crate::AppState;
use crate::auth::internal;
use crate::convert::id_to_db;
use crate::media::{self, EmbedThumbnail, Owner};
use crate::proto::thehideout::v1::Embed;
use crate::services::message::publish_message_updated;
use crate::snowflake::bucket_of;
use fetch::Fetcher;
use provider::{AssetKind, Ctx, MediaSource, Provider, Unfurled};
use ssrf::Policy;

/// Largest HTML prefix read from a page.
pub const HTML_CAP: usize = 1024 * 1024;
/// Largest image downloaded for a thumbnail or a direct image link.
pub const IMAGE_CAP: usize = 5 * 1024 * 1024;
/// Messages unfurled at once; further ones are dropped (and logged).
pub const MAX_CONCURRENT_JOBS: usize = 8;

/// Tunables of the embed pipeline. `Default` is production.
#[derive(Clone)]
pub struct EmbedSettings {
    /// `THEHIDEOUT_EMBEDS=on|off`.
    pub enabled: bool,
    /// Lets the fetcher reach IPv4 loopback on any port. For tests only: it is
    /// deliberately not configurable from the environment.
    pub allow_loopback_for_tests: bool,
    pub connect_timeout: Duration,
    /// Deadline of one fetch (all redirects and the body).
    pub total_timeout: Duration,
    pub max_redirects: usize,
    pub html_cap: usize,
    pub image_cap: usize,
    /// How long an unfurl result is reused.
    pub cache_ttl: Duration,
    /// How long "nothing useful found" is remembered.
    pub negative_cache_ttl: Duration,
    /// Contact shown in the user agent (`THEHIDEOUT_EMBEDS_CONTACT`), e.g. a URL or `mailto:`.
    pub contact: Option<String>,
    slots: Arc<Semaphore>,
    max_jobs: usize,
}

impl Default for EmbedSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_loopback_for_tests: false,
            connect_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(5),
            max_redirects: 3,
            html_cap: HTML_CAP,
            image_cap: IMAGE_CAP,
            cache_ttl: Duration::from_secs(24 * 3600),
            negative_cache_ttl: Duration::from_secs(3600),
            contact: None,
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS)),
            max_jobs: MAX_CONCURRENT_JOBS,
        }
    }
}

impl EmbedSettings {
    pub fn disabled() -> Self {
        Self { enabled: false, ..Self::default() }
    }

    /// Allows only this many jobs at once.
    pub fn with_max_concurrent(mut self, jobs: usize) -> Self {
        self.slots = Arc::new(Semaphore::new(jobs));
        self.max_jobs = jobs;
        self
    }

    /// Jobs running right now (tests wait for zero to know a negative outcome is final).
    pub fn in_flight(&self) -> usize {
        self.max_jobs - self.slots.available_permits()
    }

    fn fetcher(&self) -> Fetcher {
        let user_agent = match &self.contact {
            Some(c) => format!("thehideout-embed/0.1 (link preview bot; +{c})"),
            None => "thehideout-embed/0.1 (link preview bot)".to_owned(),
        };
        Fetcher {
            policy: Policy { allow_loopback_for_tests: self.allow_loopback_for_tests },
            connect_timeout: self.connect_timeout,
            total_timeout: self.total_timeout,
            max_redirects: self.max_redirects,
            user_agent,
        }
    }
}

/// A freshly created message to unfurl.
pub struct Job {
    pub channel_id: u64,
    /// 0 for DM channels.
    pub guild_id: u64,
    pub message_id: u64,
    pub author_id: u64,
    pub content: String,
}

/// Starts unfurling the URLs of a message in the background, if there are
/// any and a job slot is free. Never blocks the caller.
pub fn spawn(state: &AppState, job: Job) {
    if !state.embeds.enabled {
        return;
    }
    let urls = extract::extract_urls(&job.content);
    if urls.is_empty() {
        return;
    }
    let Ok(permit) = state.embeds.slots.clone().try_acquire_owned() else {
        tracing::warn!(message_id = job.message_id, "embed queue saturated; not unfurling this message");
        return;
    };
    let state = state.clone();
    tokio::spawn(async move {
        run(&state, &job, urls).await;
        drop(permit);
    });
}

/// A described link with its media downloaded and ready to store.
struct Ready {
    url: String,
    unfurled: Unfurled,
    thumbnail: Option<EmbedThumbnail>,
}

/// Unfurls every URL, stores the embeds that have something to show, and
/// publishes `message_updated`. Awaitable so tests can run it directly.
pub async fn run(state: &AppState, job: &Job, urls: Vec<String>) {
    let unfurled: Vec<Ready> = join_all(urls.into_iter().map(|u| unfurl(state, u))).await.into_iter().flatten().collect();
    if unfurled.is_empty() {
        return;
    }
    if let Err(e) = store(state, job, unfurled).await {
        tracing::warn!(message_id = job.message_id, error = %e.message.as_deref().unwrap_or("?"), "storing embeds failed");
        return;
    }
    if let Err(e) = publish_message_updated(state, job.channel_id, job.message_id).await {
        tracing::warn!(message_id = job.message_id, error = %e.message.as_deref().unwrap_or("?"), "publishing message_updated failed");
    }
}

/// Stores thumbnails and writes the embed rows (one batch in the message's
/// partition). A failure undoes what was stored.
async fn store(state: &AppState, job: &Job, unfurled: Vec<Ready>) -> Result<(), connectrpc::ConnectError> {
    let owner = if job.guild_id != 0 {
        Owner { kind: "guild", id: job.guild_id, guild_id: Some(job.guild_id), label: "embed" }
    } else {
        Owner { kind: "user", id: job.author_id, guild_id: None, label: "embed" }
    };
    let mut stored_keys: Vec<String> = Vec::new();
    // (embed, thumbnail key)
    let mut rows: Vec<(Ready, Option<String>)> = Vec::new();
    for item in unfurled {
        let mut key = None;
        if let (Some(thumb), Some(storage)) = (&item.thumbnail, &state.storage) {
            let candidate = media::embed_thumbnail_key(job.channel_id);
            let size = thumb.bytes.len() as u64;
            match storage.put(&candidate, thumb.bytes.clone(), "image/webp").await {
                Ok(()) => match media::record(state, &candidate, owner, size).await {
                    Ok(()) => {
                        stored_keys.push(candidate.clone());
                        key = Some(candidate);
                    }
                    Err(_) => {
                        let _ = storage.delete(&candidate).await;
                    }
                },
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "storing embed thumbnail failed"),
            }
        }
        // An embed whose only content was an image that could not be stored has nothing left.
        if item.unfurled.has_text() || key.is_some() {
            rows.push((item, key));
        }
    }
    if rows.is_empty() {
        return Ok(());
    }
    let mut batch = scylla::statement::batch::Batch::new(scylla::statement::batch::BatchType::Logged);
    let mut values = Vec::with_capacity(rows.len());
    for (position, (item, key)) in rows.iter().enumerate() {
        batch.append_statement(
            "INSERT INTO message_embeds (channel_id, bucket, message_id, position, url, title, description, \
             site_name, thumbnail, thumbnail_width, thumbnail_height) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        );
        let thumb = key.as_ref().and(item.thumbnail.as_ref());
        values.push((
            id_to_db(job.channel_id),
            bucket_of(job.message_id),
            id_to_db(job.message_id),
            position as i32,
            item.url.as_str(),
            item.unfurled.title.as_str(),
            item.unfurled.description.as_str(),
            item.unfurled.site_name.as_str(),
            key.as_deref(),
            thumb.map(|t| t.width as i32),
            thumb.map(|t| t.height as i32),
        ));
    }
    if let Err(e) = state.session.batch(&batch, values).await {
        // Keys stored for rows that were never written (or were all dropped) are orphans: undo them.
        for key in &stored_keys {
            media::delete_record(state, key).await;
            if let Some(storage) = &state.storage {
                let _ = storage.delete(key).await;
            }
        }
        return Err(internal(e));
    }
    Ok(())
}

/// Unfurls one URL: provider (or cache) for the description, then the media
/// step. `None` if there is nothing useful to show.
async fn unfurl(state: &AppState, url: String) -> Option<Ready> {
    let parsed = Url::parse(&url).ok()?;
    let fetcher = state.embeds.fetcher();
    let mut unfurled = match cache_get(state, &url).await {
        Some(Some(unfurled)) => unfurled,
        Some(None) => return None,
        None => {
            let ctx = Ctx { fetcher: &fetcher, html_cap: state.embeds.html_cap, image_cap: state.embeds.image_cap };
            match Provider::for_url(&parsed).unfurl(&ctx, &parsed).await {
                Ok(found) => {
                    cache_put(state, &url, found.as_ref()).await;
                    found?
                }
                Err(e) => {
                    tracing::debug!(%url, error = %e, "unfurl failed");
                    return None;
                }
            }
        }
    };
    let thumbnail = if state.storage.is_some() { thumbnail_of(&fetcher, &mut unfurled, state.embeds.image_cap).await } else { None };
    (unfurled.has_text() || thumbnail.is_some()).then_some(Ready { url, unfurled, thumbnail })
}

/// The media step as it is today: the first image becomes the embed's
/// thumbnail (downloaded, decoded, fit within 400x400, re-encoded as WebP).
/// Videos and further images are not downloaded yet.
async fn thumbnail_of(fetcher: &Fetcher, unfurled: &mut Unfurled, cap: usize) -> Option<EmbedThumbnail> {
    let first = unfurled.media.iter().position(|m| m.kind == AssetKind::Image)?;
    // Only `media[0]` can have been prefetched.
    if first == 0
        && let Some(bytes) = unfurled.prefetched.take()
    {
        return media::embed_thumbnail(bytes).await;
    }
    let source = unfurled.media[first].clone();
    let bytes = download_image(fetcher, &source, cap).await?;
    media::embed_thumbnail(bytes).await
}

/// Downloads an image source into memory (for decoding); any failure just
/// means no thumbnail.
async fn download_image(fetcher: &Fetcher, source: &MediaSource, cap: usize) -> Option<Vec<u8>> {
    let url = Url::parse(&source.url).ok()?;
    let result = fetcher
        .within(async {
            let reply = fetcher.get(&url).await?;
            if !reply.content_type.starts_with("image/") {
                return Err(fetch::FetchError::NotAllowedType(reply.content_type));
            }
            reply.read_capped(cap).await
        })
        .await;
    match result {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            tracing::debug!(image_url = %source.url, error = %e, "embed media download failed");
            None
        }
    }
}

type CacheRow = (Option<bool>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);

/// `None`: not cached. `Some(None)`: cached as "nothing useful".
async fn cache_get(state: &AppState, url: &str) -> Option<Option<Unfurled>> {
    let row = state
        .session
        .execute_unpaged("SELECT found, title, description, site_name, author, media FROM embed_cache WHERE url = ?", (url,))
        .await
        .ok()?
        .into_rows_result()
        .ok()?
        .maybe_first_row::<CacheRow>()
        .ok()??;
    let (found, title, description, site_name, author, media) = row;
    if found != Some(true) {
        return Some(None);
    }
    Some(Some(Unfurled {
        title: title.unwrap_or_default(),
        description: description.unwrap_or_default(),
        site_name: site_name.unwrap_or_default(),
        author: author.unwrap_or_default(),
        media: media.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or_default(),
        prefetched: None,
    }))
}

/// Best effort: a failed cache write only costs a refetch later.
async fn cache_put(state: &AppState, url: &str, found: Option<&Unfurled>) {
    let ttl = if found.is_some() { state.embeds.cache_ttl } else { state.embeds.negative_cache_ttl };
    let ttl = i32::try_from(ttl.as_secs()).unwrap_or(i32::MAX).max(1);
    let empty = Unfurled::default();
    let f = found.unwrap_or(&empty);
    let media = serde_json::to_string(&f.media).unwrap_or_else(|_| "[]".to_owned());
    let result = state
        .session
        .execute_unpaged(
            "INSERT INTO embed_cache (url, found, title, description, site_name, author, media) \
             VALUES (?, ?, ?, ?, ?, ?, ?) USING TTL ?",
            (url, found.is_some(), &f.title, &f.description, &f.site_name, &f.author, media, ttl),
        )
        .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "writing embed cache failed");
    }
}

/// Embed rows as read back by `load_embeds`.
pub type EmbedRow = (i64, i32, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i32>, Option<i32>);

pub fn row_to_embed(row: EmbedRow) -> (u64, Embed) {
    let (message_id, _, url, title, description, site_name, thumbnail, width, height) = row;
    let thumbnail = thumbnail.filter(|t| !t.is_empty());
    (
        crate::convert::id_from_db(message_id),
        Embed {
            url: url.unwrap_or_default(),
            title: title.unwrap_or_default(),
            description: description.unwrap_or_default(),
            site_name: site_name.unwrap_or_default(),
            thumbnail_width: thumbnail.as_ref().and(width).map(|v| v as u32),
            thumbnail_height: thumbnail.as_ref().and(height).map(|v| v as u32),
            thumbnail,
            ..Default::default()
        },
    )
}
