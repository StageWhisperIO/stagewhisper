use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelaySettings {
    pub relay_url: String,
    pub relay_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<u16>,
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
        mutate(&mut guard);
        guard.relay_url = normalize_relay_url(&guard.relay_url);
        let snapshot = guard.clone();
        drop(guard);
        self.persist(&snapshot)?;
        Ok(snapshot)
    }

    fn persist(&self, snapshot: &RelaySettings) -> Result<()> {
        let raw = serde_json::to_string_pretty(snapshot)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, raw)
            .with_context(|| format!("writing relay settings tmp {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&tmp)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&tmp, perms)?;
        }
        fs::rename(&tmp, &self.path).with_context(|| {
            format!("renaming relay settings into place {}", self.path.display())
        })?;
        Ok(())
    }
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
    use super::{normalize_relay_url, RelaySettings};

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
