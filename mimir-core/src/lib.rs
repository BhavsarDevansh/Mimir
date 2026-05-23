pub mod config;
pub mod context;
pub mod llm;
pub mod memory;
pub mod paths;
pub mod personality;
pub mod skills;
pub mod tools;

pub fn version() -> &'static str {
    "0.9.0"
}
