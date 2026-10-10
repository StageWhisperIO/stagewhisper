use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelaySettings {
    pub relay_url: String,
    pub relay_token: String,
    #[serde(default)]
    pub paired_verified: bool,
}

impl RelaySettings {
    pub fn has_relay(&self) -> bool {
        !self.relay_url.trim().is_empty() && !self.relay_token.trim().is_empty()
    }

    pub fn pairing_pending(&self) -> bool {
        self.has_relay() && !self.paired_verified
    }

    pub fn ready(&self) -> bool {
        self.has_relay() && self.paired_verified
    }
}

pub struct RelaySettingsStore {
    path: PathBuf,
    inner: Mutex<RelaySettings>,
}

impl RelaySettingsStore {
    pub fn load(app: &AppHandle) -> Result<Self> {
        let path = settings_file_path(app)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating relay settings dir {}", parent.display()))?;
        }
        let inner = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading relay settings at {}", path.display()))?;
            serde_json::from_str::<RelaySettings>(&raw).unwrap_or_default()
        } else {
            RelaySettings::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    pub fn snapshot(&self) -> RelaySettings {
        self.inner
            .lock()
            .expect("relay settings mutex poisoned")
            .clone()
    }

    pub fn update<F>(&self, mutate: F) -> Result<RelaySettings>
    where
        F: FnOnce(&mut RelaySettings),
    {
        let mut guard = self.inner.lock().expect("relay settings mutex poisoned");
        let mut candidate = guard.clone();
        mutate(&mut candidate);
        candidate.relay_url = normalize_relay_url(&candidate.relay_url);
        self.persist(&candidate)?;
        *guard = candidate.clone();
        Ok(candidate)
    }

    fn persist(&self, snapshot: &RelaySettings) -> Result<()> {
        let raw = serde_json::to_string_pretty(snapshot)?;
        let tmp = unique_tmp_path(&self.path);
        let result = write_relay_settings_via_tmp(&tmp, &self.path, &raw);
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

fn unique_tmp_path(path: &Path) -> PathBuf {
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).expect("OS RNG failed while generating temp file suffix");
    let random_hex = suffix
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    path.with_extension(format!("json.tmp.{}.{random_hex}", std::process::id()))
}

fn write_relay_settings_via_tmp(tmp: &Path, path: &Path, raw: &str) -> Result<()> {
    fs::write(tmp, raw).with_context(|| format!("writing relay settings tmp {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(tmp)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(tmp, perms)?;
    }
    fs::rename(tmp, path)
        .with_context(|| format!("renaming relay settings into place {}", path.display()))?;
    Ok(())
}

fn settings_file_path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|err| anyhow!("resolving app config dir: {err}"))?;
    Ok(dir.join("relay.json"))
}

fn normalize_relay_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let suffixes = [
        "/v1/incoming",
        "/v1/ping",
        "/api/v1/assistant-relay/tasks",
        "/api/v1/assistant-relay/ping",
        "/api/v1/assistant-relay",
        "/tasks",
        "/incoming",
        "/ping",
    ];
    for suffix in suffixes {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            return stripped.trim_end_matches('/').to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::{normalize_relay_url, unique_tmp_path, RelaySettings, RelaySettingsStore};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
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
        let store = Arc::new(RelaySettingsStore {
            path: path.clone(),
            inner: Mutex::new(RelaySettings::default()),
        });
        let writer_count = 16;

        let handles: Vec<_> = (0..writer_count)
            .map(|i| {
                let store = store.clone();
                thread::spawn(move || {
                    let token = format!("token-{i:02}");
                    store
                        .update(|s| s.relay_token = token.clone())
                        .map(|_| token)
                })
            })
            .collect();

        let mut candidates = HashSet::new();
        for handle in handles {
            let token = handle.join().unwrap().unwrap();
            candidates.insert(token);
        }

        let raw = std::fs::read_to_string(&path).unwrap();
        let persisted: RelaySettings = serde_json::from_str(&raw).unwrap();
        assert!(
            candidates.contains(&persisted.relay_token),
            "persisted relay_token must exactly match one writer's token, not a mix"
        );
        assert!(leftover_tmp_files(&root.0).is_empty());
    }

    #[test]
    fn concurrent_updates_to_different_fields_leave_disk_state_matching_the_final_in_memory_snapshot(
    ) {
        let root = TempTestDir::create("cross-field");
        let path = root.0.join("relay.json");
        let store = Arc::new(RelaySettingsStore {
            path: path.clone(),
            inner: Mutex::new(RelaySettings::default()),
        });
        let writer_count = 24;

        let handles: Vec<_> = (0..writer_count)
            .map(|i| {
                let store = store.clone();
                thread::spawn(move || {
                    store.update(|s| match i % 3 {
                        0 => s.relay_url = format!("http://host-{i}.example"),
                        1 => s.relay_token = format!("token-{i:02}"),
                        _ => s.paired_verified = i % 2 == 0,
                    })
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let expected = store.snapshot();
        let raw = std::fs::read_to_string(&path).unwrap();
        let persisted: RelaySettings = serde_json::from_str(&raw).unwrap();

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
        let mut s = RelaySettings::default();
        assert!(!s.pairing_pending(), "no relay configured is not 'pending'");

        s.relay_url = "http://127.0.0.1:8765".to_string();
        s.relay_token = "tok".to_string();
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
}
