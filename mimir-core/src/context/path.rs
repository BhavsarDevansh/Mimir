//! Path helpers: tilde expansion for database locations.

use std::path::{Path, PathBuf};

pub(super) fn expand_tilde(path: &Path) -> PathBuf {
    if let Some(s) = path.to_str()
        && let Some(stripped) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    path.to_path_buf()
}

#[cfg(test)]
pub(super) fn expand_tilde_with_home(path: &Path, home: &Path) -> PathBuf {
    if let Some(s) = path.to_str()
        && let Some(stripped) = s.strip_prefix("~/")
    {
        return home.join(stripped);
    }
    path.to_path_buf()
}
