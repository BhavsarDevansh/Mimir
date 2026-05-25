//! First-run initialisation handler.
//!
//! Creates the Mimir directory structure and default files, then prints
//! a friendly welcome message guiding the user to the next step.

use is_terminal::IsTerminal;
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

    #[cfg(target_os = "linux")]
    {
        if std::io::stdin().is_terminal() {
            println!();
            print!("Install systemd user service for auto-start? [y/N]: ");
            if let Err(e) = std::io::Write::flush(&mut std::io::stdout()) {
                eprintln!("Warning: failed to flush stdout: {e}");
            }

            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) => {
                    // EOF — treat as "no"
                    print_systemd_manual();
                }
                Ok(_) => {
                    let answer = line.trim().to_lowercase();
                    if answer == "y" || answer == "yes" {
                        install_systemd_service().await;
                    } else {
                        print_systemd_manual();
                    }
                }
                Err(e) => {
                    eprintln!("Warning: failed to read prompt response: {e}");
                    print_systemd_manual();
                }
            }
        } else {
            print_systemd_manual();
        }
    }

    #[cfg(target_os = "macos")]
    {
        println!();
        println!("Note: On macOS, use launchd for auto-start (planned for a future phase).");
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // Windows and other platforms: skip silently.
    }

    println!();
    println!("Next: set your API key in the config file or via MIMIR_LLM_API_KEY.");
    println!("Then run: mimir ask hello");
}

#[cfg(target_os = "linux")]
async fn install_systemd_service() {
    use mimir_core::systemd::SystemdRunner;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Warning: could not resolve current executable path: {e}");
            print_systemd_manual();
            return;
        }
    };

    let service_path = match mimir_core::systemd::generate_and_install_service_file(&exe) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Warning: failed to install systemd service file: {e}");
            print_systemd_manual();
            return;
        }
    };

    println!("Installed systemd service: {}", service_path.display());

    let runner = mimir_core::systemd::RealSystemdRunner;
    if let Err(e) = runner.daemon_reload().await {
        eprintln!("Warning: systemctl daemon-reload failed: {e}");
        print_systemd_manual();
        return;
    }

    if let Err(e) = runner.enable_now("mimir").await {
        eprintln!("Warning: systemctl enable --now mimir failed: {e}");
        print_systemd_manual();
        return;
    }

    println!("Enabled mimir user service.");
    println!("Run the following to keep it active when not logged in:");
    println!("  loginctl enable-linger $USER");
}

#[cfg(target_os = "linux")]
fn print_systemd_manual() {
    println!("To enable auto-start manually, run:");
    println!("  systemctl --user daemon-reload");
    println!("  systemctl --user enable --now mimir");
    println!("  loginctl enable-linger $USER");
}
