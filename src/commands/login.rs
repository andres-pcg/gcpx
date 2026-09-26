//! Login command implementation - re-authenticate and save credentials.

use anyhow::{Context, Result, bail};
use std::process::Command;

use crate::commands::save::save_context;
use crate::config::validate_context_name;

/// Re-authenticates an existing context or creates a new one.
///
/// This function:
/// 1. Activates or creates the gcloud configuration
/// 2. Runs `gcloud auth login --update-adc` (one browser flow for gcloud + ADC)
/// 3. Auto-saves the credentials to the context
///
/// If `quiet` is true, sensitive details are hidden after save.
pub fn login_context(name: &str, quiet: bool) -> Result<()> {
    validate_context_name(name)?;
    // First, try to activate or create the gcloud configuration
    println!("Setting up gcloud configuration '{}'...", name);

    // Check if config exists
    let check = Command::new("gcloud")
        .args(["config", "configurations", "describe", name])
        .output()
        .context("Failed to execute gcloud command")?;

    if check.status.success() {
        // Config exists, activate it
        let status = Command::new("gcloud")
            .args(["config", "configurations", "activate", name])
            .status()
            .context("Failed to activate gcloud configuration")?;

        if !status.success() {
            println!("Warning: Could not activate configuration '{}'", name);
        }
    } else {
        // Config doesn't exist, create it
        println!("Creating new gcloud configuration '{}'...", name);
        let status = Command::new("gcloud")
            .args(["config", "configurations", "create", name])
            .status()
            .context("Failed to create gcloud configuration")?;

        if !status.success() {
            println!("Warning: Could not create configuration '{}'", name);
        }
    }

    // One browser flow for both gcloud and ADC: --update-adc writes the same
    // grant to the ADC file, instead of a second `application-default login`.
    println!("\nStarting gcloud authentication (gcloud + ADC)...");
    println!("A browser window will open for you to sign in.\n");

    let auth_status = Command::new("gcloud")
        .args(["auth", "login", "--update-adc"])
        .status()
        .context("Failed to run gcloud auth login")?;

    if !auth_status.success() {
        bail!("gcloud auth login did not complete; nothing was saved.");
    }

    // Save the context (with kubectl validation)
    println!("\nSaving credentials to context '{}'...", name);
    save_context(name, quiet, false, false)?;

    println!("\nLogin complete! Context '{}' is now ready to use.", name);
    Ok(())
}
