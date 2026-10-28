//! Shared library for the EloraNotes app and its collaboration server.
//!
//! The server is installed on machines with no display, so everything that needs egui is behind
//! the `gui` feature: `cargo build --no-default-features --bin elora-server` builds without it. What
//! lets AI apps use the notes is behind `assistants`, which `gui` turns on.

#[cfg(feature = "gui")]
pub mod app;
pub mod collab;
pub mod format;
pub mod graph_layout;
#[cfg(any(feature = "gui", feature = "icons"))]
pub mod icon_names;
#[cfg(feature = "gui")]
pub mod icons;
#[cfg(feature = "gui")]
pub mod images;
pub mod live;
#[cfg(feature = "gui")]
pub mod markdown;
#[cfg(feature = "assistants")]
pub mod mcp;
#[cfg(feature = "assistants")]
pub mod mcp_clients;
pub mod mcp_shared;
pub mod net;
pub mod people;
pub mod server;
#[cfg(feature = "assistants")]
pub mod skills;
pub mod storage;
#[cfg(feature = "gui")]
pub mod theme;
pub mod tls;
#[cfg(feature = "gui")]
pub mod update;
