//! End-to-end tests of the `gcpx` binary without a terminal (as scripts and
//! AI agents run it), against a stand-in `gcloud` on PATH and a local fake
//! token endpoint. Everything lives in temp dirs; each test passes its own
//! environment to the child process, so tests don't share global state.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

mod common;

/// Stand-in for gcloud: just enough of `auth login` / `auth print-access-token`
/// for gcpx's reauth flows. State lives in $FAKE_GCLOUD_DIR.
///
/// Like the real gcloud, `--update-adc` with GOOGLE_APPLICATION_CREDENTIALS
/// set stops to ask "Do you want to continue (Y/n)?" — here it records that
/// and fails, so a test can assert gcpx never triggers it.
const FAKE_GCLOUD: &str = r#"#!/bin/bash
D="$FAKE_GCLOUD_DIR"
echo "gcloud $*" >> "$D/calls.log"
cfg="$D/configurations/config_${CLOUDSDK_ACTIVE_CONFIG_NAME:-default}"
set_account() {
  grep -v '^account = ' "$cfg" > "$cfg.tmp" 2>/dev/null; mv "$cfg.tmp" "$cfg"
  printf '[core]\naccount = %s\n' "$1" > "$cfg.new"; grep -v '^\[core\]' "$cfg" >> "$cfg.new"; mv "$cfg.new" "$cfg"
  grep -vx "$1" "$D/stale_accounts" > "$D/stale.tmp" 2>/dev/null; mv "$D/stale.tmp" "$D/stale_accounts"
}
write_adc() {
  printf '{"type":"authorized_user","client_id":"c","client_secret":"s","refresh_token":"fresh:%s"}' "$1" \
    > "$D/application_default_credentials.json"
}
case "$*" in *--update-adc*)
  if [ -n "$GOOGLE_APPLICATION_CREDENTIALS" ]; then
    echo "The environment variable [GOOGLE_APPLICATION_CREDENTIALS] is set to:" >&2
    echo "Do you want to continue (Y/n)?" >&2
    echo "GAC_PROMPT" >> "$D/calls.log"; exit 1
  fi ;;
esac
case "$*" in
  "auth print-access-token --account "*)
    if grep -qx "$4" "$D/stale_accounts" 2>/dev/null; then
      echo "ERROR: Reauthentication failed. cannot prompt during non-interactive execution." >&2; exit 1
    fi
    echo "ya29.fake" ;;
  "auth login "*" --no-launch-browser"*)
    printf 'Go to the following link in your browser:\n\n    https://accounts.google.com/o/oauth2/auth?client_id=fake\n\nEnter the verification code: ' >&2
    IFS= read -r code
    [ "$code" = "good-code" ] || { echo "ERROR: (gcloud.auth.login) (invalid_grant) Malformed auth code." >&2; exit 1; }
    set_account "$3"; case "$*" in *--update-adc*) write_adc "$3";; esac ;;
  "auth login "*)
    set_account "$3"; case "$*" in *--update-adc*) write_adc "$3";; esac ;;
  *) echo "fake gcloud: unhandled: $*" >&2; exit 2 ;;
esac
"#;

const GLOBAL_ADC: &[u8] = br#"{"type":"authorized_user","refresh_token":"global-original"}"#;

struct Env {
    _root: TempDir,
    bin: PathBuf,
    gcloud: PathBuf,
    gcpx: PathBuf,
    token_uri: String,
}

impl Env {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let bin = root.path().join("bin");
        let gcloud = root.path().join("gcloud");
        let gcpx = root.path().join("gcpx");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(gcloud.join("configurations")).unwrap();
        fs::create_dir_all(&gcpx).unwrap();
        let script = bin.join("gcloud");
        fs::write(&script, FAKE_GCLOUD).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            gcloud.join("application_default_credentials.json"),
            GLOBAL_ADC,
        )
        .unwrap();
        Env {
            token_uri: common::start_fake_token_server(),
            _root: root,
            bin,
            gcloud,
            gcpx,
        }
    }

    /// A saved context whose gcloud session and ADC are both expired.
    fn stale_context(&self, name: &str, account: &str) {
        let dir = self.gcpx.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("metadata.json"),
            format!(r#"{{"gcloud_config":"{name}","account":"{account}","project":"p"}}"#),
        )
        .unwrap();
        fs::write(
            dir.join("adc.json"),
            r#"{"type":"authorized_user","client_id":"c","client_secret":"s","refresh_token":"stale"}"#,
        )
        .unwrap();
        fs::write(
            self.gcloud
                .join("configurations")
                .join(format!("config_{name}")),
            format!("[core]\naccount = {account}\n"),
        )
        .unwrap();
        fs::write(self.gcloud.join("stale_accounts"), format!("{account}\n")).unwrap();
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_gcpx"));
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap());
        c.args(args)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self._root.path())
            .env("GCPX_ALLOW_TEST_ENV", "1")
            .env("GCPX_HOME", &self.gcpx)
            .env("GCPX_GCLOUD_DIR", &self.gcloud)
            .env("GCPX_TOKEN_URI", &self.token_uri)
            .env("GCPX_NO_AUTH_CHECK", "1")
            .env("FAKE_GCLOUD_DIR", &self.gcloud)
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str], extra_env: &[(&str, &Path)]) -> Output {
        let mut c = self.cmd(args);
        for (k, v) in extra_env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.gcloud.join("calls.log")).unwrap_or_default()
    }

    fn global_adc(&self) -> Vec<u8> {
        fs::read(self.gcloud.join("application_default_credentials.json")).unwrap()
    }

    fn context_refresh_token(&self, name: &str) -> String {
        let v: serde_json::Value =
            serde_json::from_slice(&fs::read(self.gcpx.join(name).join("adc.json")).unwrap())
                .unwrap();
        v["refresh_token"].as_str().unwrap().to_string()
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Regression: a shell on `gcpx use <ctx>` exports GOOGLE_APPLICATION_CREDENTIALS.
/// `gcpx reauth --adc` must not let gcloud see it, or gcloud stops to ask
/// whether to write ADC to its default location.
#[test]
fn reauth_adc_from_a_use_shell_does_not_prompt() {
    let env = Env::new();
    env.stale_context("work", "me@example.com");
    let use_adc = env.gcpx.join("work").join("adc.json");

    let out = env.run(
        &["reauth", "work", "--adc", "--json"],
        &[("GOOGLE_APPLICATION_CREDENTIALS", &use_adc)],
    );

    assert!(out.status.success(), "reauth failed: {}", stderr(&out));
    assert!(
        !env.calls().contains("GAC_PROMPT"),
        "gcloud saw GOOGLE_APPLICATION_CREDENTIALS"
    );
    assert_eq!(env.context_refresh_token("work"), "fresh:me@example.com");
    assert_eq!(env.global_adc(), GLOBAL_ADC, "global ADC not restored");
}

#[test]
fn two_step_adc_from_a_use_shell_does_not_prompt() {
    let env = Env::new();
    env.stale_context("work", "me@example.com");
    let use_adc = env.gcpx.join("work").join("adc.json");
    let gac = [("GOOGLE_APPLICATION_CREDENTIALS", use_adc.as_path())];

    let start = env.run(&["reauth", "work", "--start", "--adc", "--json"], &gac);
    assert!(start.status.success(), "start failed: {}", stderr(&start));
    let v: serde_json::Value = serde_json::from_slice(&start.stdout).unwrap();
    assert!(
        v["url"]
            .as_str()
            .unwrap()
            .starts_with("https://accounts.google.com/")
    );

    let done = env.run(&["reauth", "work", "--code", "good-code", "--json"], &gac);
    assert!(done.status.success(), "code failed: {}", stderr(&done));
    assert!(
        !env.calls().contains("GAC_PROMPT"),
        "gcloud saw GOOGLE_APPLICATION_CREDENTIALS"
    );
    assert_eq!(env.context_refresh_token("work"), "fresh:me@example.com");
    assert_eq!(env.global_adc(), GLOBAL_ADC, "global ADC not restored");
}

#[test]
fn two_step_wrong_code_reports_gcloud_error_and_cleans_up() {
    let env = Env::new();
    env.stale_context("work", "me@example.com");

    assert!(
        env.run(&["reauth", "work", "--start"], &[])
            .status
            .success()
    );
    let out = env.run(&["reauth", "work", "--code", "nope"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Malformed auth code"),
        "{}",
        stderr(&out)
    );
    assert!(!env.gcpx.join(".pending").join("work").exists());
}

#[test]
fn status_exits_3_when_reauth_needed() {
    let env = Env::new();
    env.stale_context("work", "me@example.com");
    let out = env.run(&["status", "--json"], &[]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["needs_reauth"][0], "work");
    assert_eq!(v["contexts"][0]["gcloud"]["state"], "reauth_required");
}

#[test]
fn run_propagates_exit_code_and_sets_context_env() {
    let env = Env::new();
    env.stale_context("work", "me@example.com");

    let out = env.run(&["run", "work", "--", "sh", "-c", "exit 7"], &[]);
    assert_eq!(out.status.code(), Some(7));

    let out = env.run(
        &[
            "run",
            "work",
            "--",
            "sh",
            "-c",
            "echo \"$GCPX_CONTEXT|$CLOUDSDK_ACTIVE_CONFIG_NAME\"",
        ],
        &[],
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "work|work");
}

#[test]
fn bare_gcpx_without_terminal_prints_help() {
    let env = Env::new();
    let out = env.run(&[], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("gcpx agents"));
}
