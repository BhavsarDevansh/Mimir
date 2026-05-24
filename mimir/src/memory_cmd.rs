//! Memory viewer. Loads and prints the contents of `memory.md` to stdout.
use crate::constants::DEFAULT_BASE_URL;
use mimir_client::MimirClient;

pub async fn handle_memory() {
    let client = MimirClient::new(DEFAULT_BASE_URL);
    match client.memory().await {
        Ok(content) => {
            println!("{}", content);
        }
        Err(e) => {
            eprintln!("Failed to load memory: {}", e);
            std::process::exit(1);
        }
    }
}
