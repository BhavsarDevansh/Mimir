//! Memory viewer. Loads and prints the contents of `memory.md` to stdout.
use mimir_client::MimirClient;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

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
