//! Server launcher. Spawns `mimir-server` as a detached background process.
use std::path::PathBuf;

pub fn handle_start() {
    let exe_name = "mimir-server";

    // Check adjacent to current executable first.
    let exe_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let exe_dir = exe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    let candidate = exe_dir.join(exe_name);

    let binary = if candidate.exists() {
        candidate
    } else {
        // Fallback: try PATH.
        match which::which(exe_name) {
            Ok(p) => p,
            Err(_) => {
                eprintln!(
                    "Could not find '{}' binary. Make sure it is on PATH or adjacent to the mimir binary ({:?}).",
                    exe_name, exe_dir
                );
                std::process::exit(1);
            }
        }
    };

    match std::process::Command::new(&binary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            let pid = child.id();
            println!("Started mimir-server (PID: {}).", pid);
        }
        Err(e) => {
            eprintln!("Failed to start mimir-server: {}", e);
            std::process::exit(1);
        }
    }
}
