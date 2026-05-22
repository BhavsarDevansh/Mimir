use crate::cli::{SkillCommands, ToolCommands};
use crate::skills_permissions_config::SkillsPermissionsConfig;
use mimir_core::skills::{Skill, SkillRegistry, SkillSource};
use mimir_core::tools::{ToolPermission, ToolRegistry, ToolSource, ToolsConfig};
use std::path::PathBuf;

/// Return the user skills directory (`~/.config/mimir/skills/`).
fn skills_dir() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join("mimir").join("skills"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub async fn handle_tool_command(command: ToolCommands) {
    let registry = ToolRegistry::with_builtins();

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
            if let Err(e) = persist_tools(&registry) {
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
            if let Err(e) = persist_tools(&registry) {
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
            if let Err(e) = persist_tools(&registry) {
                eprintln!("Error saving tools config: {e}");
                std::process::exit(1);
            }
            println!("Tool '{name}' permission set to {}.", permission.as_str());
        }
    }
}

pub async fn handle_skill_command(command: SkillCommands) {
    let registry = SkillRegistry::with_builtins();

    let sdir = skills_dir();
    if let Err(e) = registry.load_user_skills(&sdir) {
        eprintln!("Warning: failed to load user skills: {e}");
    }

    // Load persisted permission overrides.
    if let Some(path) = SkillsPermissionsConfig::default_path()
        && path.exists()
    {
        match SkillsPermissionsConfig::load(&path) {
            Ok(config) => {
                for (name, permission) in config.permissions {
                    let _ = registry.set_permission(&name, permission);
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to load skills permissions: {e}");
            }
        }
    }

    match command {
        SkillCommands::List { origin, tag } => {
            let skills = match (origin, tag) {
                (Some(orig), Some(t)) => {
                    let source = match orig.to_lowercase().as_str() {
                        "builtin" => SkillSource::Builtin,
                        "user" => SkillSource::User,
                        "generated" => SkillSource::Generated,
                        _ => {
                            eprintln!(
                                "Error: invalid origin '{orig}'. Use builtin, user, or generated."
                            );
                            std::process::exit(1);
                        }
                    };
                    registry
                        .list_by_source(source)
                        .into_iter()
                        .filter(|m| m.tags.iter().any(|tag| tag.eq_ignore_ascii_case(&t)))
                        .collect()
                }
                (Some(orig), None) => {
                    let source = match orig.to_lowercase().as_str() {
                        "builtin" => SkillSource::Builtin,
                        "user" => SkillSource::User,
                        "generated" => SkillSource::Generated,
                        _ => {
                            eprintln!(
                                "Error: invalid origin '{orig}'. Use builtin, user, or generated."
                            );
                            std::process::exit(1);
                        }
                    };
                    registry.list_by_source(source)
                }
                (None, Some(t)) => registry.list_by_tag(&t),
                (None, None) => registry.list(),
            };

            if skills.is_empty() {
                println!("No skills registered.");
                return;
            }
            println!(
                "{:<25} {:<10} {:<12} {:<10}",
                "Name", "Source", "Permission", "Version"
            );
            println!("{}", "-".repeat(60));
            for meta in skills {
                let source = match meta.source {
                    SkillSource::Builtin => "builtin",
                    SkillSource::User => "user",
                    SkillSource::Generated => "generated",
                };
                println!(
                    "{:<25} {:<10} {:<12} {:<10}",
                    meta.name,
                    source,
                    meta.permission.as_str(),
                    meta.version,
                );
            }
        }
        SkillCommands::Show { name } => {
            let meta = registry.metadata(&name);
            let Some(meta) = meta else {
                eprintln!("Error: skill '{name}' not found.");
                std::process::exit(1);
            };
            println!("Name:        {}", meta.name);
            println!("Description: {}", meta.description);
            println!("Source:      {:?}", meta.source);
            println!("Permission:  {}", meta.permission.as_str());
            println!("Version:     {}", meta.version);
            if !meta.tags.is_empty() {
                println!("Tags:        {}", meta.tags.join(", "));
            }
        }
        SkillCommands::Add { path } => {
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                eprintln!("Error: skill file must have a .md extension");
                std::process::exit(1);
            }
            let contents = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: failed to read skill file: {e}");
                    std::process::exit(1);
                }
            };
            let def = match mimir_core::skills::markdown::parse_skill_file(&contents) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to parse skill file: {e}");
                    std::process::exit(1);
                }
            };
            let skill = match mimir_core::skills::markdown::MarkdownSkill::from_definition(def) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: failed to build skill: {e}");
                    std::process::exit(1);
                }
            };
            let name = skill.name().to_string();
            let metadata = mimir_core::skills::markdown::build_metadata(
                &skill,
                &skill.version,
                &skill.tags,
                Some(&path),
            );
            if let Err(e) = registry.register_with_metadata(std::sync::Arc::new(skill), metadata) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }

            let sdir = skills_dir();
            if let Err(e) = std::fs::create_dir_all(&sdir) {
                eprintln!("Error: failed to create skills directory: {e}");
                std::process::exit(1);
            }
            let dest = sdir.join(format!("{name}.md"));
            // Defensive: ensure the computed destination does not escape the skills directory.
            if !dest.starts_with(&sdir) {
                eprintln!("Error: skill name would escape the skills directory");
                std::process::exit(1);
            }
            if let Err(e) = std::fs::copy(&path, &dest) {
                eprintln!("Error: failed to copy skill to config dir: {e}");
                std::process::exit(1);
            }

            println!("Skill '{name}' added successfully.");
        }
        SkillCommands::Delete { name } => {
            let meta = registry.metadata(&name);
            let path = meta
                .as_ref()
                .and_then(|m| m.source_path.clone())
                .unwrap_or_else(|| skills_dir().join(format!("{name}.md")));
            // Remove the file first; if it fails, keep the registry entry
            // so the user can retry.
            if path.exists()
                && let Err(e) = std::fs::remove_file(&path)
            {
                eprintln!("Error: failed to remove skill file: {e}");
                std::process::exit(1);
            }
            if let Err(e) = registry.delete(&name) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            println!("Skill '{name}' deleted.");
        }
        SkillCommands::Enable { name } => {
            if let Err(e) = registry.set_permission(&name, ToolPermission::Auto) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            if let Err(e) = persist_skills_permissions(&registry) {
                eprintln!("Error saving skills permissions: {e}");
                std::process::exit(1);
            }
            println!("Skill '{name}' enabled (permission: auto).");
        }
        SkillCommands::Disable { name } => {
            if let Err(e) = registry.set_permission(&name, ToolPermission::Disabled) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            if let Err(e) = persist_skills_permissions(&registry) {
                eprintln!("Error saving skills permissions: {e}");
                std::process::exit(1);
            }
            println!("Skill '{name}' disabled.");
        }
    }
}

fn persist_tools(registry: &ToolRegistry) -> Result<(), mimir_core::tools::ToolError> {
    let path = ToolsConfig::default_path().ok_or_else(|| {
        mimir_core::tools::ToolError::execution_failed(
            "tools_config",
            "could not determine config directory",
        )
    })?;
    registry.save_tools_config(&path)
}

fn persist_skills_permissions(
    registry: &SkillRegistry,
) -> Result<(), mimir_core::skills::SkillError> {
    let path = SkillsPermissionsConfig::default_path().ok_or_else(|| {
        mimir_core::skills::SkillError::execution_failed(
            "skills_permissions_config",
            "could not determine config directory",
        )
    })?;
    let mut permissions = std::collections::HashMap::new();
    for meta in registry.list() {
        if meta.source != SkillSource::Builtin && meta.permission != ToolPermission::Auto {
            permissions.insert(meta.name, meta.permission);
        }
    }
    let config = SkillsPermissionsConfig { permissions };
    config.save(&path)
}
