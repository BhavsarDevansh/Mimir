//! Memory viewer. Loads and prints the contents of `memory.md` to stdout.
use mimir_client::MimirClient;

pub async fn handle_memory(base_url: &str) {
    let client = MimirClient::new(base_url);
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
