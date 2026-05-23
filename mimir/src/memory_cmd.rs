//! Memory viewer. Loads and prints the contents of `memory.md` to stdout.
use mimir_core::memory::MemoryLoader;

pub async fn handle_memory() {
    let path = MemoryLoader::get_memory_path();
    match MemoryLoader::load(&path).await {
        Ok(content) => {
            println!("{}", content);
        }
        Err(e) => {
            eprintln!("Failed to load memory: {}", e);
            std::process::exit(1);
        }
    }
}
