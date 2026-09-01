#![deny(unsafe_code)]
pub mod agents;
pub mod auth;
pub mod config;
pub mod context;
pub mod conversation;
pub mod frontmatter;
pub mod fts5;
pub mod geocoder;
pub mod hooks;
pub mod job_queue;
pub mod llm;
pub mod paths;
pub mod personality;
pub mod scheduler;
pub mod skills;
mod sqlite;
pub mod systemd;
pub mod tools;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
