//! Shared library for the EloraNotes app and its collaboration server.
//!
//! The server is installed on machines with no display, so everything that needs egui is behind
//! the `gui` feature: `cargo build --no-default-features --bin elora-server` builds without it.

#[cfg(feature = "gui")]
pub mod app;
pub mod collab;
#[cfg(feature = "gui")]
pub mod icon_names;
#[cfg(feature = "gui")]
pub mod icons;
#[cfg(feature = "gui")]
pub mod markdown;
pub mod net;
pub mod server;
pub mod storage;
#[cfg(feature = "gui")]
pub mod theme;
pub mod tls;
#[cfg(feature = "gui")]
pub mod update;
