use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sw_crypto::KEYCHAIN_RETRY_AFTER;

use super::{RelayChange, RelaySettingsStore, TokenUnavailable, PAIR_AGAIN};
use crate::sealed_secrets::test_doubles::{
    capture_log_lines, captured_log_lines, files_mention, scratch_dir, unique_token, CountingVault,
    ManualClock,
};
use crate::sealed_secrets::KEYCHAIN_REFUSED;

const RELAY_URL: &str = "https://assistant.example.ts.net";

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ReleasedRelaySettings {
    relay_url: String,
    relay_token: String,
    #[serde(default)]
    paired_verified: bool,
}

impl ReleasedRelaySettings {
    fn has_relay(&self) -> bool {
        !self.relay_url.trim().is_empty() && !self.relay_token.trim().is_empty()
    }
}

fn relay_file(dir: &Path) -> PathBuf {
    dir.join("relay.json")
}

fn pair(store: &RelaySettingsStore, token: &str) {
    store
        .update(RelayChange {
            relay_url: Some(RELAY_URL.to_string()),
            relay_token: Some(token.to_string()),
            paired_verified: Some(true),
        })
        .unwrap();
}

fn write_as_an_older_build(dir: &Path, token: &str) {
    let older = ReleasedRelaySettings {
        relay_url: RELAY_URL.to_string(),
        relay_token: token.to_string(),
        paired_verified: true,
    };
    fs::write(
        relay_file(dir),
        serde_json::to_string_pretty(&older).unwrap(),
    )
    .unwrap();
}

#[test]
fn pairing_stores_no_plaintext_relay_token_and_never_hands_it_to_the_webview() {
    let dir = scratch_dir("relay-pair");
    let token = unique_token("relay");
    let vault = Arc::new(CountingVault::default());
    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();

    pair(&store, &token);

    assert!(!files_mention(&dir, &token));
    assert!(vault.holds_key());
    let status = store.snapshot();
    assert!(status.ready());
    assert!(!serde_json::to_string(&status).unwrap().contains(&token));
    let credentials = store.credentials().unwrap();
    assert_eq!(credentials.relay_token, token);
    assert!(!format!("{credentials:?}").contains(&token));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_existing_relay_json_token_migrates_on_first_use_and_is_removed_from_the_file() {
    let dir = scratch_dir("relay-migrate");
    let token = unique_token("relay");
    fs::write(
        relay_file(&dir),
        format!(r#"{{"relayUrl":"{RELAY_URL}","relayToken":"{token}"}}"#),
    )
    .unwrap();
    let vault = Arc::new(CountingVault::default());
    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();
    assert!(store.snapshot().has_relay());

    assert_eq!(store.credentials().unwrap().relay_token, token);

    assert!(!files_mention(&dir, &token));
    let raw = fs::read_to_string(relay_file(&dir)).unwrap();
    assert!(raw.contains("sealedRelayToken"));
    let relaunched = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();
    assert!(relaunched.snapshot().has_relay());
    assert_eq!(relaunched.credentials().unwrap().relay_token, token);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn startup_reads_paired_state_without_touching_the_secret_store() {
    let dir = scratch_dir("relay-startup");
    let vault = Arc::new(CountingVault::default());
    pair(
        &RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap(),
        "token",
    );
    let calls_before_launch = vault.calls();

    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();
    let status = store.snapshot();

    assert!(status.has_relay());
    assert!(status.ready());
    assert_eq!(status.relay_url, RELAY_URL);
    assert_eq!(vault.calls(), calls_before_launch);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_first_send_decrypts_lazily_and_later_sends_reuse_the_key() {
    let dir = scratch_dir("relay-lazy");
    let token = unique_token("relay");
    let vault = Arc::new(CountingVault::default());
    pair(
        &RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap(),
        &token,
    );
    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();
    let calls_before_send = vault.calls();

    assert_eq!(store.credentials().unwrap().relay_token, token);
    assert_eq!(vault.calls(), calls_before_send + 1);
    assert_eq!(store.credentials().unwrap().relay_token, token);
    assert_eq!(vault.calls(), calls_before_send + 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn unpairing_removes_the_secret() {
    let dir = scratch_dir("relay-unpair");
    let vault = Arc::new(CountingVault::default());
    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();
    pair(&store, "token");

    let status = store
        .update(RelayChange {
            relay_url: Some(String::new()),
            relay_token: Some(String::new()),
            paired_verified: Some(false),
        })
        .unwrap();

    assert!(!status.has_relay());
    let raw = fs::read_to_string(relay_file(&dir)).unwrap();
    assert!(!raw.contains("sealedRelayToken"));
    assert_eq!(store.credentials().unwrap().relay_token, "");
    let relaunched = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();
    assert!(!relaunched.snapshot().has_relay());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_failed_store_keeps_the_plaintext_and_logs_no_secret() {
    capture_log_lines();
    let dir = scratch_dir("relay-refused");
    let token = unique_token("relay");
    write_as_an_older_build(&dir, &token);
    let vault = Arc::new(CountingVault::refusing_to_store());
    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();

    assert_eq!(store.credentials().unwrap().relay_token, token);

    assert!(files_mention(&dir, &token));
    assert!(store.snapshot().ready());
    let lines = captured_log_lines();
    assert!(lines
        .iter()
        .any(|line| line.contains("kept the assistant token unencrypted")));
    assert!(lines.iter().all(|line| !line.contains(&token)));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_refused_keychain_fails_pairing_without_writing_the_token() {
    let dir = scratch_dir("relay-refused-pair");
    let token = unique_token("relay");
    let store = RelaySettingsStore::open_at(
        relay_file(&dir),
        Arc::new(CountingVault::refusing_to_store()).sealer(),
    )
    .unwrap();

    let refused = store
        .update(RelayChange {
            relay_url: Some(RELAY_URL.to_string()),
            relay_token: Some(token.clone()),
            paired_verified: Some(false),
        })
        .unwrap_err();

    assert_eq!(refused.to_string(), KEYCHAIN_REFUSED);
    assert!(!relay_file(&dir).exists());
    assert!(!store.snapshot().has_relay());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_older_build_still_parses_the_new_relay_json_and_shows_as_not_paired() {
    let dir = scratch_dir("relay-older-build");
    let token = unique_token("relay");
    let store = RelaySettingsStore::open_at(
        relay_file(&dir),
        Arc::new(CountingVault::default()).sealer(),
    )
    .unwrap();
    pair(&store, &token);

    let raw = fs::read_to_string(relay_file(&dir)).unwrap();
    let older: ReleasedRelaySettings = serde_json::from_str(&raw).unwrap();

    assert_eq!(older.relay_url, RELAY_URL);
    assert_eq!(older.relay_token, "");
    assert!(!older.has_relay());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_token_an_older_build_saved_after_a_downgrade_wins_and_is_sealed_again() {
    let dir = scratch_dir("relay-downgrade");
    let vault = Arc::new(CountingVault::default());
    pair(
        &RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap(),
        "token-before",
    );
    let repaired = unique_token("relay");
    write_as_an_older_build(&dir, &repaired);

    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();

    assert_eq!(store.credentials().unwrap().relay_token, repaired);
    assert!(!files_mention(&dir, &repaired));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_sealed_token_whose_key_is_gone_asks_to_pair_again() {
    let dir = scratch_dir("relay-key-gone");
    let vault = Arc::new(CountingVault::default());
    pair(
        &RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap(),
        "token",
    );
    vault.forget_key();

    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap();

    let refused = store.credentials().unwrap_err();
    assert_eq!(refused, TokenUnavailable::PairAgain);
    assert_eq!(refused.to_string(), PAIR_AGAIN);
    assert!(relay_file(&dir).exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_locked_keychain_fails_the_send_with_the_keychain_message_and_never_reads_as_unpaired() {
    let dir = scratch_dir("relay-locked");
    let token = unique_token("relay");
    let vault = Arc::new(CountingVault::default());
    pair(
        &RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap(),
        &token,
    );
    let clock = ManualClock::start();
    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer_on(&clock)).unwrap();
    vault.refuse_access(true);

    let refused = store.credentials().unwrap_err();

    assert_eq!(refused, TokenUnavailable::KeychainRefused);
    assert_eq!(refused.to_string(), KEYCHAIN_REFUSED);
    assert!(store.snapshot().has_relay());
    assert!(store.snapshot().paired_verified);
    let raw = fs::read_to_string(relay_file(&dir)).unwrap();
    assert!(raw.contains("sealedRelayToken"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_send_after_a_refusal_waits_out_the_cool_down_and_then_unlocks_the_token() {
    let dir = scratch_dir("relay-retry");
    let token = unique_token("relay");
    let vault = Arc::new(CountingVault::default());
    pair(
        &RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap(),
        &token,
    );
    let clock = ManualClock::start();
    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer_on(&clock)).unwrap();
    vault.refuse_access(true);
    assert!(store.credentials().unwrap_err().is_transient());

    vault.refuse_access(false);
    let calls_after_refusal = vault.calls();
    assert!(store.credentials().unwrap_err().is_transient());
    assert_eq!(vault.calls(), calls_after_refusal);

    clock.advance(KEYCHAIN_RETRY_AFTER);

    assert_eq!(store.credentials().unwrap().relay_token, token);
    assert_eq!(vault.calls(), calls_after_refusal + 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn pairing_right_after_a_refused_prompt_asks_the_keychain_again() {
    let dir = scratch_dir("relay-repair");
    let vault = Arc::new(CountingVault::default());
    pair(
        &RelaySettingsStore::open_at(relay_file(&dir), vault.sealer()).unwrap(),
        "old-token",
    );
    let clock = ManualClock::start();
    let store = RelaySettingsStore::open_at(relay_file(&dir), vault.sealer_on(&clock)).unwrap();
    vault.refuse_access(true);
    assert!(store.credentials().unwrap_err().is_transient());

    vault.refuse_access(false);
    let calls_after_refusal = vault.calls();
    let token = unique_token("relay");
    pair(&store, &token);

    assert_eq!(vault.calls(), calls_after_refusal + 1);
    assert_eq!(store.credentials().unwrap().relay_token, token);
    let _ = fs::remove_dir_all(dir);
}
