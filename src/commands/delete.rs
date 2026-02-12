//! Delete command implementation.

use anyhow::{Context, Result, bail};
use std::fs;
use std::process::Command;

use crate::config::{context_exists, get_context_dir, get_current_tracking, validate_context_name};

/// Deletes a saved context.
///
/// This function:
/// 1. Removes the stored ADC credentials
/// 2. Optionally deletes the gcloud configuration as well
pub fn delete_context(name: &str, delete_gcloud_config: bool) -> Result<()> {
    validate_context_name(name)?;
    if !context_exists(name)? {
        bail!("Context '{}' not found.", name);
    }

    let current = get_current_tracking();
    if current == name {
        println!(
            "Warning: '{}' is the currently active context. \
            You may want to switch to another context first.",
            name
        );
    }

    // Delete the stored context directory
    let context_dir = get_context_dir(name)?;
    fs::remove_dir_all(&context_dir)
        .with_context(|| format!("Failed to delete context directory: {:?}", context_dir))?;

    println!("Deleted context '{}'.", name);

    // Optionally delete the gcloud configuration
    if delete_gcloud_config {
        println!("Deleting gcloud configuration '{}'...", name);
        let status = Command::new("gcloud")
            .args(["config", "configurations", "delete", name, "--quiet"])
            .output()
            .context("Failed to execute gcloud command")?;

        if !status.status.success() {
            let err_msg = String::from_utf8_lossy(&status.stderr);
            // Don't fail if config doesn't exist
            if !err_msg.contains("does not exist") {
                bail!("gcloud error: {}", err_msg);
            }
        } else {
            println!("Deleted gcloud configuration '{}'.", name);
        }
    }

    Ok(())
}
