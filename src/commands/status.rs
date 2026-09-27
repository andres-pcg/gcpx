//! Status command — show credential health for saved contexts.

use anyhow::{Result, bail};
use serde_json::{Value, json};
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

/// Machine-readable report for one context (`gcpx status --json`).
fn health_json(h: &ContextHealth) -> Value {
    let mut issues = Vec::new();
    let mut fix = Vec::new();
    if h.gcloud_stale() {
        issues.push("gcloud_reauth_required");
    }
    if h.config_mismatch() {
        issues.push("config_account_mismatch");
    }
    if h.gcloud_stale() || h.config_mismatch() {
        fix.push(format!("gcpx reauth {}", h.name));
    }
    if h.adc_stale() {
        issues.push("adc_stale");
    }
    if h.adc_mismatch() {
        issues.push("adc_account_mismatch");
    }
    if h.adc_stale() || h.adc_mismatch() {
        fix.push(format!("gcpx reauth {} --adc", h.name));
    }
    json!({
        "name": h.name,
        "account": h.expected_account,
        "gcloud_config": h.gcloud_config,
        "config_account": h.config_account,
        "gcloud": h.gcloud.as_ref().map(|s| json!({"state": s.code(), "detail": s.detail()})),
        "adc": {
            "state": h.adc.state.code(),
            "detail": h.adc.state.detail(),
            "account": h.adc.email,
            "fresher_available": h.fresher_adc_available,
        },
        "issues": issues,
        "fix": fix,
    })
}

/// `gcpx status [name] [--json]`. Returns `true` when any checked context has
/// a stale gcloud credential or an account mismatch (the CLI then exits with
/// code 3, so scripts and agents can tell "needs reauth" from errors).
pub fn status(name: Option<&str>, as_json: bool) -> Result<bool> {
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
        if as_json {
            println!(
                "{}",
                json!({"contexts": [], "needs_reauth": [], "adc_stale": []})
            );
        } else {
            println!("No contexts found. Create one with 'gcpx login'");
        }
        return Ok(false);
    }

    let mut needs_reauth = Vec::new();
    let mut needs_adc = Vec::new();
    let mut reports = Vec::new();
    for (i, res) in check_contexts(&names, Duration::from_secs(15))
        .into_iter()
        .enumerate()
    {
        if !as_json && i > 0 {
            println!();
        }
        match res {
            Ok(h) => {
                if as_json {
                    reports.push(health_json(&h));
                } else {
                    print_health(&h);
                }
                if h.gcloud_stale() || h.config_mismatch() {
                    needs_reauth.push(h.name.clone());
                }
                if h.adc_stale() || h.adc_mismatch() {
                    needs_adc.push(h.name.clone());
                }
            }
            Err(e) => {
                if as_json {
                    reports.push(json!({"name": names[i], "error": e.to_string()}));
                } else {
                    println!("{}  ? {}", names[i], e);
                }
            }
        }
    }

    if as_json {
        let out = json!({
            "contexts": reports,
            "needs_reauth": needs_reauth,
            "adc_stale": needs_adc,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(!needs_reauth.is_empty());
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
    Ok(!needs_reauth.is_empty())
}
