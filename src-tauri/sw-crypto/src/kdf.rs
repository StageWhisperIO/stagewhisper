use crate::errors::CryptoError;
use hkdf::Hkdf;
use sha2::Sha256;

const ENVELOPE_INFO: &[u8] = b"stagewhisper-byo-envelope-v1";
const LOCAL_DB_INFO: &[u8] = b"stagewhisper-local-db-key";
const LOCAL_AUDIO_INFO: &[u8] = b"stagewhisper-local-audio-key";
const LOCAL_CACHE_INFO: &[u8] = b"stagewhisper-local-cache-key";
const LOCAL_INDEX_INFO: &[u8] = b"stagewhisper-local-index-key";
const LOCAL_FILE_INFO: &[u8] = b"stagewhisper-local-file-key";

pub fn derive_envelope_key(shared_secret: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    derive_key(shared_secret, ENVELOPE_INFO)
}

pub fn derive_db_key(root_key: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    derive_key(root_key, LOCAL_DB_INFO)
}

pub fn derive_audio_key(root_key: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    derive_key(root_key, LOCAL_AUDIO_INFO)
}

pub fn derive_cache_key(root_key: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    derive_key(root_key, LOCAL_CACHE_INFO)
}

pub fn derive_index_key(root_key: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    derive_key(root_key, LOCAL_INDEX_INFO)
}

pub fn derive_file_key(root_key: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    derive_key(root_key, LOCAL_FILE_INFO)
}

const CONTENT_WRAPPING_INFO: &[u8] = b"sw/cwk/v1";

pub fn derive_content_wrapping_key(amk: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    derive_key(amk, CONTENT_WRAPPING_INFO)
}

fn derive_key(ikm: &[u8; 32], info: &[u8]) -> Result<[u8; 32], CryptoError> {
    let hk = Hkdf::<Sha256>::new(None, ikm);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm)
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
    Ok(okm)
}

pub fn derive_key_with_context(ikm: &[u8; 32], context: &str) -> Result<[u8; 32], CryptoError> {
    let info = format!("stagewhisper-{}", context);
    derive_key(ikm, info.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_keys_are_deterministic() {
        let secret = [0xABu8; 32];
        let k1 = derive_envelope_key(&secret).unwrap();
        let k2 = derive_envelope_key(&secret).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn different_info_produces_different_keys() {
        let secret = [0xABu8; 32];
        let envelope = derive_envelope_key(&secret).unwrap();
        let db = derive_db_key(&secret).unwrap();
        let audio = derive_audio_key(&secret).unwrap();
        assert_ne!(envelope, db);
        assert_ne!(db, audio);
    }

    #[test]
    fn different_secrets_produce_different_keys() {
        let s1 = [0xAAu8; 32];
        let s2 = [0xBBu8; 32];
        let k1 = derive_envelope_key(&s1).unwrap();
        let k2 = derive_envelope_key(&s2).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn zeroize_works() {
        use zeroize::Zeroize;
        let secret = [0xABu8; 32];
        let mut key = derive_envelope_key(&secret).unwrap();
        let non_zero = key;
        key.zeroize();
        assert_ne!(non_zero, key);
        assert_eq!(key, [0u8; 32]);
    }
}
