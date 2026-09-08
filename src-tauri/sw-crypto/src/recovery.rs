use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::errors::CryptoError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryKdfParams {
    pub algorithm: String,
    pub memory_kib: u32,
    pub time_cost: u32,
    pub parallelism: u32,
}

impl RecoveryKdfParams {
    pub fn default_v1() -> Self {
        Self {
            algorithm: "argon2id".to_string(),
            memory_kib: 65536,
            time_cost: 3,
            parallelism: 1,
        }
    }
}

pub fn derive_recovery_key(
    passphrase: &str,
    salt: &[u8],
    params: &RecoveryKdfParams,
) -> Result<[u8; 32], CryptoError> {
    if params.algorithm != "argon2id" {
        return Err(CryptoError::InvalidEnvelope(format!(
            "unsupported recovery kdf algorithm: {}",
            params.algorithm
        )));
    }

    let argon = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(
            params.memory_kib,
            params.time_cost,
            params.parallelism,
            Some(32),
        )
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?,
    );
    let mut out = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
    Ok(out)
}

pub struct WrappedAmk {
    pub wrapped: Vec<u8>,
    pub nonce: Vec<u8>,
    pub salt: Vec<u8>,
    pub params: RecoveryKdfParams,
}

pub fn wrap_amk_with_passphrase(
    amk: &[u8; 32],
    passphrase: &str,
) -> Result<WrappedAmk, CryptoError> {
    let mut salt = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let params = RecoveryKdfParams::default_v1();
    let kek = derive_recovery_key(passphrase, &salt, &params)?;
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new(&kek.into());
    let wrapped = cipher
        .encrypt(Nonce::from_slice(&nonce), amk.as_ref())
        .map_err(|_| CryptoError::Encryption("recovery wrap failed".into()))?;
    Ok(WrappedAmk {
        wrapped,
        nonce: nonce.to_vec(),
        salt: salt.to_vec(),
        params,
    })
}

pub fn unwrap_amk_with_passphrase(
    passphrase: &str,
    salt: &[u8],
    nonce: &[u8],
    wrapped: &[u8],
    params: &RecoveryKdfParams,
) -> Result<[u8; 32], CryptoError> {
    let kek = derive_recovery_key(passphrase, salt, params)?;
    if nonce.len() != 12 {
        return Err(CryptoError::InvalidEnvelope("recovery nonce length".into()));
    }
    let cipher = ChaCha20Poly1305::new(&kek.into());
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce), wrapped)
        .map_err(|_| CryptoError::Decryption)?;
    if pt.len() != 32 {
        return Err(CryptoError::InvalidEnvelope(
            "recovery plaintext length".into(),
        ));
    }
    let mut amk = [0u8; 32];
    amk.copy_from_slice(&pt);
    Ok(amk)
}

pub fn generate_bip39_passphrase() -> Result<String, CryptoError> {
    let mut entropy = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut entropy);
    let mnemonic = bip39::Mnemonic::from_entropy(&entropy)
        .map_err(|e| CryptoError::KeyGeneration(e.to_string()))?;
    Ok(mnemonic.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_unwrap_roundtrip() {
        let amk = [0x77u8; 32];
        let wrap = wrap_amk_with_passphrase(&amk, "correct horse battery staple").unwrap();
        let out = unwrap_amk_with_passphrase(
            "correct horse battery staple",
            &wrap.salt,
            &wrap.nonce,
            &wrap.wrapped,
            &wrap.params,
        )
        .unwrap();
        assert_eq!(out, amk);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let amk = [0x88u8; 32];
        let wrap = wrap_amk_with_passphrase(&amk, "phrase one").unwrap();
        let err = unwrap_amk_with_passphrase(
            "phrase two",
            &wrap.salt,
            &wrap.nonce,
            &wrap.wrapped,
            &wrap.params,
        );
        assert!(err.is_err());
    }

    #[test]
    fn bip39_passphrase_is_24_words() {
        let phrase = generate_bip39_passphrase().unwrap();
        assert_eq!(phrase.split_whitespace().count(), 24);
    }

    #[test]
    fn unsupported_algorithm_fails() {
        let err = derive_recovery_key(
            "correct horse battery staple",
            &[0x11; 16],
            &RecoveryKdfParams {
                algorithm: "argon2i".to_string(),
                ..RecoveryKdfParams::default_v1()
            },
        )
        .unwrap_err();

        assert!(matches!(err, CryptoError::InvalidEnvelope(_)));
    }
}
