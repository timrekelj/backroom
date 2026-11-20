use std::env;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub scylla_nodes: Vec<String>,
    pub keyspace: String,
    pub worker_id: u16,
    pub heartbeat_interval: Duration,
    pub s3: S3Config,
    /// Largest attachment upload in bytes (`THEHIDEOUT_MAX_UPLOAD_BYTES`).
    pub max_upload_bytes: u64,
}

/// Connection settings of the S3-compatible object store holding media.
/// Defaults match the SeaweedFS service in `docker-compose.yml`.
#[derive(Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    /// Path-style addressing (`endpoint/bucket/key`); `false` for AWS S3 and
    /// other providers that use virtual-hosted style.
    pub path_style: bool,
    /// Reserved for a CDN or public bucket URL in front of `/media`; currently
    /// only logged.
    pub public_base: Option<String>,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key", &self.access_key)
            .field("secret_key", &"<redacted>")
            .field("path_style", &self.path_style)
            .field("public_base", &self.public_base)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let listen_addr = var("THEHIDEOUT_LISTEN_ADDR", "0.0.0.0:8080")
            .parse()
            .context("invalid THEHIDEOUT_LISTEN_ADDR")?;
        let scylla_nodes = var("THEHIDEOUT_SCYLLA_NODES", "127.0.0.1:9042")
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let keyspace = var("THEHIDEOUT_KEYSPACE", "thehideout");
        let worker_id = var("THEHIDEOUT_WORKER_ID", "0")
            .parse()
            .context("invalid THEHIDEOUT_WORKER_ID")?;
        let heartbeat_secs: u64 = var("THEHIDEOUT_HEARTBEAT_SECS", "30")
            .parse()
            .context("invalid THEHIDEOUT_HEARTBEAT_SECS")?;
        if heartbeat_secs == 0 {
            bail!("THEHIDEOUT_HEARTBEAT_SECS must be at least 1");
        }
        let max_upload_bytes: u64 = var("THEHIDEOUT_MAX_UPLOAD_BYTES", "52428800")
            .parse()
            .context("invalid THEHIDEOUT_MAX_UPLOAD_BYTES")?;
        if max_upload_bytes == 0 {
            bail!("THEHIDEOUT_MAX_UPLOAD_BYTES must be at least 1");
        }
        Ok(Self {
            listen_addr,
            scylla_nodes,
            keyspace,
            worker_id,
            heartbeat_interval: Duration::from_secs(heartbeat_secs),
            s3: S3Config::from_env()?,
            max_upload_bytes,
        })
    }
}

impl S3Config {
    pub fn from_env() -> Result<Self> {
        let path_style = match var("THEHIDEOUT_S3_PATH_STYLE", "true").to_lowercase().as_str() {
            "true" | "1" | "yes" => true,
            "false" | "0" | "no" => false,
            other => bail!("invalid THEHIDEOUT_S3_PATH_STYLE {other:?}, expected true or false"),
        };
        Ok(Self {
            endpoint: var("THEHIDEOUT_S3_ENDPOINT", "http://127.0.0.1:8333"),
            region: var("THEHIDEOUT_S3_REGION", "us-east-1"),
            bucket: var("THEHIDEOUT_S3_BUCKET", "thehideout-media"),
            access_key: var("THEHIDEOUT_S3_ACCESS_KEY", "thehideout-dev"),
            secret_key: var("THEHIDEOUT_S3_SECRET_KEY", "thehideout-dev-secret"),
            path_style,
            public_base: env::var("THEHIDEOUT_S3_PUBLIC_BASE").ok().filter(|s| !s.is_empty()),
        })
    }
}

fn var(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}
