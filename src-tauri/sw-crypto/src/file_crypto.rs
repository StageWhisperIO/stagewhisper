use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;

use crate::errors::CryptoError;

const NONCE_LEN: usize = 24;

pub fn encrypt_bytes(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CryptoError::Encryption(e.to_string()))?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt_bytes(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.len() < NONCE_LEN + 16 {
        return Err(CryptoError::InvalidEnvelope(
            "ciphertext too short for file decryption".to_string(),
        ));
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let nonce = XNonce::from_slice(nonce_bytes);
    let cipher = XChaCha20Poly1305::new(key.into());

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::Decryption)
}

pub fn encrypt_file(
    key: &[u8; 32],
    path: &std::path::Path,
    plaintext: &[u8],
) -> Result<(), CryptoError> {
    let encrypted = encrypt_bytes(key, plaintext)?;
    std::fs::write(path, encrypted).map_err(|e| CryptoError::Encryption(e.to_string()))
}

pub fn decrypt_file(key: &[u8; 32], path: &std::path::Path) -> Result<Vec<u8>, CryptoError> {
    let data = std::fs::read(path).map_err(|e| CryptoError::InvalidEnvelope(e.to_string()))?;
    decrypt_bytes(key, &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = [0xABu8; 32];
        let plaintext = b"hello world, this is screen recording data";
        let encrypted = encrypt_bytes(&key, plaintext).unwrap();
        assert_ne!(encrypted, plaintext);
        assert!(encrypted.len() > plaintext.len());
        let decrypted = decrypt_bytes(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = [0xAAu8; 32];
        let key2 = [0xBBu8; 32];
        let plaintext = b"secret data";
        let encrypted = encrypt_bytes(&key1, plaintext).unwrap();
        assert!(decrypt_bytes(&key2, &encrypted).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = [0xCCu8; 32];
        let plaintext = b"important bytes";
        let mut encrypted = encrypt_bytes(&key, plaintext).unwrap();
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;
        assert!(decrypt_bytes(&key, &encrypted).is_err());
    }

    #[test]
    fn too_short_data_fails() {
        let key = [0xDDu8; 32];
        assert!(decrypt_bytes(&key, &[0u8; 10]).is_err());
    }

    #[test]
    fn empty_plaintext_roundtrip() {
        let key = [0xEEu8; 32];
        let encrypted = encrypt_bytes(&key, b"").unwrap();
        let decrypted = decrypt_bytes(&key, &encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn large_data_roundtrip() {
        let key = [0x11u8; 32];
        let plaintext = vec![0x42u8; 10_000_000];
        let encrypted = encrypt_bytes(&key, &plaintext).unwrap();
        let decrypted = decrypt_bytes(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn file_roundtrip() {
        let key = [0x22u8; 32];
        let plaintext = b"screenshot jpeg bytes would go here";
        let dir = std::env::temp_dir().join("sw_crypto_file_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test_encrypted.bin");

        encrypt_file(&key, &path, plaintext).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert_ne!(raw, plaintext);

        let decrypted = decrypt_file(&key, &path).unwrap();
        assert_eq!(decrypted, plaintext);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
