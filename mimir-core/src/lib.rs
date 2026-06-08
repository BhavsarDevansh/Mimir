pub mod config;
pub mod context;
pub mod job_queue;
pub mod llm;
pub mod paths;
pub mod personality;
pub mod skills;
pub mod systemd;
pub mod tools;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
