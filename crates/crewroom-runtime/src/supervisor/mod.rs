//! Supervised native execution. Workflow admission is composed above these
//! process and lifecycle boundaries; queued work does not establish execution.
pub mod client;
mod hosts;
pub mod journal;
pub mod tasks;
pub mod workflow;
pub use hosts::{HostManager, HostOptions};
