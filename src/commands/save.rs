//! Save command implementation.

use anyhow::{Result, bail};
use std::fs;

use crate::config::{
    ContextMetadata, get_adc_path, get_context_dir, get_current_gcloud_account,
    get_current_gcloud_config, get_current_gcloud_project, get_current_kubectl_context,
    save_context_metadata, set_current_tracking, validate_context_name,
};

/// Saves the current gcloud ADC credentials as a named context.
///
/// This function:
/// 1. Checks if ADC credentials exist
/// 2. Captures current gcloud config, account, project, and kubectl context
/// 3. Copies credentials to the context storage directory
/// 4. Saves metadata (gcloud config name, account, project, kubectl context)
/// 5. Sets secure file permissions (Unix only)
/// 6. Updates the current context tracking
///
/// If `quiet` is true, sensitive details (account, project, etc.) are hidden.
pub fn save_context(name: &str, quiet: bool) -> Result<()> {
    validate_context_name(name)?;
    let adc_path = get_adc_path()?;

    // Check if credentials exist
    if !adc_path.exists() {
        bail!(
            "No credentials found at {:?}.\nRun 'gcloud auth application-default login' first!",
            adc_path
        );
    }

    // Capture current gcloud state
    let gcloud_config = get_current_gcloud_config()?;
    let account = get_current_gcloud_account()?;
    let project = get_current_gcloud_project()?;

    // Capture current kubectl context (if available)
    let kubectl_context = get_current_kubectl_context();

    let store_path = get_context_dir(name)?;
    fs::create_dir_all(&store_path)?;

    let dest_adc = store_path.join("adc.json");

    // Read and save credentials
    let content = fs::read(&adc_path)?;
    fs::write(&dest_adc, content)?;

    // Save metadata
    let metadata = ContextMetadata {
        gcloud_config: gcloud_config.clone(),
        account: account.clone(),
        project: project.clone(),
        kubectl_context: kubectl_context.clone(),
    };
    save_context_metadata(name, &metadata)?;

    // Set secure permissions (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dest_adc)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&dest_adc, perms)?;
    }

    println!("Context '{}' saved.", name);
    if !quiet {
        println!("  gcloud config: {}", gcloud_config);
        if let Some(acc) = &account {
            println!("  account: {}", acc);
        }
        if let Some(proj) = &project {
            println!("  project: {}", proj);
        }
        if let Some(kctx) = &kubectl_context {
            println!("  kubectl: {}", kctx);
        }
    }
    set_current_tracking(name)?;
    Ok(())
}
