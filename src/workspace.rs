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

/// Heuristic trust check for a directory containing a `.gcpx.toml`.
///
/// Returns false if the directory itself or any ancestor up to (but not
/// including) the user's home is world-writable without the sticky bit. The
/// auto-switch hook runs this on `chpwd`, so a `cd /tmp/attacker/repo` should
/// NOT silently re-point credentials based on an attacker-dropped config.
///
/// Directories under the user's home are trusted by default (the user owns
/// them). Trust can also be bypassed entirely with GCPX_TRUST_ALL_WORKSPACES=1
/// for users who know what they're doing.
pub fn is_trusted_workspace(dir: &Path) -> bool {
    if std::env::var("GCPX_TRUST_ALL_WORKSPACES").as_deref() == Ok("1") {
        return true;
    }
    let home = dirs::home_dir();
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

    // Under $HOME → trusted (user owns it).
    if let Some(ref h) = home {
        if canonical.starts_with(h) {
            return true;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Walk from canonical up to /, refusing if any component is world-writable
        // without the sticky bit (i.e. anyone can replace it).
        let mut cur: Option<&Path> = Some(&canonical);
        while let Some(p) = cur {
            if let Ok(meta) = std::fs::metadata(p) {
                let mode = meta.permissions().mode();
                let world_writable = mode & 0o002 != 0;
                let sticky = mode & 0o1000 != 0;
                if world_writable && !sticky {
                    return false;
                }
            }
            cur = p.parent();
        }
        true
    }
    #[cfg(not(unix))]
    {
        true
    }
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
