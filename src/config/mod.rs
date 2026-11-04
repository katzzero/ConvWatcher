//! Configuration subsystem: loading, validation, first-run generation and
//! default codec presets.
//!
//! The implementation is split into focused submodules; this module is the
//! facade that exposes the entry points used by the rest of the crate.

pub mod codec_registry;
pub mod embedded;
pub mod generate;
pub mod global;
pub mod load;
pub mod presets;
pub mod validate;
pub mod watch;

pub use load::{config_dir_for, load_config};
pub use validate::validate_config;

pub(crate) use validate::validate_absolute_path;
