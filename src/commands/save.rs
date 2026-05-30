//! Save command implementation.

use anyhow::{Result, bail};
use dialoguer::{Select, theme::ColorfulTheme};
use std::fs;

use std::process::Command;

use crate::config::{
    ContextMetadata, ensure_dir_0700, extract_project_from_kubectl_context, get_adc_path,
    get_context_dir, get_context_kube_path, get_current_gcloud_account, get_current_gcloud_config,
    get_current_gcloud_project, get_current_kubectl_context, save_context_metadata,
    set_current_tracking, validate_context_name, write_secret,
};

/// Sanitize untrusted strings before printing — strip ASCII control chars
/// (including ESC) so a malicious kubectl context name can't inject terminal
/// escape sequences into the user's terminal.
fn sanitize_for_display(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_control() { '?' } else { c })
        .collect()
}

/// Options for handling a mismatched kubectl context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KubectlMismatchChoice {
    SaveWithoutKubectl,
    SaveWithMismatch,
    Cancel,
}

/// Saves the current gcloud ADC credentials as a named context.
///
/// This function:
/// 1. Checks if ADC credentials exist
/// 2. Captures current gcloud config, account, project, and kubectl context
/// 3. Validates kubectl context matches gcloud project (unless `force` or `no_kubectl`)
/// 4. Copies credentials to the context storage directory
/// 5. Saves metadata (gcloud config name, account, project, kubectl context)
/// 6. Sets secure file permissions (Unix only)
/// 7. Updates the current context tracking
///
/// If `quiet` is true, sensitive details (account, project, etc.) are hidden.
/// If `no_kubectl` is true, kubectl context is not saved.
/// If `force` is true, skip kubectl validation and save as-is.
pub fn save_context(name: &str, quiet: bool, no_kubectl: bool, force: bool) -> Result<()> {
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

    // Capture and validate kubectl context
    let kubectl_context = if no_kubectl {
        None
    } else {
        let raw_context = get_current_kubectl_context();
        validate_kubectl_context(raw_context, project.as_deref(), force)?
    };

    let store_path = get_context_dir(name)?;
    ensure_dir_0700(&store_path)?;

    let dest_adc = store_path.join("adc.json");

    // Read source ADC and write the per-context copy atomically with mode 0600
    // (avoids any window where the secret has wider permissions).
    let content = fs::read(&adc_path)?;
    write_secret(&dest_adc, &content)?;

    // Snapshot kubectl config (minified+flattened) so `gcpx use` can point
    // KUBECONFIG at a per-context file. Best-effort; failure is non-fatal.
    // Written atomically at 0600 because the flattened config embeds bearer
    // tokens / client certs for the cluster.
    if let Some(ref kctx) = kubectl_context {
        let kube_path = get_context_kube_path(name)?;
        let out = Command::new("kubectl")
            .args(["config", "view", "--flatten", "--minify", "--context", kctx])
            .output();
        if let Ok(o) = out {
            if o.status.success() && !o.stdout.is_empty() {
                let _ = write_secret(&kube_path, &o.stdout);
            }
        }
    }

    // Save metadata
    let metadata = ContextMetadata {
        gcloud_config: gcloud_config.clone(),
        account: account.clone(),
        project: project.clone(),
        kubectl_context: kubectl_context.clone(),
    };
    save_context_metadata(name, &metadata)?;

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

/// Validates kubectl context against the gcloud project and prompts user if mismatched.
///
/// Returns:
/// - `Ok(Some(context))` if context should be saved
/// - `Ok(None)` if kubectl context should be skipped
/// - `Err` if user cancels
fn validate_kubectl_context(
    kubectl_context: Option<String>,
    gcloud_project: Option<&str>,
    force: bool,
) -> Result<Option<String>> {
    let kubectl_context = match kubectl_context {
        Some(ctx) => ctx,
        None => return Ok(None),
    };

    // If force mode, skip validation
    if force {
        return Ok(Some(kubectl_context));
    }

    // Try to extract project from kubectl context (only works for GKE contexts)
    let kubectl_project = extract_project_from_kubectl_context(&kubectl_context);

    // If we can't extract project (non-GKE context), allow saving with info message
    let kubectl_project = match kubectl_project {
        Some(p) => p,
        None => {
            eprintln!(
                "  info: kubectl context '{}' is not a GKE context, skipping validation",
                sanitize_for_display(&kubectl_context)
            );
            return Ok(Some(kubectl_context));
        }
    };

    // If no gcloud project set, skip validation
    let gcloud_project = match gcloud_project {
        Some(p) => p,
        None => return Ok(Some(kubectl_context)),
    };

    // Check if projects match
    if kubectl_project == gcloud_project {
        return Ok(Some(kubectl_context));
    }

    // Projects don't match - prompt user
    eprintln!();
    eprintln!("Warning: kubectl context may belong to a different project");
    eprintln!("  gcloud project:  {}", gcloud_project);
    eprintln!(
        "  kubectl context: {} (project: {})",
        sanitize_for_display(&kubectl_context),
        sanitize_for_display(&kubectl_project)
    );
    eprintln!();

    let choice = prompt_kubectl_mismatch()?;

    match choice {
        KubectlMismatchChoice::SaveWithoutKubectl => Ok(None),
        KubectlMismatchChoice::SaveWithMismatch => Ok(Some(kubectl_context)),
        KubectlMismatchChoice::Cancel => bail!("Save cancelled by user."),
    }
}

/// Prompts the user to choose how to handle a mismatched kubectl context.
fn prompt_kubectl_mismatch() -> Result<KubectlMismatchChoice> {
    let options = [
        "Save without kubectl context",
        "Save with mismatched kubectl context",
        "Cancel",
    ];

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("How would you like to proceed?")
        .default(0)
        .items(options)
        .interact()?;

    Ok(match selection {
        0 => KubectlMismatchChoice::SaveWithoutKubectl,
        1 => KubectlMismatchChoice::SaveWithMismatch,
        _ => KubectlMismatchChoice::Cancel,
    })
}
