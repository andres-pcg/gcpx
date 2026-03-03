//! Workspace- and project-level configuration for gcpx.
//!
//! Discovers a config file (`.gcpx.toml`) by walking from the current directory
//! up through parent directories. This allows pinning a project or workspace to
//! a specific gcpx context (e.g. which gcloud config to use).

use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Names of config files we look for (in order).
const CONFIG_FILENAMES: &[&str] = &[".gcpx.toml", ".gcpx.json"];

/// Workspace-level gcpx configuration.
#[derive(Debug, Clone)]
pub struct WorkspaceConfig {
    /// Pinned gcpx context name for this workspace.
    pub context: Option<String>,
}

/// Raw shape of `.gcpx.toml` (TOML).
#[derive(Debug, Deserialize)]
struct TomlWorkspaceConfig {
    context: Option<String>,
}

/// Raw shape of `.gcpx.json` (JSON).
#[derive(Debug, Deserialize)]
struct JsonWorkspaceConfig {
    context: Option<String>,
}

/// Finds the nearest workspace config by walking from `start_dir` upward.
///
/// Stops at the user's home directory or filesystem root, whichever is reached
/// first. Returns the directory containing the config file and the parsed config,
/// or `None` if no config file is found.
pub fn find_workspace_config(start_dir: &Path) -> Result<Option<(PathBuf, WorkspaceConfig)>> {
    let home = dirs::home_dir();
    let mut current = start_dir
        .canonicalize()
        .unwrap_or_else(|_| start_dir.to_path_buf());

    loop {
        for name in CONFIG_FILENAMES {
            let path = current.join(name);
            if path.exists() {
                let config = parse_config_file(&path)?;
                return Ok(Some((current, config)));
            }
        }

        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
            // Stop at home to avoid reading config from unrelated parents
            if let Some(ref h) = home {
                if current == *h {
                    break;
                }
            }
        } else {
            break;
        }
    }

    Ok(None)
}

fn parse_config_file(path: &Path) -> Result<WorkspaceConfig> {
    let content = std::fs::read_to_string(path)?;
    let context = if path.extension().and_then(|e| e.to_str()) == Some("json") {
        let parsed: JsonWorkspaceConfig = serde_json::from_str(&content)?;
        parsed.context
    } else {
        let parsed: TomlWorkspaceConfig = toml::from_str(&content)?;
        parsed.context
    };
    Ok(WorkspaceConfig { context })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn find_workspace_config_no_file_returns_none() {
        let tmp = std::env::temp_dir();
        let result = find_workspace_config(&tmp).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn find_workspace_config_finds_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gcpx.toml");
        fs::write(&path, "context = \"my-ctx\"\n").unwrap();
        let (dir, config) = find_workspace_config(tmp.path()).unwrap().unwrap();
        assert!(
            dir.join(".gcpx.toml").exists(),
            "config should be in returned dir"
        );
        assert_eq!(config.context.as_deref(), Some("my-ctx"));
    }

    #[test]
    fn find_workspace_config_finds_in_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("a").join("b");
        fs::create_dir_all(&sub).unwrap();
        let path = tmp.path().join(".gcpx.toml");
        fs::write(&path, "context = \"project\"\n").unwrap();
        let (dir, config) = find_workspace_config(&sub).unwrap().unwrap();
        assert!(
            dir.join(".gcpx.toml").exists(),
            "config should be in project root"
        );
        assert_eq!(config.context.as_deref(), Some("project"));
    }
}
