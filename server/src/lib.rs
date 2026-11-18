#![allow(refining_impl_trait_internal, refining_impl_trait_reachable)]

pub mod access;
pub mod app;
pub mod auth;
pub mod config;
pub mod convert;
pub mod events;
pub mod migrations;
pub mod ratelimit;
pub mod services;
pub mod snowflake;

pub mod proto {
    connectrpc::include_generated!();
}

pub use app::{AppState, build_app};
