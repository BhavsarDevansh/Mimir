use super::{Tool, ToolError, ToolOutput, ToolPermission};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Configuration for a CLI tool loaded from `tools.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CliToolConfig {
    pub name: String,
    pub description: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub schema: Value,
    #[serde(default = "default_cli_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub permission: ToolPermission,
}

fn default_cli_timeout() -> u64 {
    30
}

/// A tool that wraps a CLI executable.
pub struct CliTool {
    config: CliToolConfig,
}

impl CliTool {
    pub fn new(config: CliToolConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &CliToolConfig {
        &self.config
    }

    /// Replace template placeholders `{{key}}` in args with values from JSON args.
    fn render_args(&self, args: &Value) -> Result<Vec<String>, ToolError> {
        let mut rendered = Vec::with_capacity(self.config.args.len());

        for arg in &self.config.args {
            if arg.starts_with("{{") && arg.ends_with("}}") {
                let key = &arg[2..arg.len() - 2];
                let value = args.get(key).ok_or_else(|| {
                    ToolError::invalid_arguments(
                        &self.config.name,
                        format!("missing template argument '{key}'"),
                    )
                })?;
                let value_str = serde_json::to_string(value).map_err(|e| {
                    ToolError::invalid_arguments(
                        &self.config.name,
                        format!("failed to serialize argument '{key}': {e}"),
                    )
                })?;
                // If the value is a simple string, unwrap quotes for better CLI ergonomics.
                let final_str = if let Some(s) = value.as_str() {
                    s.to_string()
                } else {
                    value_str
                };
                rendered.push(final_str);
            } else {
                rendered.push(arg.clone());
            }
        }

        Ok(rendered)
    }
}

#[async_trait]
impl Tool for CliTool {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn description(&self) -> &str {
        &self.config.description
    }

    fn parameters_schema(&self) -> Value {
        self.config.schema.clone()
    }

    fn permission(&self) -> ToolPermission {
        self.config.permission
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        if !self.config.executable.is_absolute() {
            return Err(ToolError::execution_failed(
                &self.config.name,
                "CLI executable must be an absolute path",
            ));
        }

        let rendered_args = self.render_args(&args)?;
        let mut cmd = Command::new(&self.config.executable);
        cmd.args(&rendered_args);
        cmd.kill_on_drop(true);

        let dur = Duration::from_secs(self.config.timeout_secs);
        let result = timeout(dur, cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let exit_code = output.status.code().unwrap_or(-1);

                let tool_output = ToolOutput {
                    result: Some(Value::String(stdout.trim().to_string())),
                    stdout: Some(stdout),
                    stderr: Some(stderr),
                    exit_code: Some(exit_code),
                    ..Default::default()
                };

                Ok(tool_output)
            }
            Ok(Err(e)) => Err(ToolError::cli_process_error(
                &self.config.name,
                format!("failed to spawn process: {e}"),
            )),
            Err(_) => Err(ToolError::timeout(&self.config.name, dur)),
        }
    }
}
