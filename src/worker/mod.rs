//! Coordinator-side worker system: discovers agents, holds locked connections,
//! and dispatches video/audio jobs to the least-loaded agent.

pub mod coordinator;
pub mod dispatch;

pub use coordinator::WorkerPool;
