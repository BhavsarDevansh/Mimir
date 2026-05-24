//! System status reporter. Displays config, LLM connectivity, and memory stats.
use crate::constants::DEFAULT_BASE_URL;
use mimir_client::MimirClient;

pub async fn handle_status() {
    let client = MimirClient::new(DEFAULT_BASE_URL);

    match client.status().await {
        Ok(status) => {
            println!(
                "Version: {}, Uptime: {}s",
                status.version, status.uptime_seconds
            );
            println!(
                "Queue: user={} system={}, Workers: {}",
                status.queue_depth_user, status.queue_depth_system, status.worker_threads
            );
            println!("Endpoint: {}", status.endpoint);
            println!("Model: {}", status.model);
            println!(
                "Config: {} ({})",
                status.config_path.as_deref().unwrap_or("unknown"),
                if status.config_exists {
                    "exists"
                } else {
                    "missing"
                }
            );
            println!(
                "LLM: reachable={}, context_window={:?}",
                status.llm_reachable, status.context_window
            );
            println!(
                "Memory: {} ({}), {} / {} chars ({:.1}%)",
                status.memory_path,
                if status.memory_exists {
                    "exists"
                } else {
                    "NOT FOUND"
                },
                status.memory_chars,
                status.memory_limit,
                status.memory_usage_pct
            );
        }
        Err(e) => {
            eprintln!("Status request failed: {}", e);
            std::process::exit(1);
        }
    }
}
