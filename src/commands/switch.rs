//! Switch command implementation.

use anyhow::{Context, Result, bail};
use dialoguer::{Select, theme::ColorfulTheme};
use std::fs;
use std::process::Command;

use crate::config::{
    get_adc_path, get_context_adc_path, get_current_tracking, list_contexts, load_context_metadata,
    set_current_tracking, switch_kubectl_context, validate_context_name,
};

/// Switches to a saved context.
///
/// This function:
/// 1. Checks if already on the requested context (skips if so)
/// 2. Reads context metadata to get the correct gcloud config name
/// 3. Activates the gcloud configuration
/// 4. Restores the saved ADC credentials
/// 5. Switches kubectl context if saved
/// 6. Updates the current context tracking
///
/// If `quiet` is true, sensitive details (account, project, etc.) are hidden.
pub fn switch_context(name: &str, quiet: bool) -> Result<()> {
    validate_context_name(name)?;
    let stored_adc = get_context_adc_path(name)?;

    if !stored_adc.exists() {
        bail!(
            "Context '{}' not found. Run 'gcpx save {}' first.",
            name,
            name
        );
    }

    // Check if already on this context (fast file read)
    let current = get_current_tracking();
    if current == name {
        println!("Already on context '{}'.", name);
        // Still show the context info (unless quiet)
        if !quiet {
            if let Ok(Some(m)) = load_context_metadata(name) {
                if let Some(acc) = &m.account {
                    println!("  account: {}", acc);
                }
                if let Some(proj) = &m.project {
                    println!("  project: {}", proj);
                }
                if let Some(kctx) = &m.kubectl_context {
                    println!("  kubectl: {}", kctx);
                }
            }
        }
        return Ok(());
    }

    // Load metadata to get the actual gcloud config name
    let metadata = load_context_metadata(name)?;
    let gcloud_config = metadata
        .as_ref()
        .map(|m| m.gcloud_config.as_str())
        .unwrap_or(name); // Fall back to context name for backward compatibility

    // Activate gcloud configuration
    println!("Switching to context '{}'...", name);
    let status = Command::new("gcloud")
        .args(["config", "configurations", "activate", gcloud_config])
        .output()
        .context("Failed to execute gcloud command")?;

    if !status.status.success() {
        let err_msg = String::from_utf8_lossy(&status.stderr);
        bail!(
            "gcloud error: {}\n\nHint: The gcloud config '{}' may not exist. \
            Try running 'gcpx login {}' to re-authenticate.",
            err_msg.trim(),
            gcloud_config,
            name
        );
    }

    // Restore ADC credentials
    let target_adc = get_adc_path()?;
    let content = fs::read(&stored_adc)?;
    fs::write(&target_adc, content)?;

    // Switch kubectl context if saved
    if let Some(ref m) = metadata {
        if let Some(kctx) = &m.kubectl_context {
            if switch_kubectl_context(kctx)? {
                // kubectl switched successfully, will print below
            }
        }
    }

    // Update tracking
    set_current_tracking(name)?;

    println!("Switched to '{}' successfully!", name);
    if !quiet {
        if let Some(m) = &metadata {
            if let Some(acc) = &m.account {
                println!("  account: {}", acc);
            }
            if let Some(proj) = &m.project {
                println!("  project: {}", proj);
            }
            if let Some(kctx) = &m.kubectl_context {
                println!("  kubectl: {}", kctx);
            }
        }
    }
    Ok(())
}

/// Shows an interactive menu to select and switch contexts.
pub fn interactive_switch(quiet: bool) -> Result<()> {
    let contexts = list_contexts()?;
    if contexts.is_empty() {
        println!("No contexts found. Create one with 'gcpx save <name>'");
        return Ok(());
    }

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select GCP Context")
        .default(0)
        .items(&contexts)
        .interact()?;

    switch_context(&contexts[selection], quiet)
}
