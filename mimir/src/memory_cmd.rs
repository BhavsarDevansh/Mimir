//! Memory viewer. Loads and prints the live condensed memory block to stdout.

pub async fn handle_memory(transport: &crate::transport::DaemonTransport, refresh: bool) {
    let client = crate::cli_util::make_client(transport);
    if refresh {
        match client.memory_refresh().await {
            Ok(resp) => {
                println!("Run ID: {}, Status: {}", resp.run_id, resp.status);
                if let Some(err) = resp.error.as_deref() {
                    eprintln!("Memory condensation reported an error: {}", err);
                    std::process::exit(1);
                } else {
                    println!("Memory condensation triggered.");
                }
            }
            Err(e) => {
                eprintln!("Failed to trigger memory refresh: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
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
