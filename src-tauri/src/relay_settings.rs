use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sw_crypto::{SealedSecret, SecretSealer, UnsealError};
use tauri::{AppHandle, Manager};

use crate::sealed_secrets::{shared_sealer, KEYCHAIN_REFUSED};

pub const PAIR_AGAIN: &str =
    "Your assistant connection needs to be set up again. Pair it again in Settings.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenUnavailable {
    KeychainRefused,
    PairAgain,
}

impl TokenUnavailable {
    pub fn is_transient(self) -> bool {
        matches!(self, TokenUnavailable::KeychainRefused)
    }
}

impl fmt::Display for TokenUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TokenUnavailable::KeychainRefused => KEYCHAIN_REFUSED,
            TokenUnavailable::PairAgain => PAIR_AGAIN,
        })
    }
}

impl From<TokenUnavailable> for String {
    fn from(reason: TokenUnavailable) -> Self {
        reason.to_string()
    }
}

#[derive(Clone, Default)]
pub struct RelaySettings {
    pub relay_url: String,
    pub relay_token: String,
    pub paired_verified: bool,
}

impl fmt::Debug for RelaySettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelaySettings")
            .field("relay_url", &self.relay_url)
            .field("paired_verified", &self.paired_verified)
            .finish_non_exhaustive()
    }
}

impl RelaySettings {
    pub fn has_relay(&self) -> bool {
        !self.relay_url.trim().is_empty() && !self.relay_token.trim().is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    pub relay_url: String,
    pub has_relay_token: bool,
    pub paired_verified: bool,
}

impl RelayStatus {
    pub fn has_relay(&self) -> bool {
        !self.relay_url.trim().is_empty() && self.has_relay_token
    }

    pub fn pairing_pending(&self) -> bool {
        self.has_relay() && !self.paired_verified
    }

    pub fn ready(&self) -> bool {
        self.has_relay() && self.paired_verified
    }
}

#[derive(Default)]
pub struct RelayChange {
    pub relay_url: Option<String>,
    pub relay_token: Option<String>,
    pub paired_verified: Option<bool>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct StoredRelaySettings {
    relay_url: String,
    relay_token: String,
    paired_verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sealed_relay_token: Option<SealedSecret>,
}

impl StoredRelaySettings {
    fn status(&self) -> RelayStatus {
        RelayStatus {
            relay_url: self.relay_url.clone(),
            has_relay_token: !self.relay_token.trim().is_empty()
                || self.sealed_relay_token.is_some(),
            paired_verified: self.paired_verified,
        }
    }
}

enum TokenChange {
    Keep,
    Clear,
    Replace(SealedSecret),
}

pub struct RelaySettingsStore {
    path: PathBuf,
    sealer: Arc<SecretSealer>,
    inner: Mutex<StoredRelaySettings>,
    legacy_token_seal_attempted: AtomicBool,
    unusable_token_reported: AtomicBool,
}

impl RelaySettingsStore {
    pub fn load(app: &AppHandle) -> Result<Self> {
        Self::open_at(settings_file_path(app)?, shared_sealer())
    }

    pub fn open_at(path: PathBuf, sealer: Arc<SecretSealer>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating relay settings dir {}", parent.display()))?;
        }
        let stored = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading relay settings at {}", path.display()))?;
            serde_json::from_str::<StoredRelaySettings>(&raw).unwrap_or_default()
        } else {
            StoredRelaySettings::default()
        };
        Ok(Self {
            path,
            sealer,
            inner: Mutex::new(stored),
            legacy_token_seal_attempted: AtomicBool::new(false),
            unusable_token_reported: AtomicBool::new(false),
        })
    }

    pub fn snapshot(&self) -> RelayStatus {
        self.lock().status()
    }

    pub fn credentials(&self) -> std::result::Result<RelaySettings, TokenUnavailable> {
        let stored = self.lock().clone();
        let relay_token = if !stored.relay_token.trim().is_empty() {
            self.seal_legacy_token(&stored.relay_token);
            stored.relay_token.clone()
        } else if let Some(sealed) = &stored.sealed_relay_token {
            self.sealer
                .unseal_text(sealed)
                .map_err(|reason| self.report_unusable_token(reason))?
                .to_string()
        } else {
            String::new()
        };
        Ok(RelaySettings {
            relay_url: stored.relay_url,
            relay_token,
            paired_verified: stored.paired_verified,
        })
    }

    pub fn update(&self, change: RelayChange) -> Result<RelayStatus> {
        let token_change = match change.relay_token {
            None => TokenChange::Keep,
            Some(token) if token.trim().is_empty() => TokenChange::Clear,
            Some(token) => TokenChange::Replace(self.sealer.seal_text(&token).map_err(|err| {
                log::warn!("[relay] could not encrypt the assistant token: {err}");
                anyhow!(KEYCHAIN_REFUSED)
            })?),
        };
        let mut guard = self.lock();
        let mut candidate = guard.clone();
        if let Some(url) = change.relay_url {
            candidate.relay_url = url;
        }
        if let Some(verified) = change.paired_verified {
            candidate.paired_verified = verified;
        }
        match token_change {
            TokenChange::Keep => {}
            TokenChange::Clear => {
                candidate.relay_token.clear();
                candidate.sealed_relay_token = None;
            }
            TokenChange::Replace(sealed) => {
                candidate.relay_token.clear();
                candidate.sealed_relay_token = Some(sealed);
                self.unusable_token_reported.store(false, Ordering::SeqCst);
            }
        }
        candidate.relay_url = normalize_relay_url(&candidate.relay_url);
        self.persist(&candidate)?;
        *guard = candidate;
        Ok(guard.status())
    }

    fn seal_legacy_token(&self, plaintext: &str) {
        if self
            .legacy_token_seal_attempted
            .swap(true, Ordering::SeqCst)
        {
            return;
        }
        let sealed = match self.sealer.seal_text_in_background(plaintext) {
            Ok(sealed) => sealed,
            Err(err) => {
                log::warn!(
                    "[relay] kept the assistant token unencrypted in relay.json because the keychain refused it: {err}"
                );
                return;
            }
        };
        let mut guard = self.lock();
        if guard.relay_token != plaintext {
            return;
        }
        let mut candidate = guard.clone();
        candidate.relay_token.clear();
        candidate.sealed_relay_token = Some(sealed);
        match self.persist(&candidate) {
            Ok(()) => {
                *guard = candidate;
                log::info!("[relay] the assistant token in relay.json is now encrypted");
            }
            Err(err) => log::warn!(
                "[relay] kept the assistant token unencrypted because relay.json could not be rewritten: {err}"
            ),
        }
    }

    fn report_unusable_token(&self, reason: UnsealError) -> TokenUnavailable {
        if reason.is_transient() {
            log::info!(
                "[relay] the assistant token stays locked until the keychain allows it: {reason}"
            );
            return TokenUnavailable::KeychainRefused;
        }
        if !self.unusable_token_reported.swap(true, Ordering::SeqCst) {
            log::warn!("[relay] the saved assistant token could not be used: {reason}");
        }
        TokenUnavailable::PairAgain
    }

    fn lock(&self) -> MutexGuard<'_, StoredRelaySettings> {
        self.inner.lock().expect("relay settings mutex poisoned")
    }

    fn persist(&self, stored: &StoredRelaySettings) -> Result<()> {
        let raw = serde_json::to_string_pretty(stored)?;
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
#[path = "relay_settings_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "relay_settings_sealing_tests.rs"]
mod sealing_tests;
