//! ConvWatcher library crate.
//!
//! Exposes the internal modules so both the standalone `convwatcher` binary and
//! the `convwatcher-server` (coordinator) binary can share the same config,
//! health, processing and watcher code without duplication.

pub mod cli;
pub mod config;
pub mod health;
pub mod logs;
pub mod processor;
pub mod utils;
pub mod watcher;
pub mod worker;
