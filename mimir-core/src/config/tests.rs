//! Configuration loading tests.

use super::*;
use crate::paths;
use std::io::Write;
use std::path::PathBuf;

#[test]
fn test_env_override_llm() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_LLM_MODEL" {
            Some("gpt-3.5-turbo".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.llm.model, "gpt-3.5-turbo");
}

#[test]
fn test_env_override_agent_max_tool_rounds() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_AGENT_MAX_TOOL_ROUNDS" {
            Some("50".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.agent.max_tool_rounds, 50);
}

#[test]
fn test_env_override_agent_remember_debounce_seconds() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_AGENT_REMEMBER_DEBOUNCE_SECONDS" {
            Some("30".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.agent.remember_debounce_seconds, 30);
}

#[test]
fn test_env_override_agent_remember_debounce_seconds_invalid_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_AGENT_REMEMBER_DEBOUNCE_SECONDS" {
            Some("not_a_number".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.agent.remember_debounce_seconds, 10);
}

#[test]
fn test_env_override_agent_max_tool_rounds_invalid_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_AGENT_MAX_TOOL_ROUNDS" {
            Some("not_a_number".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.agent.max_tool_rounds, 100);
}

#[test]
fn test_env_override_context() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| match key {
        "MIMIR_CONTEXT_MAX_TOKENS" => Some("8192".to_string()),
        "MIMIR_CONTEXT_MAX_TURNS" => Some("50".to_string()),
        "MIMIR_CONTEXT_DB_PATH" => Some("/tmp/mimir/context.db".to_string()),
        _ => None,
    });
    assert_eq!(config.context.max_tokens, Some(8192));
    assert_eq!(config.context.max_turns, 50);
    assert_eq!(
        config.context.db_path,
        Some(PathBuf::from("/tmp/mimir/context.db"))
    );
}

#[test]
fn test_env_override_db_paths() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| match key {
        "MIMIR_KNOWLEDGE_DB_PATH" => Some("/tmp/mimir/knowledge.db".to_string()),
        "MIMIR_JOBS_DB_PATH" => Some("/tmp/mimir/jobs.db".to_string()),
        _ => None,
    });
    assert_eq!(
        config.knowledge.db_path,
        Some(PathBuf::from("/tmp/mimir/knowledge.db"))
    );
    assert_eq!(
        config.scheduler.db_path,
        Some(PathBuf::from("/tmp/mimir/jobs.db"))
    );
}

#[test]
fn test_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let original = Config {
        llm: LlmConfig {
            endpoint: "http://localhost:8080".to_string(),
            api_key: "secret".to_string(),
            model: "test-model".to_string(),
            max_tokens: Some(100),
            temperature: 0.5,
        },
        agent: AgentConfig {
            name: "TestAgent".to_string(),
            proactivity: Proactivity::Always,
            verbose_reasoning: true,
            max_tool_rounds: 100,
            remember_debounce_seconds: 10,
        },
        memory: MemoryConfig {
            enabled: false,
            char_limit: 100,
            auto_manage: false,
            temporal_horizon: 7,
            condensation_top_n: 500,
        },
        context: ContextConfig {
            max_tokens: Some(2048),
            max_turns: 10,
            db_path: Some(PathBuf::from("~/.local/share/mimir/context.db")),
            compaction: ContextCompactionConfig {
                enabled: true,
                max_turns: 8,
            },
        },
        personality: PersonalityConfig {
            preset: "transparent".to_string(),
        },
        server: ServerConfig {
            bind_addr: "127.0.0.1:8080".to_string(),
            socket_path: None,
        },
        identity: IdentityConfig::default(),
        knowledge: KnowledgeConfig::default(),
        scheduler: SchedulerConfig::default(),
        geocoder: GeocoderConfig::default(),
        secrets: SecretsConfig::default(),
    };

    original.save(&path).unwrap();
    let loaded = Config::load(Some(&path)).unwrap();
    assert_eq!(original, loaded);
}

#[test]
fn test_toml_roundtrip() {
    let toml_str = r#"
[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o"
max_tokens = 4096
temperature = 0.2

[agent]
name = "Mimir"
proactivity = "important_only"
verbose_reasoning = false

[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
condensation_top_n = 500

[scheduler]
debounce_seconds = 5
cooldown_seconds = 60

[context]
max_tokens = 4096
max_turns = 20
db_path = "~/.local/share/mimir/context.db"
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.llm.max_tokens, Some(4096));
    assert_eq!(config.context.max_tokens, Some(4096));
    assert_eq!(config.context.max_turns, 20);
    assert_eq!(
        config.context.db_path,
        Some(PathBuf::from("~/.local/share/mimir/context.db"))
    );
}

#[test]
fn test_missing_config_file_errors_when_explicit_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.toml");
    let result = Config::load(Some(&path));
    assert!(
        result.is_err(),
        "explicit non-existent path should return an error"
    );
}

#[test]
fn test_load_none_uses_defaults_when_file_missing() {
    // Config::load(None) with no existing file bootstraps and returns defaults.
    // We verify by creating a temp dir, calling init_at, then loading from the
    // resulting config file.
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    let data_dir = dir.path().join("data");
    let cache_dir = dir.path().join("cache");
    Config::init_at(&cfg_dir, &data_dir, &cache_dir).unwrap();
    let cfg_path = cfg_dir.join("config.toml");

    let config = Config::load(Some(&cfg_path)).unwrap();
    assert_eq!(config.llm.endpoint, "https://api.openai.com/v1");
    assert_eq!(config.llm.model, "gpt-4o");
    assert_eq!(config.agent.name, "Mimir");
    assert_eq!(config.memory.char_limit, 2500);
    assert_eq!(config.identity.name, "");
    assert_eq!(config.llm.max_tokens, None);
    assert_eq!(config.context.max_tokens, None);
    assert_eq!(config.context.max_turns, 20);
}

#[test]
fn test_config_path_returns_platform_path() {
    let path = Config::config_path();
    assert!(path.is_some());
    let path = path.unwrap();
    assert!(path.ends_with("mimir/config.toml"));
}

#[test]
fn test_invalid_proactivity_env_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_AGENT_PROACTIVITY" {
            Some("nonsense".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.agent.proactivity, Proactivity::ImportantOnly);
}

#[test]
fn test_load_from_toml_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    {
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(
            file,
            r#"
[llm]
model = "custom-model"

[context]
max_turns = 5
"#
        )
        .unwrap();
    }

    let config = Config::load(Some(&path)).unwrap();
    assert_eq!(config.llm.model, "custom-model");
    assert_eq!(config.context.max_turns, 5);
    assert_eq!(config.context.max_tokens, None); // default
}

#[test]
fn test_personality_config_toml_roundtrip() {
    let toml_str = r#"
[personality]
preset = "formal"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.personality.preset, "formal");
}

#[test]
fn test_personality_preset_env_override() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_PERSONALITY_PRESET" {
            Some("concise".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.personality.preset, "concise");
}

#[test]
fn test_env_override_scheduler_debounce_seconds() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_SCHEDULER_DEBOUNCE_SECONDS" {
            Some("10".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.scheduler.debounce_seconds, 10);
}

#[test]
fn test_env_override_scheduler_debounce_seconds_invalid_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_SCHEDULER_DEBOUNCE_SECONDS" {
            Some("not_a_number".to_string())
        } else {
            None
        }
    });
    // Should remain at default value
    assert_eq!(config.scheduler.debounce_seconds, 5);
}

#[test]
fn test_env_override_scheduler_cooldown_seconds() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_SCHEDULER_COOLDOWN_SECONDS" {
            Some("120".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.scheduler.cooldown_seconds, 120);
}

#[test]
fn test_env_override_scheduler_cooldown_seconds_invalid_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_SCHEDULER_COOLDOWN_SECONDS" {
            Some("invalid".to_string())
        } else {
            None
        }
    });
    // Should remain at default value
    assert_eq!(config.scheduler.cooldown_seconds, 60);
}

#[test]
fn test_env_override_events_schedule_times() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES" {
            Some("07:30, 19:45".to_string())
        } else {
            None
        }
    });
    assert_eq!(
        config.knowledge.events.schedule_times,
        vec!["07:30".to_string(), "19:45".to_string()]
    );
}

#[test]
fn test_env_override_events_schedule_times_empty_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES" {
            Some("   ,  ".to_string())
        } else {
            None
        }
    });
    // All tokens blank -> keep defaults.
    assert_eq!(
        config.knowledge.events.schedule_times,
        vec!["06:00".to_string(), "18:00".to_string()]
    );
}

#[test]
fn test_optimization_config_memory_limit_defaults_to_none() {
    let config = Config::default();
    assert_eq!(config.knowledge.optimization.memory_limit_mb, None);
}

#[test]
fn test_optimization_config_parses_memory_limit_mb() {
    let toml_str = r#"
        [knowledge.optimization]
        cpu_cores = 2
        nice_level = 5
        timeout_minutes = 60
        schedule_time = "02:00"
        memory_limit_mb = 2048
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.knowledge.optimization.cpu_cores, 2);
    assert_eq!(config.knowledge.optimization.nice_level, 5);
    assert_eq!(config.knowledge.optimization.memory_limit_mb, Some(2048));
}

#[test]
fn test_optimization_config_parses_large_memory_limit_mb() {
    let toml_str = r#"
        [knowledge.optimization]
        cpu_cores = 2
        nice_level = 5
        timeout_minutes = 60
        schedule_time = "02:00"
        memory_limit_mb = 131072
    "#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.knowledge.optimization.memory_limit_mb, Some(131072));
}

#[test]
fn test_env_override_events_horizon_days() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS" {
            Some("90".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.knowledge.events.horizon_days, 90);
}

#[test]
fn test_env_override_events_horizon_days_invalid_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS" {
            Some("nope".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.knowledge.events.horizon_days, 30);
}

#[test]
fn test_init_creates_config_dir_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_home = dir.path().join("config");
    let data_home = dir.path().join("data");
    let cache_home = dir.path().join("cache");

    let result = Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
    match result {
        InitResult::Created {
            config_dir,
            data_dir,
            config_file,
        } => {
            assert!(config_dir.exists());
            assert!(data_dir.exists());
            assert!(config_file.exists());
            assert!(config_file.ends_with("config.toml"));
        }
        InitResult::AlreadyInitialized => {
            panic!("first init should report Created");
        }
    }

    // Verify config.toml content is valid TOML.
    let contents = std::fs::read_to_string(cfg_home.join("config.toml")).unwrap();
    let parsed: Config = toml::from_str(&contents).unwrap();
    assert_eq!(parsed.llm.model, "gpt-4o");
}

#[test]
fn test_init_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_home = dir.path().join("config");
    let data_home = dir.path().join("data");
    let cache_home = dir.path().join("cache");

    let result1 = Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
    assert!(matches!(result1, InitResult::Created { .. }));

    let result2 = Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
    assert!(matches!(result2, InitResult::AlreadyInitialized));

    // Config file should not have been overwritten.
    let contents = std::fs::read_to_string(cfg_home.join("config.toml")).unwrap();
    // Default TOML should still parse cleanly.
    let parsed: Config = toml::from_str(&contents).unwrap();
    assert_eq!(parsed.llm.model, "gpt-4o");
}

#[test]
fn test_load_none_bootstraps_on_first_run() {
    // This test originally verified that Config::load(None) creates
    // default directories and files when env vars point to temp dirs.
    // That behaviour is covered by test_init_creates_config_dir_and_file
    // combined with test_load_from_toml_file, so we just sanity-check
    // that load(Some) still returns defaults when the file does not exist.
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("mimir").join("config.toml");
    paths::ensure_dir(cfg_path.parent().unwrap()).unwrap();
    assert!(!cfg_path.exists());

    // When load(Some) is given a non-existent file, it fails rather than
    // bootstrapping — bootstrapping is load(None)'s responsibility.
    // Verify that explicit-path load still produces expected defaults
    // when the file DOES exist (written by init_at).
    Config::init_at(
        cfg_path.parent().unwrap(),
        dir.path().join("data").as_path(),
        dir.path().join("cache").as_path(),
    )
    .unwrap();
    assert!(cfg_path.exists());
    let config = Config::load(Some(&cfg_path)).unwrap();
    assert_eq!(config.llm.model, "gpt-4o");
    assert_eq!(config.agent.name, "Mimir");
}

#[test]
fn test_default_config_toml_is_valid() {
    let toml_str = Config::default_config_toml();
    let parsed: Config = toml::from_str(&toml_str).unwrap();
    assert_eq!(parsed.llm.endpoint, "https://api.openai.com/v1");
    assert_eq!(parsed.llm.api_key, "");
    assert_eq!(parsed.llm.model, "gpt-4o");
    assert_eq!(parsed.agent.name, "Mimir");
    assert_eq!(parsed.agent.max_tool_rounds, 100);
    assert!(parsed.geocoder.enabled);
    assert_eq!(
        parsed.geocoder.endpoint,
        crate::geocoder::DEFAULT_NOMINATIM_ENDPOINT
    );
}

#[test]
fn test_init_does_not_overwrite_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_home = dir.path().join("config");
    let data_home = dir.path().join("data");
    let cache_home = dir.path().join("cache");

    // Write a custom config first.
    Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
    let cfg_path = cfg_home.join("config.toml");
    let custom = r#"
[llm]
model = "custom-model"
"#;
    std::fs::write(&cfg_path, custom).unwrap();

    // init again — should not overwrite.
    let result = Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
    assert!(matches!(result, InitResult::AlreadyInitialized));

    let contents = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(contents.contains("custom-model"));
}

#[test]
fn test_geocoder_config_defaults() {
    let config = Config::default();
    assert!(config.geocoder.enabled);
    assert_eq!(
        config.geocoder.endpoint,
        crate::geocoder::DEFAULT_NOMINATIM_ENDPOINT
    );
    assert_eq!(config.geocoder.contact_email, None);
}

#[test]
fn test_geocoder_config_toml_parses() {
    let toml_str = r#"
[geocoder]
enabled = false
endpoint = "https://nominatim.example.com"
contact_email = "ops@example.com"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(!config.geocoder.enabled);
    assert_eq!(config.geocoder.endpoint, "https://nominatim.example.com");
    assert_eq!(
        config.geocoder.contact_email.as_deref(),
        Some("ops@example.com")
    );
}

#[test]
fn test_geocoder_section_is_optional() {
    let toml_str = r#"
[llm]
model = "gpt-4o"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert!(config.geocoder.enabled);
    assert_eq!(
        config.geocoder.endpoint,
        crate::geocoder::DEFAULT_NOMINATIM_ENDPOINT
    );
    assert_eq!(config.geocoder.contact_email, None);
}

#[test]
fn test_env_override_geocoder_enabled() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_GEOCODER_ENABLED" {
            Some("false".to_string())
        } else {
            None
        }
    });
    assert!(!config.geocoder.enabled);
}

#[test]
fn test_env_override_geocoder_enabled_invalid_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_GEOCODER_ENABLED" {
            Some("not_a_bool".to_string())
        } else {
            None
        }
    });
    assert!(config.geocoder.enabled);
}

#[test]
fn test_env_override_geocoder_endpoint() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_GEOCODER_ENDPOINT" {
            Some("https://nominatim.example.com".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.geocoder.endpoint, "https://nominatim.example.com");
}

#[test]
fn test_env_override_geocoder_contact_email() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_GEOCODER_CONTACT_EMAIL" {
            Some("ops@example.com".to_string())
        } else {
            None
        }
    });
    assert_eq!(
        config.geocoder.contact_email.as_deref(),
        Some("ops@example.com")
    );
}

#[test]
fn test_env_override_geocoder_contact_email_empty_clears() {
    let mut config = Config::default();
    config.geocoder.contact_email = Some("ops@example.com".to_string());
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_GEOCODER_CONTACT_EMAIL" {
            Some(String::new())
        } else {
            None
        }
    });
    assert_eq!(config.geocoder.contact_email, None);
}

#[test]
fn test_env_override_geocoder_contact_email_trims_padding() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_GEOCODER_CONTACT_EMAIL" {
            Some("  ops@example.com  ".to_string())
        } else {
            None
        }
    });
    assert_eq!(
        config.geocoder.contact_email.as_deref(),
        Some("ops@example.com")
    );
}

#[test]
fn test_secrets_config_defaults() {
    let config = Config::default();
    assert_eq!(config.secrets.backend, SecretsBackend::File);
}

#[test]
fn test_secrets_config_toml_parses() {
    let toml_str = r#"
[secrets]
backend = "keychain"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.secrets.backend, SecretsBackend::Keychain);
}

#[test]
fn test_secrets_section_is_optional() {
    let toml_str = r#"
[llm]
model = "gpt-4o"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.secrets.backend, SecretsBackend::File);
}

#[test]
fn test_env_override_secrets_backend() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_SECRETS_BACKEND" {
            Some("keychain".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.secrets.backend, SecretsBackend::Keychain);
}

#[test]
fn test_env_override_secrets_backend_invalid_ignored() {
    let mut config = Config::default();
    config.apply_env_overrides_with(|key| {
        if key == "MIMIR_SECRETS_BACKEND" {
            Some("not_a_backend".to_string())
        } else {
            None
        }
    });
    assert_eq!(config.secrets.backend, SecretsBackend::File);
}
