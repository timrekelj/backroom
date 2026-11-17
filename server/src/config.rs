use std::env;
use std::net::SocketAddr;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub scylla_nodes: Vec<String>,
    pub keyspace: String,
    pub worker_id: u16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let listen_addr = var("BACKROOM_LISTEN_ADDR", "0.0.0.0:8080")
            .parse()
            .context("invalid BACKROOM_LISTEN_ADDR")?;
        let scylla_nodes = var("BACKROOM_SCYLLA_NODES", "127.0.0.1:9042")
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let keyspace = var("BACKROOM_KEYSPACE", "backroom");
        let worker_id = var("BACKROOM_WORKER_ID", "0")
            .parse()
            .context("invalid BACKROOM_WORKER_ID")?;
        Ok(Self {
            listen_addr,
            scylla_nodes,
            keyspace,
            worker_id,
        })
    }
}

fn var(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}
