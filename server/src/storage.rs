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
use object_store::{Attribute, Attributes, ClientOptions, ObjectStore, ObjectStoreExt, PutOptions, PutPayload, RetryConfig};

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
}

/// A stored object being streamed out.
pub struct ObjectBody {
    pub info: ObjectInfo,
    pub stream: BoxStream<'static, object_store::Result<Bytes>>,
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
        let mut attributes = Attributes::new();
        attributes.insert(Attribute::ContentType, content_type.to_owned().into());
        let opts = PutOptions {
            attributes,
            ..Default::default()
        };
        self.store
            .put_opts(&path(key)?, PutPayload::from_bytes(bytes), opts)
            .await
            .with_context(|| format!("storing {key}"))?;
        Ok(())
    }

    /// `None` if the object does not exist.
    pub async fn get(&self, key: &str) -> Result<Option<ObjectBody>> {
        match self.store.get(&path(key)?).await {
            Ok(result) => {
                let info = ObjectInfo {
                    size: result.meta.size,
                    content_type: content_type_of(&result.attributes),
                };
                Ok(Some(ObjectBody {
                    info,
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

fn content_type_of(attributes: &Attributes) -> Option<String> {
    attributes.get(&Attribute::ContentType).map(|v| v.to_string())
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
    fn rejects_odd_keys() {
        assert!(path("a/../b").is_err());
        assert!(path("a//b").is_err());
    }
}
