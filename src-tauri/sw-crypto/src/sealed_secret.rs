use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::file_crypto::{decrypt_bytes, encrypt_bytes};
use crate::identity::generate_local_root_key;

pub const SEALED_SECRET_VERSION: u32 = 1;
pub const KEYCHAIN_RETRY_AFTER: Duration = Duration::from_secs(60);
const SEALING_KEY_LEN: usize = 32;
const ASKED_MOMENTS_AGO: &str = "it was asked less than a minute ago";

pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

pub trait SealingKeyStore: Send + Sync {
    fn load(&self) -> Result<Option<Vec<u8>>, String>;
    fn store(&self, key: &[u8]) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedSecret {
    pub version: u32,
    pub sealed: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UnsealError {
    KeyUnavailable(String),
    KeyMissing,
    NewerFormat,
    Unreadable,
    WrongKey,
}

impl UnsealError {
    pub fn is_transient(&self) -> bool {
        matches!(self, UnsealError::KeyUnavailable(_))
    }
}

impl fmt::Display for UnsealError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnsealError::KeyUnavailable(reason) => write!(f, "the keychain refused: {reason}"),
            UnsealError::KeyMissing => f.write_str("its key is no longer in the keychain"),
            UnsealError::NewerFormat => f.write_str("a newer StageWhisper saved it"),
            UnsealError::Unreadable => f.write_str("its contents are unreadable"),
            UnsealError::WrongKey => f.write_str("it does not match its key"),
        }
    }
}

type SealingKey = Zeroizing<[u8; SEALING_KEY_LEN]>;

#[derive(Default)]
struct KeyState {
    cached: Option<SealingKey>,
    refused_at: Option<Instant>,
}

pub struct SecretSealer {
    keys: Arc<dyn SealingKeyStore>,
    clock: Clock,
    state: Mutex<KeyState>,
}

impl SecretSealer {
    pub fn new(keys: Arc<dyn SealingKeyStore>) -> Self {
        Self::with_clock(keys, Arc::new(Instant::now))
    }

    pub fn with_clock(keys: Arc<dyn SealingKeyStore>, clock: Clock) -> Self {
        Self {
            keys,
            clock,
            state: Mutex::new(KeyState::default()),
        }
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<SealedSecret, String> {
        let key = self.key_for_sealing()?;
        let sealed = encrypt_bytes(&key, plaintext).map_err(|_| "encryption failed".to_string())?;
        Ok(SealedSecret {
            version: SEALED_SECRET_VERSION,
            sealed: STANDARD.encode(sealed),
        })
    }

    pub fn seal_text(&self, text: &str) -> Result<SealedSecret, String> {
        self.seal(text.as_bytes())
    }

    pub fn seal_in_background(&self, plaintext: &[u8]) -> Result<SealedSecret, String> {
        if self.refused_recently(&self.lock_state()) {
            return Err(ASKED_MOMENTS_AGO.to_string());
        }
        self.seal(plaintext)
    }

    pub fn seal_text_in_background(&self, text: &str) -> Result<SealedSecret, String> {
        self.seal_in_background(text.as_bytes())
    }

    pub fn unseal(&self, secret: &SealedSecret) -> Result<Zeroizing<Vec<u8>>, UnsealError> {
        if secret.version != SEALED_SECRET_VERSION {
            return Err(UnsealError::NewerFormat);
        }
        let sealed = STANDARD
            .decode(&secret.sealed)
            .map_err(|_| UnsealError::Unreadable)?;
        let key = self.key_for_unsealing()?;
        decrypt_bytes(&key, &sealed)
            .map(Zeroizing::new)
            .map_err(|_| UnsealError::WrongKey)
    }

    pub fn unseal_text(&self, secret: &SealedSecret) -> Result<Zeroizing<String>, UnsealError> {
        let bytes = self.unseal(secret)?;
        std::str::from_utf8(&bytes)
            .map(|text| Zeroizing::new(text.to_string()))
            .map_err(|_| UnsealError::Unreadable)
    }

    fn key_for_unsealing(&self) -> Result<SealingKey, UnsealError> {
        let mut state = self.lock_state();
        if let Some(key) = state.cached.as_ref() {
            return Ok(key.clone());
        }
        if self.refused_recently(&state) {
            return Err(UnsealError::KeyUnavailable(ASKED_MOMENTS_AGO.to_string()));
        }
        let stored = self
            .ask_keychain(&mut state, |keys| keys.load())
            .map_err(UnsealError::KeyUnavailable)?;
        let key = sealing_key_from(stored.ok_or(UnsealError::KeyMissing)?)
            .ok_or(UnsealError::WrongKey)?;
        state.cached = Some(key.clone());
        Ok(key)
    }

    fn key_for_sealing(&self) -> Result<SealingKey, String> {
        let mut state = self.lock_state();
        if let Some(key) = state.cached.as_ref() {
            return Ok(key.clone());
        }
        let key = match self.ask_keychain(&mut state, |keys| keys.load())? {
            Some(stored) => {
                sealing_key_from(stored).ok_or("the saved sealing key is damaged".to_string())?
            }
            None => {
                let fresh = Zeroizing::new(generate_local_root_key());
                self.ask_keychain(&mut state, |keys| keys.store(&fresh[..]))?;
                fresh
            }
        };
        state.cached = Some(key.clone());
        Ok(key)
    }

    fn refused_recently(&self, state: &KeyState) -> bool {
        state.refused_at.is_some_and(|refused_at| {
            (self.clock)().saturating_duration_since(refused_at) < KEYCHAIN_RETRY_AFTER
        })
    }

    fn ask_keychain<T>(
        &self,
        state: &mut KeyState,
        request: impl FnOnce(&dyn SealingKeyStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let answer = request(self.keys.as_ref());
        state.refused_at = answer.is_err().then(|| (self.clock)());
        answer
    }

    fn lock_state(&self) -> MutexGuard<'_, KeyState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn sealing_key_from(bytes: Vec<u8>) -> Option<SealingKey> {
    let bytes = Zeroizing::new(bytes);
    let key: [u8; SEALING_KEY_LEN] = bytes.as_slice().try_into().ok()?;
    Some(Zeroizing::new(key))
}

#[cfg(test)]
#[path = "sealed_secret_tests.rs"]
mod tests;
