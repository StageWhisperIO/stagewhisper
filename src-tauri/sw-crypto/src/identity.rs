use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

use crate::errors::CryptoError;
use crate::kdf::derive_envelope_key;

#[derive(Serialize, Deserialize, Clone)]
pub struct IdentityPublicKey {
    pub bytes: [u8; 32],
}

impl IdentityPublicKey {
    pub fn to_base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(self.bytes)
    }

    pub fn from_base64(s: &str) -> Result<Self, CryptoError> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(|e| CryptoError::KeyGeneration(format!("invalid base64: {}", e)))?;
        if bytes.len() != 32 {
            return Err(CryptoError::KeyGeneration(format!(
                "expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self { bytes: arr })
    }
}

pub struct IdentityKeypair {
    secret: StaticSecret,
    public: PublicKey,
}

impl IdentityKeypair {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Self {
        let secret = StaticSecret::from(*bytes);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn public_key(&self) -> IdentityPublicKey {
        IdentityPublicKey {
            bytes: *self.public.as_bytes(),
        }
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    pub fn diffie_hellman(&self, peer: &IdentityPublicKey) -> Result<[u8; 32], CryptoError> {
        let peer_public = PublicKey::from(peer.bytes);
        let shared = self.secret.diffie_hellman(&peer_public);
        if shared.as_bytes().iter().all(|&b| b == 0) {
            return Err(CryptoError::KeyDerivation(
                "ECDH produced all-zero shared secret (low-order point)".into(),
            ));
        }
        Ok(*shared.as_bytes())
    }

    pub fn derive_envelope_key(&self, peer: &IdentityPublicKey) -> Result<[u8; 32], CryptoError> {
        let shared = self.diffie_hellman(peer)?;
        derive_envelope_key(&shared)
    }
}

impl Drop for IdentityKeypair {
    fn drop(&mut self) {
        let mut secret_bytes = self.secret.to_bytes();
        secret_bytes.zeroize();
    }
}

pub fn generate_local_root_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_generation_produces_valid_keys() {
        let kp = IdentityKeypair::generate();
        let pub_key = kp.public_key();
        assert_ne!(pub_key.bytes, [0u8; 32]);
    }

    #[test]
    fn roundtrip_from_secret_bytes() {
        let kp1 = IdentityKeypair::generate();
        let secret_bytes = kp1.secret_bytes();
        let kp2 = IdentityKeypair::from_secret_bytes(&secret_bytes);
        assert_eq!(kp1.public_key().bytes, kp2.public_key().bytes);
    }

    #[test]
    fn diffie_hellman_agreement() {
        let alice = IdentityKeypair::generate();
        let bob = IdentityKeypair::generate();

        let shared_ab = alice.diffie_hellman(&bob.public_key()).unwrap();
        let shared_ba = bob.diffie_hellman(&alice.public_key()).unwrap();
        assert_eq!(shared_ab, shared_ba);
    }

    #[test]
    fn derived_envelope_keys_match() {
        let alice = IdentityKeypair::generate();
        let bob = IdentityKeypair::generate();

        let key_ab = alice.derive_envelope_key(&bob.public_key()).unwrap();
        let key_ba = bob.derive_envelope_key(&alice.public_key()).unwrap();
        assert_eq!(key_ab, key_ba);
    }

    #[test]
    fn public_key_base64_roundtrip() {
        let kp = IdentityKeypair::generate();
        let b64 = kp.public_key().to_base64();
        let restored = IdentityPublicKey::from_base64(&b64).unwrap();
        assert_eq!(kp.public_key().bytes, restored.bytes);
    }

    #[test]
    fn local_root_key_is_random() {
        let k1 = generate_local_root_key();
        let k2 = generate_local_root_key();
        assert_ne!(k1, k2);
        assert_ne!(k1, [0u8; 32]);
    }
}
