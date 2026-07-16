#[cfg(target_vendor = "apple")]
mod apple {
    use super::super::keychain;
    use sw_crypto::{
        derive_audio_key, derive_cache_key, derive_db_key, derive_file_key, derive_index_key,
        generate_local_root_key,
    };

    const KEYCHAIN_SERVICE: &str = "com.stagewhisper.free";
    const KEYCHAIN_ACCOUNT: &str = "device-root-key";

    pub struct DeviceKeyManager {
        root_key: [u8; 32],
    }

    impl DeviceKeyManager {
        pub fn load_or_create() -> Result<Self, String> {
            match keychain::load(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
                Ok(Some(bytes)) => {
                    if bytes.len() != 32 {
                        return Err(format!(
                            "Root key has wrong length: {} (expected 32)",
                            bytes.len()
                        ));
                    }
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&bytes);
                    Ok(Self { root_key: key })
                }
                Ok(None) => {
                    let key = generate_local_root_key();
                    keychain::store(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &key)?;
                    Ok(Self { root_key: key })
                }
                Err(e) => Err(format!("Failed to load device key: {e}")),
            }
        }

        #[allow(dead_code)]
        pub fn db_key(&self) -> Result<[u8; 32], String> {
            derive_db_key(&self.root_key).map_err(|e| e.to_string())
        }

        #[allow(dead_code)]
        pub fn audio_key(&self) -> Result<[u8; 32], String> {
            derive_audio_key(&self.root_key).map_err(|e| e.to_string())
        }

        #[allow(dead_code)]
        pub fn cache_key(&self) -> Result<[u8; 32], String> {
            derive_cache_key(&self.root_key).map_err(|e| e.to_string())
        }

        #[allow(dead_code)]
        pub fn index_key(&self) -> Result<[u8; 32], String> {
            derive_index_key(&self.root_key).map_err(|e| e.to_string())
        }

        pub fn file_key(&self) -> Result<[u8; 32], String> {
            derive_file_key(&self.root_key).map_err(|e| e.to_string())
        }

        #[allow(dead_code)]
        pub fn destroy_keychain_entry() -> Result<(), String> {
            keychain::delete(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        }
    }

    impl Drop for DeviceKeyManager {
        fn drop(&mut self) {
            use zeroize::Zeroize;
            self.root_key.zeroize();
        }
    }
}

#[cfg(not(target_vendor = "apple"))]
mod fallback {
    pub struct DeviceKeyManager;

    impl DeviceKeyManager {
        pub fn load_or_create() -> Result<Self, String> {
            Err(
                "Device key management requires macOS Keychain (not available on this platform)"
                    .into(),
            )
        }

        pub fn db_key(&self) -> Result<[u8; 32], String> {
            Err("Not available on this platform".into())
        }

        #[allow(dead_code)]
        pub fn audio_key(&self) -> Result<[u8; 32], String> {
            Err("Not available on this platform".into())
        }

        #[allow(dead_code)]
        pub fn cache_key(&self) -> Result<[u8; 32], String> {
            Err("Not available on this platform".into())
        }

        #[allow(dead_code)]
        pub fn index_key(&self) -> Result<[u8; 32], String> {
            Err("Not available on this platform".into())
        }

        pub fn file_key(&self) -> Result<[u8; 32], String> {
            Err("Not available on this platform".into())
        }
    }
}

#[cfg(target_vendor = "apple")]
pub use apple::DeviceKeyManager;

#[cfg(not(target_vendor = "apple"))]
pub use fallback::DeviceKeyManager;
