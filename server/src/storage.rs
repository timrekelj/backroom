//! Object storage for media, over any S3-compatible service.
//!
//! Built on the `object_store` crate: small, maintained (Apache Arrow), speaks
//! plain S3 (so SeaweedFS locally and AWS/R2/B2/... in production differ only
//! in configuration) and ships an in-memory backend used by unit tests.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::aws::AmazonS3Builder;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    Attribute, Attributes, ClientOptions, GetOptions, GetRange, ObjectStore, ObjectStoreExt, PutMultipartOptions, PutOptions,
    PutPayload, RetryConfig, WriteMultipart,
};

use crate::config::S3Config;

/// Per-request timeout towards the object store.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Storage {
    store: Arc<dyn ObjectStore>,
}

/// Metadata of a stored object.
#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub size: u64,
    /// Content type recorded at upload time, if the backend kept it.
    pub content_type: Option<String>,
    /// Content disposition recorded at upload time, if the backend kept it.
    pub content_disposition: Option<String>,
}

/// A stored object being streamed out.
pub struct ObjectBody {
    pub info: ObjectInfo,
    /// The byte range `stream` yields (the whole object unless a range was asked for).
    pub range: std::ops::Range<u64>,
    pub stream: BoxStream<'static, object_store::Result<Bytes>>,
}

/// A byte range of an HTTP `Range` request, resolved against the object size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// `a-b` (inclusive end)
    Bounded(u64, u64),
    /// `a-`
    From(u64),
    /// `-n`
    Suffix(u64),
}

/// Parses a single-range `Range: bytes=…` header; `None` for anything else
/// (multiple ranges, other units, junk), which callers answer with the full body.
pub fn parse_range_header(value: &str) -> Option<ByteRange> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    match (a.trim(), b.trim()) {
        ("", n) => n.parse().ok().map(ByteRange::Suffix),
        (a, "") => a.parse().ok().map(ByteRange::From),
        (a, b) => {
            let (a, b): (u64, u64) = (a.parse().ok()?, b.parse().ok()?);
            (a <= b).then_some(ByteRange::Bounded(a, b))
        }
    }
}

/// An object being written in parts, so large bodies never sit in memory
/// whole. Dropping it without [`Upload::finish`] leaves the multipart upload
/// open on the backend; call [`Upload::abort`] instead.
pub struct Upload {
    inner: WriteMultipart,
}

/// Size of one uploaded part, and parts in flight at once: bounds memory per
/// upload to about `PART_SIZE * (IN_FLIGHT + 1)`.
const PART_SIZE: usize = 5 * 1024 * 1024;
const IN_FLIGHT: usize = 2;

impl Upload {
    pub async fn write(&mut self, bytes: Bytes) -> Result<()> {
        self.inner.wait_for_capacity(IN_FLIGHT).await.context("uploading part")?;
        self.inner.put(bytes);
        Ok(())
    }

    pub async fn finish(self) -> Result<()> {
        self.inner.finish().await.context("completing upload")?;
        Ok(())
    }

    pub async fn abort(self) -> Result<()> {
        self.inner.abort().await.context("aborting upload")
    }
}

impl Storage {
    /// Connects (lazily) to the S3 service described by `config`.
    pub fn s3(config: &S3Config) -> Result<Self> {
        let store = AmazonS3Builder::new()
            .with_endpoint(&config.endpoint)
            .with_region(&config.region)
            .with_bucket_name(&config.bucket)
            .with_access_key_id(&config.access_key)
            .with_secret_access_key(&config.secret_key)
            .with_virtual_hosted_style_request(!config.path_style)
            .with_retry(RetryConfig {
                max_retries: 2,
                ..Default::default()
            })
            .with_client_options(
                ClientOptions::new()
                    .with_allow_http(config.endpoint.starts_with("http://"))
                    .with_timeout(REQUEST_TIMEOUT)
                    .with_connect_timeout(Duration::from_secs(5)),
            )
            .build()
            .context("configuring S3 storage")?;
        Ok(Self { store: Arc::new(store) })
    }

    /// A throwaway in-memory store, for unit tests.
    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(InMemory::new()),
        }
    }

    /// Verifies the store is reachable and the bucket exists.
    pub async fn check(&self) -> Result<()> {
        self.head(".healthcheck").await.map(|_| ()).context("probing object storage")
    }

    pub async fn put(&self, key: &str, bytes: Bytes, content_type: &str) -> Result<()> {
        let opts = PutOptions {
            attributes: attributes(content_type, None),
            ..Default::default()
        };
        self.store
            .put_opts(&path(key)?, PutPayload::from_bytes(bytes), opts)
            .await
            .with_context(|| format!("storing {key}"))?;
        Ok(())
    }

    /// Starts a streamed write of `key` with the given response attributes.
    pub async fn begin_upload(&self, key: &str, content_type: &str, content_disposition: Option<&str>) -> Result<Upload> {
        let opts = PutMultipartOptions {
            attributes: attributes(content_type, content_disposition),
            ..Default::default()
        };
        let upload = self
            .store
            .put_multipart_opts(&path(key)?, opts)
            .await
            .with_context(|| format!("starting upload of {key}"))?;
        Ok(Upload {
            inner: WriteMultipart::new_with_chunk_size(upload, PART_SIZE),
        })
    }

    /// `None` if the object does not exist.
    pub async fn get(&self, key: &str) -> Result<Option<ObjectBody>> {
        self.get_range(key, None).await
    }

    /// Like [`Storage::get`], for a byte range of the object. The range must
    /// already be satisfiable (see [`Storage::resolve_range`]).
    pub async fn get_range(&self, key: &str, range: Option<std::ops::Range<u64>>) -> Result<Option<ObjectBody>> {
        let options = GetOptions {
            range: range.map(GetRange::Bounded),
            ..Default::default()
        };
        match self.store.get_opts(&path(key)?, options).await {
            Ok(result) => {
                let info = ObjectInfo {
                    size: result.meta.size,
                    content_type: attribute_of(&result.attributes, &Attribute::ContentType),
                    content_disposition: attribute_of(&result.attributes, &Attribute::ContentDisposition),
                };
                Ok(Some(ObjectBody {
                    info,
                    range: result.range.clone(),
                    stream: result.into_stream(),
                }))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {key}")),
        }
    }

    /// `None` if the object does not exist.
    pub async fn head(&self, key: &str) -> Result<Option<ObjectInfo>> {
        match self.store.head(&path(key)?).await {
            Ok(meta) => Ok(Some(ObjectInfo {
                size: meta.size,
                content_type: None,
                content_disposition: None,
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("probing {key}")),
        }
    }

    /// Deletes the object; a missing object is not an error.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self.store.delete(&path(key)?).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e).with_context(|| format!("deleting {key}")),
        }
    }
}

fn path(key: &str) -> Result<Path> {
    Path::parse(key).with_context(|| format!("invalid storage key {key:?}"))
}

fn attributes(content_type: &str, content_disposition: Option<&str>) -> Attributes {
    let mut attributes = Attributes::new();
    attributes.insert(Attribute::ContentType, content_type.to_owned().into());
    if let Some(d) = content_disposition {
        attributes.insert(Attribute::ContentDisposition, d.to_owned().into());
    }
    attributes
}

fn attribute_of(attributes: &Attributes, attribute: &Attribute) -> Option<String> {
    attributes.get(attribute).map(|v| v.to_string())
}

/// Resolves an HTTP range against the object `size`: the half-open byte range
/// to read, or `None` if it is not satisfiable.
pub fn resolve_range(range: ByteRange, size: u64) -> Option<std::ops::Range<u64>> {
    let (start, end) = match range {
        ByteRange::Bounded(a, b) => (a, b.saturating_add(1).min(size)),
        ByteRange::From(a) => (a, size),
        ByteRange::Suffix(0) => return None,
        ByteRange::Suffix(n) => (size.saturating_sub(n), size),
    };
    (start < end).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryStreamExt;

    #[tokio::test]
    async fn put_get_head_delete_roundtrip() {
        let s = Storage::in_memory();
        assert!(s.get("a/b.webp").await.unwrap().is_none());
        s.put("a/b.webp", Bytes::from_static(b"hello"), "image/webp").await.unwrap();
        assert_eq!(s.head("a/b.webp").await.unwrap().unwrap().size, 5);
        let body = s.get("a/b.webp").await.unwrap().unwrap();
        assert_eq!(body.info.content_type.as_deref(), Some("image/webp"));
        let chunks: Vec<Bytes> = body.stream.try_collect().await.unwrap();
        assert_eq!(chunks.concat(), b"hello");
        s.delete("a/b.webp").await.unwrap();
        s.delete("a/b.webp").await.unwrap();
        assert!(s.get("a/b.webp").await.unwrap().is_none());
    }

    #[test]
    fn range_headers_parse_and_resolve() {
        assert_eq!(parse_range_header("bytes=0-99"), Some(ByteRange::Bounded(0, 99)));
        assert_eq!(parse_range_header("bytes=5-"), Some(ByteRange::From(5)));
        assert_eq!(parse_range_header("bytes=-5"), Some(ByteRange::Suffix(5)));
        for bad in ["", "bytes=", "bytes=1-2,4-5", "items=1-2", "bytes=9-1", "bytes=a-b", "bytes=-"] {
            assert_eq!(parse_range_header(bad), None, "{bad:?}");
        }
        assert_eq!(resolve_range(ByteRange::Bounded(0, 99), 10), Some(0..10));
        assert_eq!(resolve_range(ByteRange::Bounded(2, 3), 10), Some(2..4));
        assert_eq!(resolve_range(ByteRange::From(10), 10), None);
        assert_eq!(resolve_range(ByteRange::Suffix(3), 10), Some(7..10));
        assert_eq!(resolve_range(ByteRange::Suffix(30), 10), Some(0..10));
        assert_eq!(resolve_range(ByteRange::Suffix(0), 10), None);
    }

    #[tokio::test]
    async fn streamed_upload_roundtrip_with_range() {
        let s = Storage::in_memory();
        let mut up = s.begin_upload("a/big", "application/octet-stream", Some("attachment; filename*=UTF-8''x")).await.unwrap();
        up.write(Bytes::from_static(b"0123456789")).await.unwrap();
        up.write(Bytes::from_static(b"abcdef")).await.unwrap();
        up.finish().await.unwrap();
        let body = s.get_range("a/big", Some(8..12)).await.unwrap().unwrap();
        assert_eq!(body.range, 8..12);
        assert_eq!(body.info.size, 16);
        assert_eq!(body.info.content_disposition.as_deref(), Some("attachment; filename*=UTF-8''x"));
        let chunks: Vec<Bytes> = body.stream.try_collect().await.unwrap();
        assert_eq!(chunks.concat(), b"89ab");
        let mut up = s.begin_upload("a/aborted", "x/y", None).await.unwrap();
        up.write(Bytes::from_static(b"zz")).await.unwrap();
        up.abort().await.unwrap();
        assert!(s.get("a/aborted").await.unwrap().is_none());
    }

    #[test]
    fn rejects_odd_keys() {
        assert!(path("a/../b").is_err());
        assert!(path("a//b").is_err());
    }
}
