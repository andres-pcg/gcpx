//! Integration tests for gcpx.
//!
//! These tests use temporary directories to isolate test environments.

use std::env;
use std::fs;
use tempfile::TempDir;

/// Helper to set up a test environment with temporary directories.
struct TestEnv {
    _gcpx_dir: TempDir,
    _gcloud_dir: TempDir,
}

impl TestEnv {
    fn new() -> Self {
        let gcpx_dir = TempDir::new().expect("Failed to create temp gcpx dir");
        let gcloud_dir = TempDir::new().expect("Failed to create temp gcloud dir");

        // SAFETY: Tests run serially (cargo test runs single-threaded by default for
        // integration tests), so setting env vars is safe here.
        unsafe {
            env::set_var("GCPX_HOME", gcpx_dir.path());
            env::set_var("GCPX_GCLOUD_DIR", gcloud_dir.path());
        }

        TestEnv {
            _gcpx_dir: gcpx_dir,
            _gcloud_dir: gcloud_dir,
        }
    }

    fn gcpx_path(&self) -> &std::path::Path {
        self._gcpx_dir.path()
    }

    fn gcloud_path(&self) -> &std::path::Path {
        self._gcloud_dir.path()
    }

    /// Creates a fake ADC file in the gcloud directory.
    fn create_fake_adc(&self) {
        let adc_path = self
            .gcloud_path()
            .join("application_default_credentials.json");
        let fake_adc = r#"{
            "client_id": "test-client-id",
            "client_secret": "test-secret",
            "refresh_token": "test-refresh-token",
            "type": "authorized_user"
        }"#;
        fs::write(adc_path, fake_adc).expect("Failed to create fake ADC");
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        // SAFETY: Tests run serially, so removing env vars is safe here.
        unsafe {
            env::remove_var("GCPX_HOME");
            env::remove_var("GCPX_GCLOUD_DIR");
        }
    }
}

#[test]
fn test_list_contexts_empty() {
    let _env = TestEnv::new();

    let contexts = gcpx::list_contexts().expect("Failed to list contexts");
    assert!(contexts.is_empty());
}

#[test]
fn test_get_current_tracking_default() {
    let _env = TestEnv::new();

    let current = gcpx::get_current_tracking();
    assert_eq!(current, "none");
}

#[test]
fn test_save_context_without_adc_fails() {
    let _env = TestEnv::new();

    let result = gcpx::save_context("test-context", false);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No credentials found")
    );
}

#[test]
fn test_save_and_list_context() {
    let env = TestEnv::new();
    env.create_fake_adc();

    // Save context
    gcpx::save_context("my-project", false).expect("Failed to save context");

    // Verify context appears in list
    let contexts = gcpx::list_contexts().expect("Failed to list contexts");
    assert_eq!(contexts, vec!["my-project"]);

    // Verify current tracking is updated
    let current = gcpx::get_current_tracking();
    assert_eq!(current, "my-project");

    // Verify ADC file was copied
    let adc_path = env.gcpx_path().join("my-project").join("adc.json");
    assert!(adc_path.exists());
}

#[test]
fn test_context_exists() {
    let env = TestEnv::new();
    env.create_fake_adc();

    // Context doesn't exist yet
    assert!(!gcpx::config::context_exists("new-project").unwrap());

    // Save context
    gcpx::save_context("new-project", false).expect("Failed to save context");

    // Now context exists
    assert!(gcpx::config::context_exists("new-project").unwrap());
}

#[test]
fn test_multiple_contexts() {
    let env = TestEnv::new();
    env.create_fake_adc();

    // Save multiple contexts
    gcpx::save_context("project-a", false).expect("Failed to save context a");
    gcpx::save_context("project-b", false).expect("Failed to save context b");
    gcpx::save_context("project-c", false).expect("Failed to save context c");

    // List should have all three (sorted)
    let contexts = gcpx::list_contexts().expect("Failed to list contexts");
    assert_eq!(contexts, vec!["project-a", "project-b", "project-c"]);
}

#[test]
fn test_switch_nonexistent_context_fails() {
    let _env = TestEnv::new();

    let result = gcpx::switch_context("nonexistent", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[test]
fn test_delete_context() {
    let env = TestEnv::new();
    env.create_fake_adc();

    // Save and then delete
    gcpx::save_context("to-delete", false).expect("Failed to save context");
    assert!(gcpx::config::context_exists("to-delete").unwrap());

    gcpx::delete_context("to-delete", false).expect("Failed to delete context");
    assert!(!gcpx::config::context_exists("to-delete").unwrap());
}

#[test]
fn test_delete_nonexistent_context_fails() {
    let _env = TestEnv::new();

    let result = gcpx::delete_context("nonexistent", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[test]
fn test_run_with_nonexistent_context_fails() {
    let _env = TestEnv::new();

    let result = gcpx::run_with_context("nonexistent", &["echo".to_string(), "hello".to_string()]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[test]
fn test_run_with_empty_command_fails() {
    let env = TestEnv::new();
    env.create_fake_adc();
    gcpx::save_context("test-ctx", false).expect("Failed to save context");

    let result = gcpx::run_with_context("test-ctx", &[]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("No command"));
}

#[test]
fn test_save_context_quiet_mode() {
    let env = TestEnv::new();
    env.create_fake_adc();

    // Save with quiet=true should succeed without errors
    gcpx::save_context("quiet-project", true).expect("Failed to save context in quiet mode");

    // Verify context was saved correctly despite quiet mode
    let contexts = gcpx::list_contexts().expect("Failed to list contexts");
    assert!(contexts.contains(&"quiet-project".to_string()));

    // Verify ADC file was copied
    let adc_path = env.gcpx_path().join("quiet-project").join("adc.json");
    assert!(adc_path.exists());

    // Verify metadata was saved
    let metadata =
        gcpx::config::load_context_metadata("quiet-project").expect("Failed to load metadata");
    assert!(metadata.is_some());

    // Verify tracking was updated
    let current = gcpx::get_current_tracking();
    assert_eq!(current, "quiet-project");
}

#[test]
fn test_switch_context_quiet_mode() {
    let _env = TestEnv::new();

    // Switching to nonexistent context should fail the same way in quiet mode
    let result = gcpx::switch_context("nonexistent", true);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[test]
fn test_validate_context_name_rejects_traversal() {
    let _env = TestEnv::new();

    // Directory traversal attempts should be rejected
    let result = gcpx::save_context("../etc", false);
    assert!(result.is_err()); // Rejected (starts with dot)

    let result = gcpx::save_context("a/../etc", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("path separators"));

    let result = gcpx::save_context("..", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("'..'"));

    let result = gcpx::save_context(".", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("'.'"));

    let result = gcpx::save_context("", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("empty"));

    let result = gcpx::save_context(".hidden", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("dot"));

    let result = gcpx::save_context("foo/bar", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("path separators"));

    let result = gcpx::save_context("foo\\bar", false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("path separators"));
}

#[test]
fn test_validate_context_name_accepts_valid() {
    // Validation should accept valid names (even if save fails due to no ADC)
    assert!(gcpx::validate_context_name("my-project").is_ok());
    assert!(gcpx::validate_context_name("work").is_ok());
    assert!(gcpx::validate_context_name("project_123").is_ok());
    assert!(gcpx::validate_context_name("My-GCP-Context").is_ok());
}

#[cfg(unix)]
#[test]
fn test_adc_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let env = TestEnv::new();
    env.create_fake_adc();

    gcpx::save_context("secure-project", false).expect("Failed to save context");

    let adc_path = env.gcpx_path().join("secure-project").join("adc.json");
    let metadata = fs::metadata(adc_path).expect("Failed to get metadata");
    let mode = metadata.permissions().mode() & 0o777;

    assert_eq!(mode, 0o600, "ADC file should have 0600 permissions");
}
