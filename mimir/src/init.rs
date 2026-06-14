//! First-run initialisation handler.
//!
//! Creates the Mimir directory structure and default files, then prints
//! a friendly welcome message guiding the user to the next step.

use is_terminal::IsTerminal;
use mimir_core::config::{Config, IdentityConfig, InitResult};

fn exit_with_error(msg: impl std::fmt::Display) -> ! {
    eprintln!("Error: {}", msg);
    std::process::exit(1);
}

fn warn_on_err<T, E: std::fmt::Display>(result: Result<T, E>, message: &str) {
    if let Err(error) = result {
        eprintln!("Warning: {message}: {error}");
    }
}

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

            if std::io::stdin().is_terminal() {
                let name = inquire::Text::new("What is your full name?")
                    .with_placeholder(&whoami::realname())
                    .prompt()
                    .unwrap_or_else(|_| whoami::realname());
                let preferred = inquire::Text::new("How would you like to be addressed?")
                    .with_placeholder(&whoami::username())
                    .prompt()
                    .unwrap_or_else(|_| whoami::username());

                let mut cfg = Config::load(Some(&config_file)).unwrap_or_default();
                let resolved_name = {
                    let t = name.trim();
                    if t.is_empty() {
                        whoami::realname()
                    } else {
                        t.to_string()
                    }
                };
                let resolved_preferred = {
                    let t = preferred.trim();
                    if t.is_empty() {
                        whoami::username()
                    } else {
                        t.to_string()
                    }
                };
                cfg.identity = IdentityConfig {
                    name: resolved_name,
                    preferred_name: resolved_preferred,
                };
                if cfg.save(&config_file).is_ok() {
                    println!("Saved identity to config.");
                } else {
                    eprintln!("Warning: failed to save identity to config");
                }
            }
        }
        Ok(InitResult::AlreadyInitialized) => {
            println!("Mimir is already initialized.");
        }
        Err(e) => exit_with_error(format!("failed to initialise Mimir: {e}")),
    }

    #[cfg(target_os = "linux")]
    {
        if std::io::stdin().is_terminal() {
            println!();
            print!("Install systemd user service for auto-start? [y/N]: ");
            warn_on_err(
                std::io::Write::flush(&mut std::io::stdout()),
                "failed to flush stdout",
            );

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
fn resolve_executable_path(current: &std::path::Path) -> std::path::PathBuf {
    let path_str = current.to_string_lossy();
    let looks_like_cargo_build =
        path_str.contains("/target/debug/") || path_str.contains("/target/release/");

    if looks_like_cargo_build {
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let candidate = dir.join("mimir");
                if candidate.is_file() {
                    eprintln!(
                        "Warning: current executable appears to be a debug build.\n         Using {} for the systemd service instead.",
                        candidate.display()
                    );
                    return candidate;
                }
            }
        }
        eprintln!(
            "Warning: current executable appears to be a debug build ({})\n         and no 'mimir' was found in PATH. The generated service file may break if this binary is removed.",
            current.display()
        );
    }

    current.to_path_buf()
}

#[cfg(target_os = "linux")]
async fn install_systemd_service() {
    use mimir_core::systemd::SystemdRunner;

    let exe = match std::env::current_exe() {
        Ok(p) => resolve_executable_path(&p),
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
    if runner.daemon_reload().await.is_err() || runner.enable_now("mimir").await.is_err() {
        eprintln!("Warning: systemd activation failed; see manual instructions below.");
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
