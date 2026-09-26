//! Credential health tests against a local fake token endpoint.
//!
//! The fake answers based on the refresh_token it receives:
//! - `fresh:<email>` → 200 with an id_token for <email>
//! - `stale`         → 400 invalid_grant / invalid_rapt
//! - `revoked`       → 400 invalid_grant

use base64::Engine;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use tempfile::TempDir;

use gcpx::auth::{CredState, adoptable_global_adc, check_adc_file};

static LOCK: Mutex<()> = Mutex::new(());

fn start_fake_token_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let l = line.trim_end().to_ascii_lowercase();
                if let Some(v) = l.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                if l.is_empty() {
                    break;
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let body = String::from_utf8_lossy(&body).to_string();
            let token = body
                .split('&')
                .find_map(|kv| kv.strip_prefix("refresh_token="))
                .unwrap_or("")
                .replace("%3A", ":")
                .replace("%40", "@");
            let (status, json) = if let Some(email) = token.strip_prefix("fresh:") {
                let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
                let jwt = format!(
                    "{}.{}.sig",
                    enc.encode("{}"),
                    enc.encode(format!(r#"{{"email":"{}"}}"#, email))
                );
                (
                    "200 OK",
                    format!(r#"{{"access_token":"x","id_token":"{}"}}"#, jwt),
                )
            } else if token == "stale" {
                (
                    "400 Bad Request",
                    r#"{"error":"invalid_grant","error_subtype":"invalid_rapt"}"#.to_string(),
                )
            } else {
                (
                    "400 Bad Request",
                    r#"{"error":"invalid_grant"}"#.to_string(),
                )
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                json.len(),
                json
            );
        }
    });
    format!("http://{}/token", addr)
}

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
