use super::{normalize_relay_url, unique_tmp_path, RelayChange, RelaySettingsStore, RelayStatus};
use crate::sealed_secrets::test_doubles::CountingVault;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

struct TempTestDir(PathBuf);

impl TempTestDir {
    fn create(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "sw-free-relay-settings-{label}-{}-{}",
            std::process::id(),
            {
                let mut suffix = [0u8; 8];
                getrandom::fill(&mut suffix).expect("OS RNG failed");
                suffix
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            }
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn leftover_tmp_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().contains(".json.tmp."))
        .collect()
}

#[test]
fn unique_tmp_path_never_returns_the_same_path_twice() {
    let base = PathBuf::from("/does/not/matter/relay.json");
    let mut seen = HashSet::new();
    for _ in 0..64 {
        assert!(seen.insert(unique_tmp_path(&base)));
    }
}

#[test]
fn concurrent_updates_to_the_same_store_all_succeed_with_no_corruption_or_leftovers() {
    let root = TempTestDir::create("concurrency");
    let path = root.0.join("relay.json");
    let vault = Arc::new(CountingVault::default());
    let store = Arc::new(RelaySettingsStore::open_at(path.clone(), vault.sealer()).unwrap());
    let writer_count = 16;

    let handles: Vec<_> = (0..writer_count)
        .map(|i| {
            let store = store.clone();
            thread::spawn(move || {
                let token = format!("token-{i:02}");
                store
                    .update(RelayChange {
                        relay_token: Some(token.clone()),
                        ..RelayChange::default()
                    })
                    .map(|_| token)
            })
        })
        .collect();

    let mut candidates = HashSet::new();
    for handle in handles {
        let token = handle.join().unwrap().unwrap();
        candidates.insert(token);
    }

    let persisted = RelaySettingsStore::open_at(path, vault.sealer())
        .unwrap()
        .credentials()
        .unwrap();
    assert!(
        candidates.contains(&persisted.relay_token),
        "persisted relay_token must exactly match one writer's token, not a mix"
    );
    assert_eq!(
        persisted.relay_token,
        store.credentials().unwrap().relay_token
    );
    assert!(leftover_tmp_files(&root.0).is_empty());
}

#[test]
fn concurrent_updates_to_different_fields_leave_disk_state_matching_the_final_in_memory_snapshot() {
    let root = TempTestDir::create("cross-field");
    let path = root.0.join("relay.json");
    let vault = Arc::new(CountingVault::default());
    let store = Arc::new(RelaySettingsStore::open_at(path.clone(), vault.sealer()).unwrap());
    let writer_count = 24;

    let handles: Vec<_> = (0..writer_count)
        .map(|i| {
            let store = store.clone();
            thread::spawn(move || {
                store.update(match i % 3 {
                    0 => RelayChange {
                        relay_url: Some(format!("http://host-{i}.example")),
                        ..RelayChange::default()
                    },
                    1 => RelayChange {
                        relay_token: Some(format!("token-{i:02}")),
                        ..RelayChange::default()
                    },
                    _ => RelayChange {
                        paired_verified: Some(i % 2 == 0),
                        ..RelayChange::default()
                    },
                })
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap().unwrap();
    }

    let expected = store.credentials().unwrap();
    let persisted = RelaySettingsStore::open_at(path, vault.sealer())
        .unwrap()
        .credentials()
        .unwrap();

    assert_eq!(
        persisted.relay_url, expected.relay_url,
        "relay_url on disk must match the final in-memory value, not a stale concurrent snapshot"
    );
    assert_eq!(
        persisted.relay_token, expected.relay_token,
        "relay_token on disk must match the final in-memory value, not a stale concurrent snapshot"
    );
    assert_eq!(
        persisted.paired_verified, expected.paired_verified,
        "paired_verified on disk must match the final in-memory value, not a stale concurrent snapshot"
    );
    assert!(leftover_tmp_files(&root.0).is_empty());
}

#[test]
fn pairing_pending_requires_relay_and_blocks_until_verified() {
    let mut s = RelayStatus::default();
    assert!(!s.pairing_pending(), "no relay configured is not 'pending'");

    s.relay_url = "http://127.0.0.1:8765".to_string();
    s.has_relay_token = true;
    assert!(
        s.pairing_pending(),
        "configured relay without verified pairing must be pending"
    );
    assert!(!s.ready(), "pending pairing is not ready to record");

    s.paired_verified = true;
    assert!(
        !s.pairing_pending(),
        "verified pairing clears pending state"
    );
    assert!(s.ready(), "configured and verified relay is ready");
}

#[test]
fn leaves_clean_base() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_trailing_slash() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_v1_incoming() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/v1/incoming"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_v1_ping() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/v1/ping"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_legacy_assistant_relay() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/api/v1/assistant-relay"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_legacy_assistant_relay_tasks() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/api/v1/assistant-relay/tasks"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_legacy_assistant_relay_ping() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/api/v1/assistant-relay/ping"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_trailing_tasks() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/tasks"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_trailing_incoming() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/incoming"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn strips_trailing_ping() {
    assert_eq!(
        normalize_relay_url("http://127.0.0.1:8765/ping"),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn trims_whitespace() {
    assert_eq!(
        normalize_relay_url("  http://127.0.0.1:8765  "),
        "http://127.0.0.1:8765"
    );
}

#[test]
fn trims_whitespace_and_strips_path() {
    assert_eq!(
        normalize_relay_url("  http://127.0.0.1:8765/v1/incoming/  "),
        "http://127.0.0.1:8765"
    );
}
