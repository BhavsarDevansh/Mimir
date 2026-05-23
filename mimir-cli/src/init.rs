//! First-run initialisation handler.
//!
//! Creates the Mimir directory structure and default files, then prints
//! a friendly welcome message guiding the user to the next step.

use mimir_core::config::{Config, InitResult};
use mimir_core::memory::MemoryLoader;

pub async fn handle_init() {
    match Config::init() {
        Ok(InitResult::Created {
            config_dir,
            data_dir,
            config_file,
        }) => {
            println!("Created config directory: {}", config_dir.display());
            println!("Created data directory:    {}", data_dir.display());
            println!("Created default config:    {}", config_file.display());
        }
        Ok(InitResult::AlreadyInitialized) => {
            println!("Mimir is already initialized.");
        }
        Err(e) => {
            eprintln!("Error: failed to initialise Mimir: {e}");
            std::process::exit(1);
        }
    }

    match MemoryLoader::init().await {
        Ok(true) => {
            let path = mimir_core::paths::memory_path().unwrap_or_default();
            println!("Created default memory:    {}", path.display());
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!("Warning: failed to create memory.md: {e}");
        }
    }

    println!();
    println!("Next: set your API key in the config file or via MIMIR_LLM_API_KEY.");
    println!("Then run: mimir ask hello");
}
