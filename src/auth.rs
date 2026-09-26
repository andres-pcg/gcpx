//! Credential health checks.
//!
//! Google Cloud session control (16-hour default since 2026) makes stored
//! refresh tokens stop working until the user re-authenticates. The token
//! endpoint answers `invalid_grant` with subtype `invalid_rapt` /
//! `rapt_required` in that case, which is different from a revoked token
//! (plain `invalid_grant`). This module tells those apart for:
//!
//! - a context's stored ADC file (refreshed directly against the token
//!   endpoint — nothing is printed or persisted), and
//! - gcloud's own credential for an account (via `gcloud auth
//!   print-access-token`, run non-interactively).
//!
//! No token or secret ever leaves this module; callers only see a
//! [`CredState`] and, for ADC, the account email the token belongs to.

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;
use std::env;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{get_adc_path, get_context_adc_path, get_gcloud_dir, load_context_metadata};

const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// Health of one credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredState {
    /// Refreshes fine.
    Valid,
    /// Google requires re-authentication (session control / RAPT).
    ReauthRequired,
    /// Refresh token is revoked or otherwise unusable; needs a fresh login.
    Revoked,
    /// No credential stored.
    Missing,
    /// Credential type gcpx can't check (service account, external account…).
    NotChecked(String),
    /// Check itself failed (network, timeout, unexpected response).
    Unknown(String),
}

impl CredState {
    /// True when the credential needs a login to work again.
    pub fn is_stale(&self) -> bool {
        matches!(
            self,
            CredState::ReauthRequired | CredState::Revoked | CredState::Missing
        )
    }

    pub fn label(&self) -> String {
        match self {
            CredState::Valid => "ok".into(),
            CredState::ReauthRequired => "reauth required".into(),
            CredState::Revoked => "revoked (login required)".into(),
            CredState::Missing => "missing".into(),
            CredState::NotChecked(kind) => format!("not checked ({})", kind),
            CredState::Unknown(why) => format!("unknown ({})", why),
        }
    }
}

/// Result of checking an ADC file.
#[derive(Debug, Clone)]
pub struct AdcCheck {
    pub state: CredState,
    /// Account the credential belongs to, when Google told us (id_token).
    pub email: Option<String>,
}

#[derive(Deserialize)]
struct AuthorizedUser {
    #[serde(rename = "type")]
    kind: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
    token_uri: Option<String>,
}

#[derive(Deserialize, Default)]
struct TokenResponse {
    error: Option<String>,
    error_subtype: Option<String>,
    id_token: Option<String>,
}

fn token_uri(from_file: Option<&str>) -> String {
    // Test-only override, gated like GCPX_HOME (see config::test_env_allowed).
    if env::var("GCPX_ALLOW_TEST_ENV").as_deref() == Ok("1") {
        if let Ok(u) = env::var("GCPX_TOKEN_URI") {
            return u;
        }
    }
    match from_file {
        // Only honor Google-owned token endpoints from the file, so a
        // tampered ADC can't make us post the refresh token elsewhere.
        Some(u) if u.starts_with("https://oauth2.googleapis.com/") => u.to_string(),
        _ => DEFAULT_TOKEN_URI.to_string(),
    }
}

/// Classifies a token-endpoint response. Pure; unit-tested.
pub fn classify_token_response(status: u16, body: &str) -> (CredState, Option<String>) {
    let parsed: TokenResponse = serde_json::from_str(body).unwrap_or_default();
    if (200..300).contains(&status) {
        let email = parsed.id_token.as_deref().and_then(email_from_id_token);
        return (CredState::Valid, email);
    }
    let state = match (parsed.error.as_deref(), parsed.error_subtype.as_deref()) {
        (Some("invalid_grant"), Some("invalid_rapt" | "rapt_required")) => {
            CredState::ReauthRequired
        }
        (Some("invalid_grant"), _) => CredState::Revoked,
        (Some("invalid_client" | "unauthorized_client"), _) => CredState::Revoked,
        (Some(e), _) => CredState::Unknown(e.to_string()),
        (None, _) => CredState::Unknown(format!("HTTP {}", status)),
    };
    (state, None)
}

/// Extracts the `email` claim from a JWT without verifying it. Only used on
/// id_tokens we just received from Google over TLS, for display/matching.
pub fn email_from_id_token(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("email")?.as_str().map(|s| s.to_string())
}

/// Checks an ADC file by attempting a refresh. The access token is discarded.
pub fn check_adc_file(path: &Path) -> AdcCheck {
    let content = match fs::read(path) {
        Ok(c) => c,
        Err(_) => {
            return AdcCheck {
                state: CredState::Missing,
                email: None,
            };
        }
    };
    check_adc_bytes(&content)
}

/// Same as [`check_adc_file`] for in-memory ADC JSON.
pub fn check_adc_bytes(content: &[u8]) -> AdcCheck {
    let unknown = |why: &str| AdcCheck {
        state: CredState::Unknown(why.to_string()),
        email: None,
    };
    let creds: AuthorizedUser = match serde_json::from_slice(content) {
        Ok(c) => c,
        Err(_) => return unknown("unreadable ADC file"),
    };
    let kind = creds.kind.clone().unwrap_or_default();
    if kind != "authorized_user" {
        return AdcCheck {
            state: CredState::NotChecked(if kind.is_empty() {
                "unknown type".into()
            } else {
                kind
            }),
            email: None,
        };
    }
    let (Some(client_id), Some(client_secret), Some(refresh_token)) =
        (creds.client_id, creds.client_secret, creds.refresh_token)
    else {
        return unknown("incomplete ADC file");
    };

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into();
    let uri = token_uri(creds.token_uri.as_deref());
    let resp = agent.post(&uri).send_form([
        ("grant_type", "refresh_token"),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("refresh_token", refresh_token.as_str()),
    ]);
    match resp {
        Ok(mut r) => {
            let status = r.status().as_u16();
            let body = r.body_mut().read_to_string().unwrap_or_default();
            let (state, email) = classify_token_response(status, &body);
            AdcCheck { state, email }
        }
        Err(e) => unknown(&format!("network: {}", e)),
    }
}

/// Classifies `gcloud auth print-access-token` stderr. Pure; unit-tested.
pub fn classify_gcloud_error(stderr: &str) -> CredState {
    let s = stderr.to_ascii_lowercase();
    if s.contains("reauthentication") || s.contains("invalid_rapt") || s.contains("rapt_required") {
        CredState::ReauthRequired
    } else if s.contains("invalid_grant") {
        CredState::Revoked
    } else if s.contains("no credentialed accounts")
        || s.contains("not found in the credential store")
        || s.contains("you do not currently have an active account")
        || s.contains("could not find credentials")
    {
        CredState::Missing
    } else {
        let first = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("gcloud failed")
            .trim();
        CredState::Unknown(first.chars().take(120).collect())
    }
}

/// Checks gcloud's stored credential for `account`, non-interactively, with a
/// timeout. Never prompts (stdin is closed).
pub fn check_gcloud_account(account: &str, timeout: Duration) -> CredState {
    let child = Command::new("gcloud")
        .args(["auth", "print-access-token", "--account", account])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(_) => return CredState::Unknown("gcloud not found".into()),
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                return if status.success() {
                    CredState::Valid
                } else {
                    classify_gcloud_error(&err)
                };
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(40)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return CredState::Unknown("timed out".into());
            }
            Err(e) => return CredState::Unknown(e.to_string()),
        }
    }
}

/// Reads `core/account` from a gcloud configuration file without spawning
/// gcloud (fast, and works even when gcloud's credentials are broken).
pub fn read_config_account(gcloud_config: &str) -> Option<String> {
    let path = get_gcloud_dir()
        .ok()?
        .join("configurations")
        .join(format!("config_{}", gcloud_config));
    let content = fs::read_to_string(path).ok()?;
    parse_ini_value(&content, "core", "account")
}

/// Minimal INI lookup for gcloud config files. Pure; unit-tested.
pub fn parse_ini_value(content: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_section = &line[1..line.len() - 1] == section;
        } else if in_section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    let v = v.trim();
                    return (!v.is_empty()).then(|| v.to_string());
                }
            }
        }
    }
    None
}

/// Case-insensitive email equality.
pub fn same_account(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Full health report for one context.
#[derive(Debug, Clone)]
pub struct ContextHealth {
    pub name: String,
    pub gcloud_config: String,
    /// Account the context was saved for (metadata.json) — source of truth.
    pub expected_account: Option<String>,
    /// Account the gcloud configuration currently points at.
    pub config_account: Option<String>,
    /// gcloud credential health for `expected_account` (None if unknown account).
    pub gcloud: Option<CredState>,
    /// The context's stored ADC.
    pub adc: AdcCheck,
    /// A working ADC for the *same* account found at gcloud's well-known path
    /// that could replace a stale stored copy.
    pub fresher_adc_available: bool,
}

impl ContextHealth {
    /// gcloud configuration points at a different account than the context.
    pub fn config_mismatch(&self) -> bool {
        match (&self.expected_account, &self.config_account) {
            (Some(e), Some(c)) => !same_account(e, c),
            _ => false,
        }
    }

    /// Stored ADC belongs to a different account than the context.
    pub fn adc_mismatch(&self) -> bool {
        match (&self.expected_account, &self.adc.email) {
            (Some(e), Some(a)) => !same_account(e, a),
            _ => false,
        }
    }

    pub fn gcloud_stale(&self) -> bool {
        self.gcloud.as_ref().is_some_and(|s| s.is_stale())
    }

    pub fn adc_stale(&self) -> bool {
        self.adc.state.is_stale()
    }
}

/// Returns the global ADC bytes if they are a *working* credential for
/// exactly `expected_account` and differ from the context's stored copy.
///
/// This is the only way gcpx adopts a credential it didn't create itself, so
/// it is deliberately strict: the account must be confirmed by Google (email
/// in the id_token) and match the context's saved account. An unknown or
/// different account is never adopted.
pub fn adoptable_global_adc(expected_account: &str, context_adc: &Path) -> Result<Option<Vec<u8>>> {
    let global = get_adc_path()?;
    let Ok(bytes) = fs::read(&global) else {
        return Ok(None);
    };
    if fs::read(context_adc).ok().as_deref() == Some(bytes.as_slice()) {
        return Ok(None);
    }
    let check = check_adc_bytes(&bytes);
    let matches = check
        .email
        .as_deref()
        .is_some_and(|e| same_account(e, expected_account));
    Ok((check.state == CredState::Valid && matches).then_some(bytes))
}

/// Checks one context. Makes network calls (ADC) and spawns gcloud.
pub fn check_context(name: &str, gcloud_timeout: Duration) -> Result<ContextHealth> {
    let metadata = load_context_metadata(name)?;
    let gcloud_config = metadata
        .as_ref()
        .map(|m| m.gcloud_config.clone())
        .unwrap_or_else(|| name.to_string());
    let expected_account = metadata.as_ref().and_then(|m| m.account.clone());
    let config_account = read_config_account(&gcloud_config);
    let adc_path = get_context_adc_path(name)?;

    // gcloud and ADC checks are independent — run them concurrently.
    let probe_account = expected_account.clone().or_else(|| config_account.clone());
    let gcloud_handle =
        probe_account.map(|acc| thread::spawn(move || check_gcloud_account(&acc, gcloud_timeout)));
    let adc = check_adc_file(&adc_path);

    let fresher_adc_available = match (&expected_account, adc.state.is_stale()) {
        (Some(acc), true) => adoptable_global_adc(acc, &adc_path)?.is_some(),
        _ => false,
    };
    let gcloud = gcloud_handle.map(|h| {
        h.join()
            .unwrap_or_else(|_| CredState::Unknown("check panicked".into()))
    });

    Ok(ContextHealth {
        name: name.to_string(),
        gcloud_config,
        expected_account,
        config_account,
        gcloud,
        adc,
        fresher_adc_available,
    })
}

/// Checks several contexts concurrently, preserving input order.
pub fn check_contexts(names: &[String], gcloud_timeout: Duration) -> Vec<Result<ContextHealth>> {
    let handles: Vec<_> = names
        .iter()
        .map(|n| {
            let n = n.clone();
            thread::spawn(move || check_context(&n, gcloud_timeout))
        })
        .collect();
    handles
        .into_iter()
        .map(|h| {
            h.join()
                .map_err(|_| anyhow::anyhow!("check panicked"))
                .and_then(|r| r)
        })
        .collect()
}

/// Best-effort staleness warning for explicit `gcpx use` / `gcpx switch`.
///
/// Prints to stderr only when something is wrong; silent otherwise. Skipped
/// when `GCPX_NO_AUTH_CHECK=1`. Never fails the caller.
pub fn warn_if_stale(name: &str) {
    if env::var("GCPX_NO_AUTH_CHECK").as_deref() == Ok("1") {
        return;
    }
    let Ok(h) = check_context(name, Duration::from_secs(3)) else {
        return;
    };
    let who = h.expected_account.as_deref().unwrap_or("this account");
    if h.config_mismatch() {
        eprintln!(
            "gcpx: warning: gcloud config '{}' is set to {}, but context '{}' was saved for {}. Run `gcpx reauth {}` to fix.",
            h.gcloud_config,
            h.config_account.as_deref().unwrap_or("?"),
            name,
            who,
            name
        );
    }
    if h.gcloud_stale() {
        eprintln!(
            "gcpx: gcloud session for {} has expired — run `gcpx reauth {}`",
            who, name
        );
    }
    if h.adc_stale() {
        eprintln!(
            "gcpx: ADC for '{}' is stale (only needed for Terraform/client libraries) — `gcpx reauth {} --adc`",
            name, name
        );
    }
    if h.adc_mismatch() {
        eprintln!(
            "gcpx: warning: ADC stored for '{}' belongs to {}, not {}",
            name,
            h.adc.email.as_deref().unwrap_or("?"),
            who
        );
    }
}

/// Reads a file if present.
pub(crate) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {:?}", path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt_with(payload: &str) -> String {
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.sig", enc.encode("{}"), enc.encode(payload))
    }

    #[test]
    fn classifies_reauth_subtypes() {
        let body = r#"{"error":"invalid_grant","error_subtype":"invalid_rapt"}"#;
        assert_eq!(
            classify_token_response(400, body).0,
            CredState::ReauthRequired
        );
        let body = r#"{"error":"invalid_grant","error_subtype":"rapt_required"}"#;
        assert_eq!(
            classify_token_response(400, body).0,
            CredState::ReauthRequired
        );
    }

    #[test]
    fn classifies_revoked() {
        let body = r#"{"error":"invalid_grant","error_description":"Bad Request"}"#;
        assert_eq!(classify_token_response(400, body).0, CredState::Revoked);
    }

    #[test]
    fn classifies_valid_with_email() {
        let body = format!(
            r#"{{"access_token":"x","id_token":"{}"}}"#,
            jwt_with(r#"{"email":"a@b.org"}"#)
        );
        let (state, email) = classify_token_response(200, &body);
        assert_eq!(state, CredState::Valid);
        assert_eq!(email.as_deref(), Some("a@b.org"));
    }

    #[test]
    fn classifies_garbage_as_unknown() {
        assert!(matches!(
            classify_token_response(500, "oops").0,
            CredState::Unknown(_)
        ));
    }

    #[test]
    fn gcloud_error_classification() {
        let reauth = "ERROR: (gcloud.auth.print-access-token) There was a problem refreshing your current auth tokens: Reauthentication failed. cannot prompt during non-interactive execution.";
        assert_eq!(classify_gcloud_error(reauth), CredState::ReauthRequired);
        let revoked = "ERROR: ... ('invalid_grant: Bad Request', {'error': 'invalid_grant'})";
        assert_eq!(classify_gcloud_error(revoked), CredState::Revoked);
        assert!(matches!(
            classify_gcloud_error("ERROR: something else"),
            CredState::Unknown(_)
        ));
    }

    #[test]
    fn ini_parsing() {
        let cfg = "[core]\naccount = me@x.org\nproject = p\n\n[compute]\naccount = nope\n";
        assert_eq!(
            parse_ini_value(cfg, "core", "account").as_deref(),
            Some("me@x.org")
        );
        assert_eq!(parse_ini_value(cfg, "core", "zone"), None);
        assert_eq!(
            parse_ini_value("[other]\naccount = x", "core", "account"),
            None
        );
    }

    #[test]
    fn non_user_adc_is_not_checked() {
        let sa = br#"{"type":"service_account","client_email":"x"}"#;
        assert!(matches!(
            check_adc_bytes(sa).state,
            CredState::NotChecked(_)
        ));
    }

    #[test]
    fn token_uri_ignores_foreign_hosts() {
        assert_eq!(
            token_uri(Some("https://evil.example/token")),
            DEFAULT_TOKEN_URI
        );
    }
}
