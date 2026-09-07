use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub mic_enabled: bool,
}

pub struct AppSettingsStore {
    path: PathBuf,
    inner: Mutex<AppSettings>,
}

impl AppSettingsStore {
    pub fn load(app: &AppHandle) -> Result<Self> {
        let path = settings_file_path(app)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating app settings dir {}", parent.display()))?;
        }
        let inner = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading app settings at {}", path.display()))?;
            serde_json::from_str::<AppSettings>(&raw).unwrap_or_default()
        } else {
            AppSettings::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    pub fn snapshot(&self) -> AppSettings {
        self.inner
            .lock()
            .expect("app settings mutex poisoned")
            .clone()
    }

    pub fn update<F>(&self, mutate: F) -> Result<AppSettings>
    where
        F: FnOnce(&mut AppSettings),
    {
        let mut guard = self.inner.lock().expect("app settings mutex poisoned");
        mutate(&mut guard);
        let snapshot = guard.clone();
        drop(guard);
        self.persist(&snapshot)?;
        Ok(snapshot)
    }

    fn persist(&self, snapshot: &AppSettings) -> Result<()> {
        let raw = serde_json::to_string_pretty(snapshot)?;
        let tmp = unique_tmp_path(&self.path);
        let result = write_app_settings_via_tmp(&tmp, &self.path, &raw);
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

fn write_app_settings_via_tmp(tmp: &Path, path: &Path, raw: &str) -> Result<()> {
    fs::write(tmp, raw).with_context(|| format!("writing app settings tmp {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(tmp)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(tmp, perms)?;
    }
    fs::rename(tmp, path)
        .with_context(|| format!("renaming app settings into place {}", path.display()))?;
    Ok(())
}

fn settings_file_path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|err| anyhow!("resolving app config dir: {err}"))?;
    Ok(dir.join("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn default_matches_legacy_derived() {
        let value = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(value, serde_json::json!({ "micEnabled": false }));
    }

    struct TempTestDir(PathBuf);

    impl TempTestDir {
        fn create(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "sw-free-app-settings-{label}-{}-{}",
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
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn leftover_tmp_files(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().contains(".json.tmp."))
            .collect()
    }

    #[test]
    fn unique_tmp_path_never_returns_the_same_path_twice() {
        let base = PathBuf::from("/does/not/matter/settings.json");
        let mut seen = HashSet::new();
        for _ in 0..64 {
            assert!(seen.insert(unique_tmp_path(&base)));
        }
    }

    #[test]
    fn concurrent_updates_to_the_same_store_all_succeed_with_no_corruption_or_leftovers() {
        let root = TempTestDir::create("concurrency");
        let path = root.0.join("settings.json");
        let store = Arc::new(AppSettingsStore {
            path: path.clone(),
            inner: Mutex::new(AppSettings::default()),
        });
        let writer_count = 16;

        let handles: Vec<_> = (0..writer_count)
            .map(|i| {
                let store = store.clone();
                thread::spawn(move || {
                    let enabled = i % 2 == 0;
                    store.update(|s| s.mic_enabled = enabled).map(|_| enabled)
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let raw = fs::read_to_string(&path).unwrap();
        let _: AppSettings = serde_json::from_str(&raw).unwrap();
        assert!(leftover_tmp_files(&root.0).is_empty());
    }
}
