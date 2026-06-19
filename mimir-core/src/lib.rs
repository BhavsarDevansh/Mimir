#![deny(unsafe_code)]
pub mod agents;
pub mod config;
pub mod context;
pub mod conversation;
pub mod fts5;
pub mod job_queue;
pub mod llm;
pub mod paths;
pub mod personality;
pub mod scheduler;
pub mod skills;
pub mod systemd;
pub mod tools;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
