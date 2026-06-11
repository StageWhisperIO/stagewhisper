use std::fs;
use std::path::PathBuf;
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
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, raw)
            .with_context(|| format!("writing app settings tmp {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&tmp)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&tmp, perms)?;
        }
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming app settings into place {}", self.path.display()))?;
        Ok(())
    }
}

fn settings_file_path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|err| anyhow!("resolving app config dir: {err}"))?;
    Ok(dir.join("settings.json"))
}
