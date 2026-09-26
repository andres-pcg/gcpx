//! Status command — show credential health for saved contexts.

use anyhow::{Result, bail};
use std::time::Duration;

use crate::auth::{ContextHealth, CredState, check_contexts};
use crate::commands::save::sanitize_for_display;
use crate::config::{context_exists, list_contexts, validate_context_name};

fn icon(state: &CredState) -> &'static str {
    match state {
        CredState::Valid => "✓",
        CredState::ReauthRequired => "!",
        CredState::Revoked | CredState::Missing => "✗",
        CredState::NotChecked(_) | CredState::Unknown(_) => "?",
    }
}

fn print_health(h: &ContextHealth) {
    let acct = h
        .expected_account
        .as_deref()
        .map(sanitize_for_display)
        .unwrap_or_else(|| "(no account saved)".into());
    println!("{}  {}", h.name, acct);

    match &h.gcloud {
        Some(s) => println!("  gcloud  {} {}", icon(s), s.label()),
        None => println!("  gcloud  ? no account to check"),
    }
    if h.config_mismatch() {
        println!(
            "          ✗ config '{}' is set to {} (expected {})",
            h.gcloud_config,
            sanitize_for_display(h.config_account.as_deref().unwrap_or("?")),
            acct
        );
    }

    let adc_note = if h.adc_stale() && h.fresher_adc_available {
        " — a working copy exists; `gcpx reauth --adc` adopts it without a browser"
    } else if h.adc_stale() {
        " — only needed for Terraform/client libraries"
    } else {
        ""
    };
    println!(
        "  adc     {} {}{}",
        icon(&h.adc.state),
        h.adc.state.label(),
        adc_note
    );
    if h.adc_mismatch() {
        println!(
            "          ✗ belongs to {} (expected {})",
            sanitize_for_display(h.adc.email.as_deref().unwrap_or("?")),
            acct
        );
    }
}

/// `gcpx status [name]`. Returns an error (non-zero exit) when any checked
/// context has a stale gcloud credential or an account mismatch, so it can be
/// used in scripts.
pub fn status(name: Option<&str>) -> Result<()> {
    let names = match name {
        Some(n) => {
            validate_context_name(n)?;
            if !context_exists(n)? {
                bail!("Context '{}' not found.", n);
            }
            vec![n.to_string()]
        }
        None => list_contexts()?,
    };
    if names.is_empty() {
        println!("No contexts found. Create one with 'gcpx login <name>'");
        return Ok(());
    }

    let mut needs_reauth = Vec::new();
    let mut needs_adc = Vec::new();
    for (i, res) in check_contexts(&names, Duration::from_secs(15))
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            println!();
        }
        match res {
            Ok(h) => {
                print_health(&h);
                if h.gcloud_stale() || h.config_mismatch() {
                    needs_reauth.push(h.name.clone());
                } else if h.adc_stale() || h.adc_mismatch() {
                    needs_adc.push(h.name.clone());
                }
            }
            Err(e) => println!("{}  ? {}", names[i], e),
        }
    }

    if !needs_reauth.is_empty() || !needs_adc.is_empty() {
        println!();
    }
    if !needs_reauth.is_empty() {
        println!("Fix gcloud:  gcpx reauth {}", needs_reauth.join(" "));
    }
    if !needs_adc.is_empty() {
        println!("Fix ADC:     gcpx reauth {} --adc", needs_adc.join(" "));
    }
    if !needs_reauth.is_empty() {
        bail!("{} context(s) need re-authentication", needs_reauth.len());
    }
    Ok(())
}
