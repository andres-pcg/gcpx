//! Credential health tests against a local fake token endpoint.
//!
//! The fake answers based on the refresh_token it receives:
//! - `fresh:<email>` → 200 with an id_token for <email>
//! - `stale`         → 400 invalid_grant / invalid_rapt
//! - `revoked`       → 400 invalid_grant

use std::env;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tempfile::TempDir;

use gcpx::auth::{CredState, adoptable_global_adc, check_adc_file};

mod common;
use common::start_fake_token_server;

static LOCK: Mutex<()> = Mutex::new(());

struct Env {
    _gcpx: TempDir,
    gcloud: TempDir,
}

impl Env {
    fn new() -> Self {
        let gcpx = TempDir::new().unwrap();
        let gcloud = TempDir::new().unwrap();
        let uri = start_fake_token_server();
        unsafe {
            env::set_var("GCPX_ALLOW_TEST_ENV", "1");
            env::set_var("GCPX_HOME", gcpx.path());
            env::set_var("GCPX_GCLOUD_DIR", gcloud.path());
            env::set_var("GCPX_TOKEN_URI", uri);
        }
        Env {
            _gcpx: gcpx,
            gcloud,
        }
    }

    fn global_adc(&self) -> std::path::PathBuf {
        self.gcloud
            .path()
            .join("application_default_credentials.json")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        unsafe {
            for k in [
                "GCPX_ALLOW_TEST_ENV",
                "GCPX_HOME",
                "GCPX_GCLOUD_DIR",
                "GCPX_TOKEN_URI",
            ] {
                env::remove_var(k);
            }
        }
    }
}

fn write_adc(path: &Path, refresh_token: &str) {
    fs::write(
        path,
        format!(
            r#"{{"type":"authorized_user","client_id":"c","client_secret":"s","refresh_token":"{}"}}"#,
            refresh_token
        ),
    )
    .unwrap();
}

#[test]
fn check_adc_classifies_states() {
    let _g = LOCK.lock().unwrap();
    let env = Env::new();
    let p = env.gcloud.path().join("a.json");

    write_adc(&p, "fresh:me@x.org");
    let c = check_adc_file(&p);
    assert_eq!(c.state, CredState::Valid);
    assert_eq!(c.email.as_deref(), Some("me@x.org"));

    write_adc(&p, "stale");
    assert_eq!(check_adc_file(&p).state, CredState::ReauthRequired);

    write_adc(&p, "revoked");
    assert_eq!(check_adc_file(&p).state, CredState::Revoked);

    assert_eq!(
        check_adc_file(&env.gcloud.path().join("nope.json")).state,
        CredState::Missing
    );
}

#[test]
fn adopts_only_working_adc_for_same_account() {
    let _g = LOCK.lock().unwrap();
    let env = Env::new();
    let ctx_adc = env.gcloud.path().join("ctx.json");
    write_adc(&ctx_adc, "stale");

    // Same account, working → adopted (case-insensitive match).
    write_adc(&env.global_adc(), "fresh:me@x.org");
    assert!(
        adoptable_global_adc("Me@X.org", &ctx_adc)
            .unwrap()
            .is_some()
    );

    // Different account, working → never adopted.
    write_adc(&env.global_adc(), "fresh:someone-else@x.org");
    assert!(
        adoptable_global_adc("me@x.org", &ctx_adc)
            .unwrap()
            .is_none()
    );

    // Same account but stale → not adopted.
    write_adc(&env.global_adc(), "stale");
    assert!(
        adoptable_global_adc("me@x.org", &ctx_adc)
            .unwrap()
            .is_none()
    );

    // No global ADC → nothing to adopt.
    fs::remove_file(env.global_adc()).unwrap();
    assert!(
        adoptable_global_adc("me@x.org", &ctx_adc)
            .unwrap()
            .is_none()
    );
}

#[test]
fn does_not_adopt_identical_copy() {
    let _g = LOCK.lock().unwrap();
    let env = Env::new();
    let ctx_adc = env.gcloud.path().join("ctx.json");
    write_adc(&ctx_adc, "fresh:me@x.org");
    write_adc(&env.global_adc(), "fresh:me@x.org");
    assert!(
        adoptable_global_adc("me@x.org", &ctx_adc)
            .unwrap()
            .is_none()
    );
}
