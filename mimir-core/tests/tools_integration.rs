use mimir_core::tools::*;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn test_register_native_tool_and_retrieve() {
    let registry = ToolRegistry::new();
    let tool = Arc::new(GetCurrentTimeTool);
    registry
        .register(tool.clone(), ToolSource::Native, ToolPermission::Auto)
        .unwrap();

    let (retrieved, metadata) = registry.get("get_current_time").unwrap();
    assert_eq!(retrieved.name(), "get_current_time");
    assert_eq!(metadata.source, ToolSource::Native);
    assert_eq!(metadata.permission, ToolPermission::Auto);
}

#[tokio::test]
async fn test_export_openai_schema() {
    let registry = ToolRegistry::new();
    registry
        .register(
            Arc::new(GetCurrentTimeTool),
            ToolSource::Native,
            ToolPermission::Auto,
        )
        .unwrap();
    registry
        .register(Arc::new(EchoTool), ToolSource::Native, ToolPermission::Auto)
        .unwrap();

    let schemas = registry.export_openai_tools();
    assert_eq!(schemas.len(), 2);

    let echo_schema = schemas
        .iter()
        .find(|s| s["function"]["name"].as_str() == Some("echo"))
        .unwrap();
    assert_eq!(echo_schema["type"].as_str(), Some("function"));
    assert_eq!(echo_schema["function"]["strict"].as_bool(), Some(true));
    let params = &echo_schema["function"]["parameters"];
    assert_eq!(params["type"].as_str(), Some("object"));
    assert!(params["properties"].get("message").is_some());
    let required = params["required"].as_array().unwrap();
    assert!(required.contains(&json!("message")));
    assert_eq!(params["additionalProperties"].as_bool(), Some(false));
}

#[tokio::test]
async fn test_disabled_tools_not_exported() {
    let registry = ToolRegistry::new();
    registry
        .register(
            Arc::new(EchoTool),
            ToolSource::Native,
            ToolPermission::Disabled,
        )
        .unwrap();
    registry
        .register(
            Arc::new(GetCurrentTimeTool),
            ToolSource::Native,
            ToolPermission::Auto,
        )
        .unwrap();

    let schemas = registry.export_openai_tools();
    assert_eq!(schemas.len(), 1);
    assert_eq!(
        schemas[0]["function"]["name"].as_str(),
        Some("get_current_time")
    );
}

#[tokio::test]
async fn test_get_current_time_execution() {
    let registry = ToolRegistry::new();
    registry
        .register(
            Arc::new(GetCurrentTimeTool),
            ToolSource::Native,
            ToolPermission::Auto,
        )
        .unwrap();

    let output = registry
        .execute("get_current_time", json!({}))
        .await
        .unwrap();
    assert!(output.result.is_some());
    let result = output.result.unwrap().as_str().unwrap().to_string();
    // Verify valid RFC 3339 by parsing with chrono.
    let parsed = chrono::DateTime::parse_from_rfc3339(&result);
    assert!(
        parsed.is_ok(),
        "expected valid RFC 3339 timestamp, got: {result}"
    );
}

#[tokio::test]
async fn test_echo_execution() {
    let registry = ToolRegistry::new();
    registry
        .register(Arc::new(EchoTool), ToolSource::Native, ToolPermission::Auto)
        .unwrap();

    let output = registry
        .execute("echo", json!({"message": "hello world"}))
        .await
        .unwrap();
    assert_eq!(output.result, Some(json!("hello world")));
}

#[tokio::test]
async fn test_permission_disabled_rejects() {
    let registry = ToolRegistry::new();
    registry
        .register(
            Arc::new(EchoTool),
            ToolSource::Native,
            ToolPermission::Disabled,
        )
        .unwrap();

    let err = registry
        .execute("echo", json!({"message": "hi"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::Disabled(_)));
}

#[tokio::test]
async fn test_permission_ask_rejects() {
    let registry = ToolRegistry::new();
    registry
        .register(Arc::new(EchoTool), ToolSource::Native, ToolPermission::Ask)
        .unwrap();

    let err = registry
        .execute("echo", json!({"message": "hi"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::PermissionDenied(_)));
}

#[tokio::test]
async fn test_permission_override() {
    let registry = ToolRegistry::new();
    registry
        .register(Arc::new(EchoTool), ToolSource::Native, ToolPermission::Ask)
        .unwrap();
    registry
        .set_permission("echo", ToolPermission::Auto)
        .unwrap();

    let output = registry
        .execute("echo", json!({"message": "hi"}))
        .await
        .unwrap();
    assert_eq!(output.result, Some(json!("hi")));
}

#[tokio::test]
async fn test_cli_tool_mock_execution() {
    let (executable, args) = if cfg!(windows) {
        (
            std::path::PathBuf::from("cmd.exe"),
            vec![
                "/C".to_string(),
                "echo".to_string(),
                "{{message}}".to_string(),
            ],
        )
    } else {
        (
            std::path::PathBuf::from("/bin/echo"),
            vec!["{{message}}".to_string()],
        )
    };

    let config = CliToolConfig {
        name: "mock_echo".to_string(),
        description: "Echoes args via system echo".to_string(),
        executable,
        args,
        schema: json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"],
            "additionalProperties": false,
        }),
        timeout_secs: 5,
        permission: ToolPermission::Auto,
    };

    let tool = CliTool::new(config);
    let output = tool.execute(json!({"message": "hello_cli"})).await.unwrap();
    assert_eq!(output.result, Some(json!("hello_cli")));
    assert_eq!(output.exit_code, Some(0));
}

#[tokio::test]
#[cfg(unix)]
async fn test_cli_tool_timeout() {
    let config = CliToolConfig {
        name: "slow_sleep".to_string(),
        description: "Sleeps for a long time".to_string(),
        executable: std::path::PathBuf::from("/bin/sleep"),
        args: vec!["10".to_string()],
        schema: json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false }),
        timeout_secs: 1,
        permission: ToolPermission::Auto,
    };

    let tool = CliTool::new(config);
    let err = tool.execute(json!({})).await.unwrap_err();
    assert!(matches!(err, ToolError::Timeout { .. }));
}

#[tokio::test]
#[cfg(unix)]
async fn test_cli_tool_invalid_executable_path() {
    let config = CliToolConfig {
        name: "bad_path".to_string(),
        description: "Tool with relative path".to_string(),
        executable: std::path::PathBuf::from("echo"),
        args: vec![],
        schema: json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false }),
        timeout_secs: 5,
        permission: ToolPermission::Auto,
    };

    let tool = CliTool::new(config);
    let err = tool.execute(json!({})).await.unwrap_err();
    assert!(matches!(err, ToolError::ExecutionFailed { .. }));
}

#[tokio::test]
#[cfg(unix)]
async fn test_cli_tool_missing_template_arg() {
    let config = CliToolConfig {
        name: "templated".to_string(),
        description: "Tool with template arg".to_string(),
        executable: std::path::PathBuf::from("/bin/echo"),
        args: vec!["{{missing}}".to_string()],
        schema: json!({
            "type": "object",
            "properties": {
                "present": { "type": "string" }
            },
            "required": ["present"],
            "additionalProperties": false,
        }),
        timeout_secs: 5,
        permission: ToolPermission::Auto,
    };

    let tool = CliTool::new(config);
    let err = tool.execute(json!({"present": "value"})).await.unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments { .. }));
}

#[tokio::test]
async fn test_tool_output_to_llm_text() {
    let output = ToolOutput {
        result: Some(json!(42)),
        stdout: Some("hello".to_string()),
        stderr: Some("warning".to_string()),
        exit_code: Some(0),
        ..Default::default()
    };

    let text = output.to_llm_text();
    assert!(text.contains("result: 42"));
    assert!(text.contains("stdout: hello"));
    assert!(text.contains("stderr: warning"));
    assert!(text.contains("exit_code: 0"));
}

#[tokio::test]
async fn test_tool_registry_list() {
    let registry = ToolRegistry::new();
    registry
        .register(
            Arc::new(GetCurrentTimeTool),
            ToolSource::Native,
            ToolPermission::Auto,
        )
        .unwrap();
    registry
        .register(Arc::new(EchoTool), ToolSource::Native, ToolPermission::Ask)
        .unwrap();

    let list = registry.list();
    assert_eq!(list.len(), 2);
    assert!(
        list.iter()
            .any(|m| m.name == "get_current_time" && m.permission == ToolPermission::Auto)
    );
    assert!(
        list.iter()
            .any(|m| m.name == "echo" && m.permission == ToolPermission::Ask)
    );
}

#[tokio::test]
async fn test_registry_with_builtins() {
    let registry = ToolRegistry::with_builtins();
    let list = registry.list();
    assert!(list.iter().any(|m| m.name == "get_current_time"));
    assert!(list.iter().any(|m| m.name == "echo"));
}

#[tokio::test]
async fn test_tools_config_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tools.toml");

    let registry = ToolRegistry::with_builtins();
    registry
        .register_cli(CliToolConfig {
            name: "mock_cli".to_string(),
            description: "A mock CLI".to_string(),
            executable: std::path::PathBuf::from("/bin/echo"),
            args: vec!["{{message}}".to_string()],
            schema: serde_json::json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"],
                "additionalProperties": false,
            }),
            timeout_secs: 10,
            permission: ToolPermission::Ask,
        })
        .unwrap();

    registry
        .set_permission("echo", ToolPermission::Disabled)
        .unwrap();
    registry.save_tools_config(&path).unwrap();

    // Load into a fresh registry.
    let registry2 = ToolRegistry::with_builtins();
    registry2.load_tools_config(&path).unwrap();

    let meta = registry2.metadata("mock_cli").unwrap();
    assert_eq!(meta.source, ToolSource::Cli);
    assert_eq!(meta.permission, ToolPermission::Ask);

    let echo_meta = registry2.metadata("echo").unwrap();
    assert_eq!(echo_meta.permission, ToolPermission::Disabled);
}

#[tokio::test]
async fn test_tools_config_loads_permissions_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tools.toml");

    std::fs::write(
        &path,
        r#"
[permissions]
get_current_time = "disabled"
echo = "ask"
"#,
    )
    .unwrap();

    let registry = ToolRegistry::with_builtins();
    registry.load_tools_config(&path).unwrap();

    assert_eq!(
        registry.metadata("get_current_time").unwrap().permission,
        ToolPermission::Disabled
    );
    assert_eq!(
        registry.metadata("echo").unwrap().permission,
        ToolPermission::Ask
    );
}

#[tokio::test]
async fn test_tools_config_default_path() {
    // Just verify it returns a path ending in tools.toml.
    let path = ToolsConfig::default_path();
    if let Some(p) = path {
        assert!(p.to_string_lossy().ends_with("tools.toml"));
    }
}

#[tokio::test]
async fn test_register_duplicate_fails() {
    let registry = ToolRegistry::new();
    registry
        .register(Arc::new(EchoTool), ToolSource::Native, ToolPermission::Auto)
        .unwrap();
    let result = registry.register(Arc::new(EchoTool), ToolSource::Native, ToolPermission::Auto);
    assert!(matches!(result, Err(ToolError::AlreadyRegistered(_))));
}
