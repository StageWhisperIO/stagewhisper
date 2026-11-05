use std::sync::{Arc, OnceLock};

use sw_crypto::{SealingKeyStore, SecretSealer};

#[cfg(any(target_vendor = "apple", windows))]
const SEALING_KEY_SERVICE: &str = "com.stagewhisper.free.secrets";
#[cfg(any(target_vendor = "apple", windows))]
const SEALING_KEY_ACCOUNT: &str = "settings-sealing-key";

pub const KEYCHAIN_REFUSED: &str =
    "StageWhisper couldn't use the system keychain. Allow access when asked, then try again.";

struct OsKeychainEntry;

#[cfg(target_vendor = "apple")]
impl SealingKeyStore for OsKeychainEntry {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        crate::state::keychain::load(SEALING_KEY_SERVICE, SEALING_KEY_ACCOUNT)
    }

    fn store(&self, key: &[u8]) -> Result<(), String> {
        crate::state::keychain::store(SEALING_KEY_SERVICE, SEALING_KEY_ACCOUNT, key)
    }
}

#[cfg(windows)]
impl SealingKeyStore for OsKeychainEntry {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        sw_device_key::keychain::load(SEALING_KEY_SERVICE, SEALING_KEY_ACCOUNT)
    }

    fn store(&self, key: &[u8]) -> Result<(), String> {
        sw_device_key::keychain::store(SEALING_KEY_SERVICE, SEALING_KEY_ACCOUNT, key)
    }
}

#[cfg(not(any(target_vendor = "apple", windows)))]
impl SealingKeyStore for OsKeychainEntry {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Err("no system keychain on this platform".to_string())
    }

    fn store(&self, _key: &[u8]) -> Result<(), String> {
        Err("no system keychain on this platform".to_string())
    }
}

pub fn shared_sealer() -> Arc<SecretSealer> {
    static SHARED: OnceLock<Arc<SecretSealer>> = OnceLock::new();
    SHARED
        .get_or_init(|| Arc::new(SecretSealer::new(Arc::new(OsKeychainEntry))))
        .clone()
}

#[cfg(test)]
pub mod test_doubles;
