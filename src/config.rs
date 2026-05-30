//! Configuration and path management for gcpx.
//!
//! This module handles all file system paths and tracking of the current context.
//!
//! ## Testing
//!
//! For testing purposes, set the `GCPX_HOME` environment variable to override
//! the storage directory location.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Returns true when test-only env overrides (GCPX_HOME, GCPX_GCLOUD_DIR) are allowed.
///
/// Honored only when GCPX_ALLOW_TEST_ENV=1 is also set. This prevents an attacker
/// who can poison a victim's environment from redirecting credential storage to
/// an attacker-controlled directory.
fn test_env_allowed() -> bool {
    env::var("GCPX_ALLOW_TEST_ENV").as_deref() == Ok("1")
}

/// Atomically create a file with mode 0600 and write bytes to it. On Unix the
/// file is opened with O_CREAT | O_EXCL and mode 0600 in one syscall so the
/// secret never appears with a wider permission, even briefly.
///
/// If `path` already exists it is unlinked first (we are overwriting a secret
/// we own). Parent directory must exist.
pub fn write_secret(path: &Path, content: &[u8]) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing existing {:?}", path))?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("creating {:?}", path))?;
        f.write_all(content)
            .with_context(|| format!("writing {:?}", path))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, content)?;
    }
    Ok(())
}

/// Ensure a directory exists with mode 0700 on Unix.
pub fn ensure_dir_0700(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating dir {:?}", path))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o700);
        fs::set_permissions(path, perms).with_context(|| format!("chmod 0700 {:?}", path))?;
    }
    Ok(())
}

/// Validates a context name to prevent directory traversal and invalid names.
///
/// Rejects names that:
/// - Are empty
/// - Contain path separators (`/` or `\`)
/// - Are `.` or `..`
/// - Contain control characters (ASCII < 32)
/// - Start with a dot (reserved for internal files like `.current`)
pub fn validate_context_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Context name cannot be empty.");
    }
    if name == "." || name == ".." {
        bail!("Context name cannot be '.' or '..'.");
    }
    if name.starts_with('.') {
        bail!("Context name cannot start with a dot.");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("Context name cannot contain path separators ('/' or '\\').");
    }
    if name.chars().any(|c| c.is_ascii_control()) {
        bail!("Context name cannot contain control characters.");
    }
    Ok(())
}

/// Metadata stored alongside each context's credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMetadata {
    /// The gcloud configuration name that was active when saved
    pub gcloud_config: String,
    /// The account email associated with this context
    pub account: Option<String>,
    /// The project ID (if set)
    pub project: Option<String>,
    /// The kubectl context that was active when saved (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kubectl_context: Option<String>,
}

/// Returns the user's home directory.
pub fn get_home() -> Result<PathBuf> {
    dirs::home_dir().context("Could not find home directory")
}

/// Returns the gcloud configuration directory (~/.config/gcloud).
/// Can be overridden with GCPX_GCLOUD_DIR environment variable for testing.
pub fn get_gcloud_dir() -> Result<PathBuf> {
    if test_env_allowed() {
        if let Ok(dir) = env::var("GCPX_GCLOUD_DIR") {
            return Ok(PathBuf::from(dir));
        }
    }
    Ok(get_home()?.join(".config").join("gcloud"))
}

/// Returns the gcpx storage directory (~/.config/gcpx).
/// Creates the directory if it doesn't exist.
/// Can be overridden with GCPX_HOME environment variable for testing.
pub fn get_store_dir() -> Result<PathBuf> {
    let path = if test_env_allowed() {
        if let Ok(dir) = env::var("GCPX_HOME") {
            PathBuf::from(dir)
        } else {
            get_home()?.join(".config").join("gcpx")
        }
    } else {
        get_home()?.join(".config").join("gcpx")
    };
    ensure_dir_0700(&path)?;
    Ok(path)
}

/// Returns the path to the Application Default Credentials file.
pub fn get_adc_path() -> Result<PathBuf> {
    Ok(get_gcloud_dir()?.join("application_default_credentials.json"))
}

/// Returns the path to a context's stored ADC file.
pub fn get_context_adc_path(name: &str) -> Result<PathBuf> {
    Ok(get_store_dir()?.join(name).join("adc.json"))
}

/// Returns the path to the context directory.
pub fn get_context_dir(name: &str) -> Result<PathBuf> {
    Ok(get_store_dir()?.join(name))
}

/// Returns the path to a context's metadata file.
pub fn get_context_metadata_path(name: &str) -> Result<PathBuf> {
    Ok(get_store_dir()?.join(name).join("metadata.json"))
}

/// Returns the path to a context's snapshot kubeconfig file.
/// This is exported as `KUBECONFIG` by `gcpx use` so each shell gets isolated
/// kubectl state without touching `~/.kube/config`.
pub fn get_context_kube_path(name: &str) -> Result<PathBuf> {
    Ok(get_store_dir()?.join(name).join("kube.config"))
}

/// Saves context metadata.
pub fn save_context_metadata(name: &str, metadata: &ContextMetadata) -> Result<()> {
    let path = get_context_metadata_path(name)?;
    let content = serde_json::to_string_pretty(metadata)?;
    fs::write(&path, content)?;
    Ok(())
}

/// Loads context metadata.
pub fn load_context_metadata(name: &str) -> Result<Option<ContextMetadata>> {
    let path = get_context_metadata_path(name)?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)?;
    let metadata: ContextMetadata = serde_json::from_str(&content)?;
    Ok(Some(metadata))
}

/// Gets the current active gcloud configuration name.
/// Returns "default" if gcloud is not installed or no active config is found.
pub fn get_current_gcloud_config() -> Result<String> {
    let output = match std::process::Command::new("gcloud")
        .args([
            "config",
            "configurations",
            "list",
            "--filter=is_active=true",
            "--format=value(name)",
        ])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok("default".to_string()),
    };

    let config = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if config.is_empty() {
        Ok("default".to_string())
    } else {
        Ok(config)
    }
}

/// Gets the current gcloud account.
/// Returns None if gcloud is not installed or no account is set.
pub fn get_current_gcloud_account() -> Result<Option<String>> {
    let output = match std::process::Command::new("gcloud")
        .args(["config", "get-value", "account"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };

    let account = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if account.is_empty() || account == "(unset)" {
        Ok(None)
    } else {
        Ok(Some(account))
    }
}

/// Gets the current gcloud project.
/// Returns None if gcloud is not installed or no project is set.
pub fn get_current_gcloud_project() -> Result<Option<String>> {
    let output = match std::process::Command::new("gcloud")
        .args(["config", "get-value", "project"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };

    let project = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if project.is_empty() || project == "(unset)" {
        Ok(None)
    } else {
        Ok(Some(project))
    }
}

/// Gets the current kubectl context (if kubectl is available).
pub fn get_current_kubectl_context() -> Option<String> {
    let output = std::process::Command::new("kubectl")
        .args(["config", "current-context"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let context = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if context.is_empty() {
        None
    } else {
        Some(context)
    }
}

/// Extracts the GCP project ID from a GKE kubectl context name.
///
/// GKE contexts follow the pattern: `gke_<project>_<region>_<cluster>`
/// Returns `None` for non-GKE contexts (e.g., minikube, kind, custom contexts).
pub fn extract_project_from_kubectl_context(context: &str) -> Option<String> {
    if context.starts_with("gke_") {
        context
            .split('_')
            .nth(1)
            .filter(|s| !s.is_empty())
            .map(String::from)
    } else {
        None
    }
}

/// Switches kubectl context. Returns Ok(true) if switched, Ok(false) if kubectl not available.
pub fn switch_kubectl_context(context: &str) -> Result<bool> {
    let status = std::process::Command::new("kubectl")
        .args(["config", "use-context", context])
        .output();

    match status {
        Ok(output) => {
            if output.status.success() {
                Ok(true)
            } else {
                // Context might not exist, log but don't fail
                let err = String::from_utf8_lossy(&output.stderr);
                if !err.is_empty() {
                    eprintln!("  kubectl warning: {}", err.trim());
                }
                Ok(false)
            }
        }
        Err(_) => {
            // kubectl not installed, that's fine
            Ok(false)
        }
    }
}

/// Sets the current active context in the tracking file.
pub fn set_current_tracking(name: &str) -> Result<()> {
    let path = get_store_dir()?.join(".current");
    fs::write(path, name)?;
    Ok(())
}

/// Gets the current active context from the tracking file.
/// Returns "none" if no context is set or on error.
pub fn get_current_tracking() -> String {
    let path = match get_store_dir() {
        Ok(p) => p.join(".current"),
        Err(_) => return "none".to_string(),
    };
    fs::read_to_string(path).unwrap_or_else(|_| "none".to_string())
}

/// Lists all saved context names.
pub fn list_contexts() -> Result<Vec<String>> {
    let store_dir = get_store_dir()?;
    let mut contexts = Vec::new();

    if store_dir.exists() {
        for entry in fs::read_dir(store_dir)? {
            let entry = entry?;
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    // Skip hidden files/directories
                    if !name.starts_with('.') {
                        contexts.push(name.to_string());
                    }
                }
            }
        }
    }
    contexts.sort();
    Ok(contexts)
}

/// Checks if a context exists.
pub fn context_exists(name: &str) -> Result<bool> {
    let adc_path = get_context_adc_path(name)?;
    Ok(adc_path.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_home_returns_path() {
        let home = get_home();
        assert!(home.is_ok());
        assert!(home.unwrap().exists());
    }

    #[test]
    fn test_gcloud_dir_is_under_home() {
        let home = get_home().unwrap();
        let gcloud = get_gcloud_dir().unwrap();
        assert!(gcloud.starts_with(&home));
    }

    #[test]
    fn test_extract_project_from_gke_context() {
        // Standard GKE context format
        assert_eq!(
            extract_project_from_kubectl_context("gke_my-project_us-east1_my-cluster"),
            Some("my-project".to_string())
        );

        // Project with hyphens
        assert_eq!(
            extract_project_from_kubectl_context("gke_my-gcp-project-123_us-central1-a_cluster"),
            Some("my-gcp-project-123".to_string())
        );

        // Real-world example
        assert_eq!(
            extract_project_from_kubectl_context("gke_indexa-core_us-east1_gravitee"),
            Some("indexa-core".to_string())
        );
    }

    #[test]
    fn test_extract_project_from_non_gke_context() {
        // Non-GKE contexts should return None
        assert_eq!(extract_project_from_kubectl_context("minikube"), None);
        assert_eq!(extract_project_from_kubectl_context("kind-kind"), None);
        assert_eq!(extract_project_from_kubectl_context("docker-desktop"), None);
        assert_eq!(
            extract_project_from_kubectl_context("my-custom-context"),
            None
        );
    }

    #[test]
    fn test_extract_project_from_malformed_gke_context() {
        // Starts with gke_ but missing parts
        assert_eq!(extract_project_from_kubectl_context("gke_"), None);
        // Only prefix, no project
        assert_eq!(
            extract_project_from_kubectl_context("gke_project"),
            Some("project".to_string())
        );
    }
}
