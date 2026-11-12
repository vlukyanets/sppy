//! sppy client without UI: connection, media delivery, sync loop, player supervision.

pub mod api;
pub mod cache;
pub mod config;
pub mod connection;
pub mod download;
pub mod link;
pub mod local_index;
pub mod media;
pub mod netaddr;
pub mod notice;
mod session;
