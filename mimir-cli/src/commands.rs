use crate::cli::ToolCommands;
use mimir_core::tools::{ToolPermission, ToolRegistry, ToolSource, ToolsConfig};

pub async fn handle_tool_command(command: ToolCommands) {
    let registry = ToolRegistry::with_builtins();

    // Load tools.toml if it exists; abort on any parse/load error.
    if let Some(path) = ToolsConfig::default_path()
        && path.exists()
        && let Err(e) = registry.load_tools_config(&path)
    {
        eprintln!("Error: failed to load tools config: {e}");
        std::process::exit(1);
    }

    match command {
        ToolCommands::List => {
            let tools = registry.list();
            if tools.is_empty() {
                println!("No tools registered.");
                return;
            }
            println!("{:<20} {:<10} {:<12}", "Name", "Source", "Permission");
            println!("{}", "-".repeat(44));
            for meta in tools {
                let source = match meta.source {
                    ToolSource::Native => "native",
                    ToolSource::Cli => "cli",
                };
                println!(
                    "{:<20} {:<10} {:<12}",
                    meta.name,
                    source,
                    meta.permission.as_str(),
                );
            }
        }
        ToolCommands::Enable { name } => {
            if let Err(e) = registry.set_permission(&name, ToolPermission::Auto) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            if let Err(e) = persist_registry(&registry) {
                eprintln!("Error saving tools config: {e}");
                std::process::exit(1);
            }
            println!("Tool '{name}' enabled (permission: auto).");
        }
        ToolCommands::Disable { name } => {
            if let Err(e) = registry.set_permission(&name, ToolPermission::Disabled) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            if let Err(e) = persist_registry(&registry) {
                eprintln!("Error saving tools config: {e}");
                std::process::exit(1);
            }
            println!("Tool '{name}' disabled.");
        }
        ToolCommands::Permission { name, level } => {
            let permission = match level.parse::<ToolPermission>() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error: invalid permission level: {e}");
                    std::process::exit(1);
                }
            };
            if let Err(e) = registry.set_permission(&name, permission) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            if let Err(e) = persist_registry(&registry) {
                eprintln!("Error saving tools config: {e}");
                std::process::exit(1);
            }
            println!("Tool '{name}' permission set to {}.", permission.as_str());
        }
    }
}

fn persist_registry(registry: &ToolRegistry) -> Result<(), mimir_core::tools::ToolError> {
    let path = ToolsConfig::default_path().ok_or_else(|| {
        mimir_core::tools::ToolError::execution_failed(
            "tools_config",
            "could not determine config directory",
        )
    })?;
    registry.save_tools_config(&path)
}
