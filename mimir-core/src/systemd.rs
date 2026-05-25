//! systemd user service generation and orchestration.
//!
//! Provides [`generate_service_file`] for producing a hardened `.service` unit,
//! [`install_service_file`] for writing it to the user's systemd directory,
//! and the [`SystemdRunner`] trait for reloading and enabling units.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use thiserror::Error;

use crate::paths::{ensure_dir, systemd_user_dir};

/// Errors that can occur while interacting with systemd.
#[derive(Debug, Error)]
pub enum SystemdError {
    /// Failed to determine a required path.
    #[error("path error: {0}")]
    Path(String),

    /// An I/O error occurred while writing the service file.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A systemctl command returned a non-zero exit code.
    #[error("systemctl failed: {stderr}")]
    Systemctl { stderr: String },
}

impl From<crate::paths::PathsError> for SystemdError {
    fn from(err: crate::paths::PathsError) -> Self {
        SystemdError::Path(err.to_string())
    }
}

/// Async trait for systemd operations, mockable in tests.
#[async_trait]
pub trait SystemdRunner: Send + Sync {
    /// Run `systemctl --user daemon-reload`.
    async fn daemon_reload(&self) -> Result<(), SystemdError>;

    /// Run `systemctl --user enable --now <service>`.
    async fn enable_now(&self, service: &str) -> Result<(), SystemdError>;
}

/// Production implementation that spawns `systemctl` via `tokio::process::Command`.
pub struct RealSystemdRunner;

#[async_trait]
impl SystemdRunner for RealSystemdRunner {
    async fn daemon_reload(&self) -> Result<(), SystemdError> {
        run_systemctl(&["--user", "daemon-reload"]).await
    }

    async fn enable_now(&self, service: &str) -> Result<(), SystemdError> {
        run_systemctl(&["--user", "enable", "--now", service]).await
    }
}

async fn run_systemctl(args: &[&str]) -> Result<(), SystemdError> {
    let output = tokio::process::Command::new("systemctl")
        .args(args)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(SystemdError::Systemctl { stderr });
    }
    Ok(())
}

/// Generate a systemd user service file with absolute paths and full hardening.
///
/// `exe_path` should be the absolute path to the `mimir` binary.
/// `config_dir`, `data_dir`, and `cache_dir` are the Mimir directories.
pub fn generate_service_file(
    exe_path: &Path,
    config_dir: &Path,
    data_dir: &Path,
    cache_dir: &Path,
) -> String {
    let exe = exe_path.display();
    let config = config_dir.display();
    let data = data_dir.display();
    let cache = cache_dir.display();

    format!(
        r#"[Unit]
Description=Mimir — persistent personal intelligence
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart="{exe}" start
Restart=on-failure
RestartSec=5

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths="{config}" "{data}" "{cache}"
PrivateTmp=true

# Logging → journalctl --user -u mimir
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
"#
    )
}

/// Install `content` as a systemd user service named `mimir.service`.
///
/// The `dir` parameter is typically `~/.config/systemd/user` (see
/// [`systemd_user_dir`]). Parent directories are created if needed.
/// Returns the path to the written file.
pub fn install_service_file(content: &str, dir: &Path) -> Result<PathBuf, SystemdError> {
    ensure_dir(dir)?;
    let path = dir.join("mimir.service");
    std::fs::write(&path, content)?;
    Ok(path)
}

/// Convenience wrapper: generate and install the service file using platform paths.
///
/// Returns the path to the installed file.
///
/// **Platform note:** This is intended for Linux/systemd contexts. On other
/// platforms the generated path will still follow XDG conventions but systemd
/// itself may not be available.
pub fn generate_and_install_service_file(exe_path: &Path) -> Result<PathBuf, SystemdError> {
    let config = crate::paths::config_dir()?;
    let data = crate::paths::data_dir()?;
    let cache = crate::paths::cache_dir()?;
    let content = generate_service_file(exe_path, &config, &data, &cache);
    let dir = systemd_user_dir()?;
    install_service_file(&content, &dir)
}

/// Mock implementation that records arguments for assertions in tests.
#[derive(Debug, Default)]
pub struct MockSystemdRunner {
    calls: std::sync::Mutex<Vec<String>>,
}

impl MockSystemdRunner {
    /// Return the ordered list of recorded calls.
    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl SystemdRunner for MockSystemdRunner {
    async fn daemon_reload(&self) -> Result<(), SystemdError> {
        self.calls.lock().unwrap().push("daemon_reload".to_string());
        Ok(())
    }

    async fn enable_now(&self, service: &str) -> Result<(), SystemdError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("enable_now:{service}"));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_generate_service_file_contains_expected_sections() {
        let exe = PathBuf::from("/home/user/.cargo/bin/mimir");
        let config = PathBuf::from("/home/user/.config/mimir");
        let data = PathBuf::from("/home/user/.local/share/mimir");
        let cache = PathBuf::from("/home/user/.cache/mimir");

        let content = generate_service_file(&exe, &config, &data, &cache);

        assert!(content.contains("[Unit]"), "should contain [Unit]");
        assert!(content.contains("[Service]"), "should contain [Service]");
        assert!(content.contains("[Install]"), "should contain [Install]");
        assert!(
            content.contains("ExecStart=\"/home/user/.cargo/bin/mimir\" start"),
            "should contain absolute ExecStart"
        );
        assert!(
            content.contains("Restart=on-failure"),
            "should contain Restart=on-failure"
        );
        assert!(
            content.contains("NoNewPrivileges=true"),
            "should contain NoNewPrivileges=true"
        );
        assert!(
            content.contains("ProtectSystem=strict"),
            "should contain ProtectSystem=strict"
        );
        assert!(
            content.contains("ProtectHome=read-only"),
            "should contain ProtectHome=read-only"
        );
        assert!(
            content.contains("PrivateTmp=true"),
            "should contain PrivateTmp=true"
        );
        assert!(
            content.contains("ReadWritePaths=\"/home/user/.config/mimir\" \"/home/user/.local/share/mimir\" \"/home/user/.cache/mimir\""),
            "should contain absolute ReadWritePaths for config, data, and cache"
        );
        assert!(
            content.contains("StandardOutput=journal"),
            "should contain StandardOutput=journal"
        );
        assert!(
            content.contains("StandardError=journal"),
            "should contain StandardError=journal"
        );
    }

    #[test]
    fn test_install_service_file_creates_file_and_parents() {
        let dir = tempfile::tempdir().unwrap();
        let service_dir = dir.path().join("systemd").join("user");
        assert!(!service_dir.exists());

        let content = "[Unit]\nDescription=test\n";
        let path = install_service_file(content, &service_dir).unwrap();

        assert!(path.exists(), "service file should exist");
        assert_eq!(path.file_name().unwrap(), "mimir.service");
        let read_back = std::fs::read_to_string(&path).unwrap();
        assert_eq!(read_back, content);
    }

    #[tokio::test]
    async fn test_mock_systemd_runner_records_daemon_reload() {
        let mock = MockSystemdRunner::default();
        mock.daemon_reload().await.unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls, vec!["daemon_reload"]);
    }

    #[tokio::test]
    async fn test_mock_systemd_runner_records_enable_now() {
        let mock = MockSystemdRunner::default();
        mock.enable_now("mimir").await.unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls, vec!["enable_now:mimir"]);
    }

    #[tokio::test]
    async fn test_mock_systemd_runner_records_multiple_calls() {
        let mock = MockSystemdRunner::default();
        mock.daemon_reload().await.unwrap();
        mock.enable_now("mimir").await.unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls, vec!["daemon_reload", "enable_now:mimir"]);
    }
}
