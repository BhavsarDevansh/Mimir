pub mod config;
pub mod context;
pub mod llm;
pub mod memory;
pub mod personality;
pub mod tools;

pub fn version() -> &'static str {
    "0.5.0"
}
