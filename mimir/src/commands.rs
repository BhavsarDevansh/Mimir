use crate::cli::{SkillCommands, ToolCommands};
use mimir_core::paths;
use mimir_core::skills::{Skill, SkillRegistry, SkillSource, SkillsPermissionsConfig};
use mimir_core::tools::{ToolPermission, ToolRegistry, ToolSource, ToolsConfig};

pub fn exit_with_error(msg: impl std::fmt::Display) -> ! {
    eprintln!("Error: {}", msg);
    std::process::exit(1);
}

fn skills_dir() -> std::path::PathBuf {
    paths::skills_dir()
        .unwrap_or_else(|e| exit_with_error(format!("failed to resolve skills directory: {e}")))
}

fn set_tool_permission_or_exit(registry: &ToolRegistry, name: &str, permission: ToolPermission) {
    if let Err(e) = registry.set_permission(name, permission) {
        exit_with_error(e);
    }
}

fn persist_tools_or_exit(registry: &ToolRegistry) {
    if let Err(e) = persist_tools(registry) {
        exit_with_error(format!("Error saving tools config: {e}"));
    }
}

fn set_skill_permission_or_exit(registry: &SkillRegistry, name: &str, permission: ToolPermission) {
    if let Err(e) = registry.set_permission(name, permission) {
        exit_with_error(e);
    }
}

fn persist_skills_permissions_or_exit(registry: &SkillRegistry) {
    if let Err(e) = persist_skills_permissions(registry) {
        exit_with_error(format!("Error saving skills permissions: {e}"));
    }
}

fn parse_skill_source(orig: &str) -> SkillSource {
    match orig.to_lowercase().as_str() {
        "builtin" => SkillSource::Builtin,
        "user" => SkillSource::User,
        "generated" => SkillSource::Generated,
        _ => exit_with_error(format!(
            "invalid origin '{orig}'. Use builtin, user, or generated."
        )),
    }
}

fn validate_skill_name(name: &str) {
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.is_empty() {
        exit_with_error(format!("invalid skill name: '{name}'"));
    }
}

pub async fn handle_tool_command(command: ToolCommands) {
    let registry = ToolRegistry::with_builtins();

    if let Some(path) = ToolsConfig::default_path()
        && path.exists()
    {
        if let Err(e) = registry.load_tools_config(&path) {
            exit_with_error(format!("failed to load tools config: {e}"));
        }
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
            set_tool_permission_or_exit(&registry, &name, ToolPermission::Auto);
            persist_tools_or_exit(&registry);
            println!("Tool '{name}' enabled (permission: auto).");
        }
        ToolCommands::Disable { name } => {
            set_tool_permission_or_exit(&registry, &name, ToolPermission::Disabled);
            persist_tools_or_exit(&registry);
            println!("Tool '{name}' disabled.");
        }
        ToolCommands::Permission { name, level } => {
            let permission = match level.parse::<ToolPermission>() {
                Ok(p) => p,
                Err(e) => exit_with_error(format!("invalid permission level: {e}")),
            };
            set_tool_permission_or_exit(&registry, &name, permission);
            persist_tools_or_exit(&registry);
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
                    let source = parse_skill_source(&orig);
                    registry
                        .list_by_source(source)
                        .into_iter()
                        .filter(|m| m.tags.iter().any(|tag| tag.eq_ignore_ascii_case(&t)))
                        .collect()
                }
                (Some(orig), None) => {
                    let source = parse_skill_source(&orig);
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
                exit_with_error(format!("skill '{name}' not found."));
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
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            validate_skill_name(&file_name);
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                exit_with_error("skill file must have a .md extension");
            }
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| exit_with_error(format!("failed to read skill file: {e}")));
            let def = mimir_core::skills::markdown::parse_skill_file(&contents)
                .unwrap_or_else(|e| exit_with_error(format!("failed to parse skill file: {e}")));
            let skill = mimir_core::skills::markdown::MarkdownSkill::from_definition(def)
                .unwrap_or_else(|e| exit_with_error(format!("failed to build skill: {e}")));
            let name = skill.name().to_string();
            let metadata = mimir_core::skills::markdown::build_metadata(
                &skill,
                &skill.version,
                &skill.tags,
                Some(&path),
            );
            registry
                .register_with_metadata(std::sync::Arc::new(skill), metadata)
                .unwrap_or_else(|e| exit_with_error(e));

            let sdir = skills_dir();
            std::fs::create_dir_all(&sdir).unwrap_or_else(|e| {
                exit_with_error(format!("failed to create skills directory: {e}"))
            });
            let dest = sdir.join(format!("{name}.md"));
            // Defensive: ensure the computed destination does not escape the skills directory.
            if !dest.starts_with(&sdir) {
                exit_with_error("skill name would escape the skills directory");
            }
            std::fs::copy(&path, &dest).unwrap_or_else(|e| {
                exit_with_error(format!("failed to copy skill to config dir: {e}"))
            });

            println!("Skill '{name}' added successfully.");
        }
        SkillCommands::Delete { name } => {
            validate_skill_name(&name);
            let meta = registry.metadata(&name);
            let path = meta
                .as_ref()
                .and_then(|m| m.source_path.clone())
                .unwrap_or_else(|| skills_dir().join(format!("{name}.md")));
            // Remove the file first; if it fails, keep the registry entry
            // so the user can retry.
            if path.exists() {
                std::fs::remove_file(&path).unwrap_or_else(|e| {
                    exit_with_error(format!("failed to remove skill file: {e}"))
                });
            }
            registry
                .delete(&name)
                .unwrap_or_else(|e| exit_with_error(e));
            println!("Skill '{name}' deleted.");
        }
        SkillCommands::Enable { name } => {
            validate_skill_name(&name);
            set_skill_permission_or_exit(&registry, &name, ToolPermission::Auto);
            persist_skills_permissions_or_exit(&registry);
            println!("Skill '{name}' enabled (permission: auto).");
        }
        SkillCommands::Disable { name } => {
            validate_skill_name(&name);
            set_skill_permission_or_exit(&registry, &name, ToolPermission::Disabled);
            persist_skills_permissions_or_exit(&registry);
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
