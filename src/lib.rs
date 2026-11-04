//! ConvWatcher library crate.
//!
//! Exposes the internal modules so the binary and integration tests can share
//! the same config, health, processing and watcher code.

pub mod cli;
pub mod config;
pub mod daemon;
pub mod health;
pub mod logs;
pub mod processor;
pub mod utils;
pub mod watcher;
