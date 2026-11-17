#![allow(refining_impl_trait_internal, refining_impl_trait_reachable)]

pub mod app;
pub mod config;
pub mod migrations;
pub mod services;
pub mod snowflake;

pub mod proto {
    connectrpc::include_generated!();
}

pub use app::{AppState, build_app};
