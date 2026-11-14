use crate::errors::CryptoError;
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const AMK_LEN: usize = 32;

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct AccountMasterKey([u8; AMK_LEN]);

impl AccountMasterKey {
    pub fn generate() -> Self {
        let mut bytes = [0u8; AMK_LEN];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; AMK_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; AMK_LEN] {
        &self.0
    }
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct DeviceIdentity {
    pub encryption_private: [u8; 32],
    pub encryption_public: [u8; 32],
    pub signing_seed: [u8; 32],
    pub signing_public: [u8; 32],
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let enc_secret = StaticSecret::random_from_rng(OsRng);
        let enc_public = X25519PublicKey::from(&enc_secret);

        let mut signing_seed = [0u8; 32];
        OsRng.fill_bytes(&mut signing_seed);
        let signing_key = SigningKey::from_bytes(&signing_seed);

        Self {
            encryption_private: enc_secret.to_bytes(),
            encryption_public: enc_public.to_bytes(),
            signing_seed,
            signing_public: signing_key.verifying_key().to_bytes(),
        }
    }

    pub fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.signing_seed)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SelfWrappedAmk {
    pub wrapped_amk: Vec<u8>,
    pub nonce: [u8; 12],
}

pub fn self_wrap_amk(amk: &AccountMasterKey) -> Result<SelfWrappedAmk, CryptoError> {
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(amk.as_bytes()));
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: amk.as_bytes(),
                aad: b"sw/amk/self-wrap/v1",
            },
        )
        .map_err(|_| CryptoError::InvalidEnvelope("amk".into()))?;
    Ok(SelfWrappedAmk {
        wrapped_amk: ciphertext,
        nonce: nonce_bytes,
    })
}

pub fn self_unwrap_amk(
    self_wrapped: &SelfWrappedAmk,
    amk: &AccountMasterKey,
) -> Result<AccountMasterKey, CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(amk.as_bytes()));
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&self_wrapped.nonce),
            Payload {
                msg: &self_wrapped.wrapped_amk,
                aad: b"sw/amk/self-wrap/v1",
            },
        )
        .map_err(|_| CryptoError::InvalidEnvelope("amk".into()))?;
    if plaintext.len() != AMK_LEN {
        return Err(CryptoError::InvalidEnvelope("amk".into()));
    }
    let mut bytes = [0u8; AMK_LEN];
    bytes.copy_from_slice(&plaintext);
    Ok(AccountMasterKey::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amk_generate_and_roundtrip() {
        let amk = AccountMasterKey::generate();
        let wrapped = self_wrap_amk(&amk).unwrap();
        let unwrapped = self_unwrap_amk(&wrapped, &amk).unwrap();
        assert_eq!(amk.as_bytes(), unwrapped.as_bytes());
    }

    #[test]
    fn device_identity_generate_is_deterministic_from_seed() {
        let id = DeviceIdentity::generate();
        assert_eq!(id.encryption_public.len(), 32);
        assert_eq!(id.signing_public.len(), 32);
        let sk = id.signing_key();
        assert_eq!(sk.verifying_key().to_bytes(), id.signing_public);
    }

    #[test]
    fn self_unwrap_rejects_tamper() {
        let amk = AccountMasterKey::generate();
        let mut wrapped = self_wrap_amk(&amk).unwrap();
        wrapped.wrapped_amk[0] ^= 0xFF;
        let res = self_unwrap_amk(&wrapped, &amk);
        assert!(res.is_err());
    }
}
