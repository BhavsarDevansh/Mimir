//! System status reporter. Displays config, LLM connectivity, and memory stats.
use mimir_core::config::Config;
use mimir_core::llm::LlmClient;
use mimir_core::memory::MemoryLoader;

pub async fn handle_status() {
    let config = match Config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    let config_path = Config::config_path();

    println!(
        "Config path: {}",
        config_path
            .as_deref()
            .unwrap_or(std::path::Path::new("(unknown)"))
            .display()
    );
    println!(
        "Config file: {}",
        if config_path.as_ref().is_some_and(|p| p.exists()) {
            "exists"
        } else {
            "NOT FOUND"
        }
    );

    println!("LLM endpoint: {}", config.llm.endpoint);
    println!("LLM model: {}", config.llm.model);

    let client = LlmClient::new(config.llm.clone()).await;
    match client.fetch_model_context_window().await {
        Ok(Some(window)) => {
            println!(
                "LLM connectivity: reachable (context window: {} tokens)",
                window
            );
        }
        Ok(None) => {
            println!("LLM connectivity: reachable");
        }
        Err(e) => {
            eprintln!("LLM connectivity: unreachable ({})", e);
        }
    }

    let mem_path = MemoryLoader::get_memory_path();
    let mem_exists = mem_path.exists();
    let mem_chars = if mem_exists {
        match MemoryLoader::load(&mem_path).await {
            Ok(content) => content.chars().count(),
            Err(_) => 0,
        }
    } else {
        0
    };
    let mem_limit = config.memory.char_limit as usize;
    let mem_usage_pct = if mem_limit > 0 {
        (mem_chars as f64 / mem_limit as f64) * 100.0
    } else {
        0.0
    };

    println!(
        "Memory path: {} ({})",
        mem_path.display(),
        if mem_exists { "exists" } else { "NOT FOUND" }
    );
    println!(
        "Memory usage: {} / {} chars ({:.1}%)",
        mem_chars, mem_limit, mem_usage_pct
    );
}
