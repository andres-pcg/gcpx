//! Login command implementation - re-authenticate and save credentials.

use anyhow::{Context, Result};
use std::process::Command;

use crate::commands::save::save_context;
use crate::config::validate_context_name;

/// Re-authenticates an existing context or creates a new one.
///
/// This function:
/// 1. Activates or creates the gcloud configuration
/// 2. Runs `gcloud auth login` for browser-based authentication
/// 3. Runs `gcloud auth application-default login` for ADC
/// 4. Auto-saves the credentials to the context
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

    // Run gcloud auth login (interactive, opens browser)
    println!("\nStarting gcloud authentication...");
    println!("A browser window will open for you to sign in.\n");

    let auth_status = Command::new("gcloud")
        .args(["auth", "login"])
        .status()
        .context("Failed to run gcloud auth login")?;

    if !auth_status.success() {
        println!("Warning: gcloud auth login may not have completed successfully.");
    }

    // Run gcloud auth application-default login
    println!("\nStarting Application Default Credentials authentication...");
    println!("Another browser window will open.\n");

    let adc_status = Command::new("gcloud")
        .args(["auth", "application-default", "login"])
        .status()
        .context("Failed to run gcloud auth application-default login")?;

    if !adc_status.success() {
        println!("Warning: ADC authentication may not have completed successfully.");
    }

    // Save the context
    println!("\nSaving credentials to context '{}'...", name);
    save_context(name, quiet)?;

    println!("\nLogin complete! Context '{}' is now ready to use.", name);
    Ok(())
}
