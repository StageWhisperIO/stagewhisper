use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Nonce as ChaChaNonce, XChaCha20Poly1305, XNonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::errors::CryptoError;
use crate::kdf::derive_content_wrapping_key;

pub const OBJECT_ENVELOPE_VERSION: u32 = 1;
pub const ALGORITHM_XCHACHA20_POLY1305: &str = "XChaCha20-Poly1305";

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub object_type: String,
    pub object_id: String,
    pub owner_id: String,
    pub created_at: String,
    pub content_length: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectEnvelopeHeader {
    pub version: u32,
    pub object_type: String,
    pub object_id: String,
    pub owner_id: String,
    pub created_at: String,
    pub algorithm: String,
    #[serde(with = "b64")]
    pub nonce: Vec<u8>,
    #[serde(with = "b64")]
    pub ciphertext_sha256: Vec<u8>,
    #[serde(with = "b64")]
    pub wrapped_dek: Vec<u8>,
    #[serde(with = "b64")]
    pub wrapped_dek_nonce: Vec<u8>,
    #[serde(with = "b64")]
    pub aad: Vec<u8>,
    pub signing_device_id: String,
    pub content_length: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectEnvelope {
    #[serde(flatten)]
    pub header: ObjectEnvelopeHeader,
    #[serde(with = "b64")]
    pub ciphertext: Vec<u8>,
    #[serde(with = "b64")]
    pub signature: Vec<u8>,
}

fn build_aad(meta: &ObjectMetadata, version: u32) -> Vec<u8> {
    let aad_data = serde_json::json!({
        "content_length": meta.content_length,
        "encryption_version": version,
        "mime_type": meta.mime_type,
        "object_id": meta.object_id,
        "object_type": meta.object_type,
        "owner_id": meta.owner_id,
    });
    serde_json::to_vec(&aad_data).expect("AAD serialization cannot fail")
}

fn canonical_header_bytes(header: &ObjectEnvelopeHeader) -> Vec<u8> {
    serde_json::to_vec(header).expect("header serialization cannot fail")
}

fn wrap_dek(cwk: &[u8; 32], dek: &[u8; 32]) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new(cwk.into());
    let wrapped = cipher
        .encrypt(ChaChaNonce::from_slice(&nonce), dek.as_ref())
        .map_err(|_| CryptoError::Encryption("DEK wrap failed".into()))?;
    Ok((wrapped, nonce.to_vec()))
}

fn unwrap_dek(cwk: &[u8; 32], wrapped: &[u8], nonce: &[u8]) -> Result<[u8; 32], CryptoError> {
    if nonce.len() != 12 {
        return Err(CryptoError::InvalidEnvelope(
            "wrapped dek nonce length".into(),
        ));
    }
    let cipher = ChaCha20Poly1305::new(cwk.into());
    let plaintext = cipher
        .decrypt(ChaChaNonce::from_slice(nonce), wrapped)
        .map_err(|_| CryptoError::Decryption)?;
    if plaintext.len() != 32 {
        return Err(CryptoError::InvalidEnvelope("unwrapped dek size".into()));
    }
    let mut dek = [0u8; 32];
    dek.copy_from_slice(&plaintext);
    Ok(dek)
}

pub fn encrypt_object(
    plaintext: &[u8],
    metadata: &ObjectMetadata,
    content_wrapping_key: &[u8; 32],
    signing_key: &SigningKey,
    signing_device_id: &str,
) -> Result<ObjectEnvelope, CryptoError> {
    encrypt_object_with(
        plaintext,
        metadata,
        content_wrapping_key,
        signing_key,
        signing_device_id,
        None,
        None,
    )
}

pub fn encrypt_object_with(
    plaintext: &[u8],
    metadata: &ObjectMetadata,
    content_wrapping_key: &[u8; 32],
    signing_key: &SigningKey,
    signing_device_id: &str,
    fixed_dek: Option<[u8; 32]>,
    fixed_nonce: Option<[u8; 24]>,
) -> Result<ObjectEnvelope, CryptoError> {
    let mut dek = [0u8; 32];
    match fixed_dek {
        Some(k) => dek.copy_from_slice(&k),
        None => rand::rngs::OsRng.fill_bytes(&mut dek),
    }

    let mut nonce = [0u8; 24];
    match fixed_nonce {
        Some(n) => nonce.copy_from_slice(&n),
        None => rand::rngs::OsRng.fill_bytes(&mut nonce),
    }

    let aad = build_aad(metadata, OBJECT_ENVELOPE_VERSION);

    let cipher = XChaCha20Poly1305::new(&dek.into());
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Encryption("object ciphertext seal failed".into()))?;

    let mut hasher = Sha256::new();
    hasher.update(&ciphertext);
    let ciphertext_sha256 = hasher.finalize().to_vec();

    let (wrapped_dek, wrapped_dek_nonce) = wrap_dek(content_wrapping_key, &dek)?;

    let header = ObjectEnvelopeHeader {
        version: OBJECT_ENVELOPE_VERSION,
        object_type: metadata.object_type.clone(),
        object_id: metadata.object_id.clone(),
        owner_id: metadata.owner_id.clone(),
        created_at: metadata.created_at.clone(),
        algorithm: ALGORITHM_XCHACHA20_POLY1305.to_string(),
        nonce: nonce.to_vec(),
        ciphertext_sha256,
        wrapped_dek,
        wrapped_dek_nonce,
        aad,
        signing_device_id: signing_device_id.to_string(),
        content_length: metadata.content_length,
        mime_type: metadata.mime_type.clone(),
    };

    let signature = signing_key.sign(&canonical_header_bytes(&header));

    Ok(ObjectEnvelope {
        header,
        ciphertext,
        signature: signature.to_bytes().to_vec(),
    })
}

pub fn decrypt_object(
    envelope: &ObjectEnvelope,
    content_wrapping_key: &[u8; 32],
    verifying_key: &VerifyingKey,
) -> Result<Vec<u8>, CryptoError> {
    if envelope.header.version != OBJECT_ENVELOPE_VERSION {
        return Err(CryptoError::UnsupportedVersion(format!(
            "{}",
            envelope.header.version
        )));
    }
    if envelope.header.algorithm != ALGORITHM_XCHACHA20_POLY1305 {
        return Err(CryptoError::UnsupportedVersion(
            envelope.header.algorithm.clone(),
        ));
    }
    if envelope.header.nonce.len() != 24 {
        return Err(CryptoError::InvalidEnvelope("nonce length".into()));
    }
    if envelope.signature.len() != 64 {
        return Err(CryptoError::InvalidEnvelope("signature length".into()));
    }

    let mut hasher = Sha256::new();
    hasher.update(&envelope.ciphertext);
    let digest = hasher.finalize();
    if digest.as_slice() != envelope.header.ciphertext_sha256.as_slice() {
        return Err(CryptoError::InvalidEnvelope(
            "ciphertext digest mismatch".into(),
        ));
    }

    let mut sig_bytes = [0u8; 64];
    sig_bytes.copy_from_slice(&envelope.signature);
    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify(&canonical_header_bytes(&envelope.header), &signature)
        .map_err(|_| CryptoError::Decryption)?;

    let dek = unwrap_dek(
        content_wrapping_key,
        &envelope.header.wrapped_dek,
        &envelope.header.wrapped_dek_nonce,
    )?;

    let cipher = XChaCha20Poly1305::new(&dek.into());
    cipher
        .decrypt(
            XNonce::from_slice(&envelope.header.nonce),
            Payload {
                msg: &envelope.ciphertext,
                aad: &envelope.header.aad,
            },
        )
        .map_err(|_| CryptoError::Decryption)
}

pub fn wrap_amk_for_recipient(
    cwk: &[u8; 32],
    amk: &[u8; 32],
) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    wrap_dek(cwk, amk)
}

pub fn unwrap_amk_for_recipient(
    cwk: &[u8; 32],
    wrapped: &[u8],
    nonce: &[u8],
) -> Result<[u8; 32], CryptoError> {
    unwrap_dek(cwk, wrapped, nonce)
}

pub fn derive_and_wrap(amk: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cwk = derive_content_wrapping_key(amk)?;
    let cipher = XChaCha20Poly1305::new(&cwk.into());
    let mut nonce = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut out = nonce.to_vec();
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| CryptoError::Encryption("wrap failed".into()))?;
    out.extend_from_slice(&ct);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn make_meta() -> ObjectMetadata {
        ObjectMetadata {
            object_type: "session_message".into(),
            object_id: "00000000-0000-0000-0000-000000000001".into(),
            owner_id: "00000000-0000-0000-0000-0000000000aa".into(),
            created_at: "2025-01-01T00:00:00Z".into(),
            content_length: 11,
            mime_type: "text/plain".into(),
        }
    }

    fn make_signer() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    #[test]
    fn roundtrip() {
        let amk = [0x11u8; 32];
        let cwk = derive_content_wrapping_key(&amk).unwrap();
        let signer = make_signer();
        let vk = signer.verifying_key();
        let meta = make_meta();

        let env = encrypt_object(b"hello world", &meta, &cwk, &signer, "device-a").unwrap();
        let pt = decrypt_object(&env, &cwk, &vk).unwrap();
        assert_eq!(pt, b"hello world");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let amk = [0x22u8; 32];
        let cwk = derive_content_wrapping_key(&amk).unwrap();
        let signer = make_signer();
        let vk = signer.verifying_key();
        let meta = make_meta();

        let mut env = encrypt_object(b"hello", &meta, &cwk, &signer, "device-a").unwrap();
        env.ciphertext[0] ^= 0xFF;
        assert!(decrypt_object(&env, &cwk, &vk).is_err());
    }

    #[test]
    fn tampered_aad_fails() {
        let amk = [0x33u8; 32];
        let cwk = derive_content_wrapping_key(&amk).unwrap();
        let signer = make_signer();
        let vk = signer.verifying_key();
        let meta = make_meta();

        let mut env = encrypt_object(b"hello", &meta, &cwk, &signer, "device-a").unwrap();
        env.header.aad[0] ^= 0xFF;
        assert!(decrypt_object(&env, &cwk, &vk).is_err());
    }

    #[test]
    fn bad_signature_fails() {
        let amk = [0x44u8; 32];
        let cwk = derive_content_wrapping_key(&amk).unwrap();
        let signer = make_signer();
        let other = make_signer();
        let meta = make_meta();

        let env = encrypt_object(b"hello", &meta, &cwk, &signer, "device-a").unwrap();
        let result = decrypt_object(&env, &cwk, &other.verifying_key());
        assert!(result.is_err());
    }

    #[test]
    fn known_vector_deterministic() {
        let amk = [0x01u8; 32];
        let cwk = derive_content_wrapping_key(&amk).unwrap();
        let dek = [0x02u8; 32];
        let nonce = [0x03u8; 24];
        let signer = SigningKey::from_bytes(&[0x04u8; 32]);
        let vk = signer.verifying_key();
        let meta = ObjectMetadata {
            object_type: "session_message".into(),
            object_id: "fixed-object".into(),
            owner_id: "fixed-owner".into(),
            created_at: "2025-01-01T00:00:00Z".into(),
            content_length: 5,
            mime_type: "text/plain".into(),
        };

        let env = encrypt_object_with(
            b"hello",
            &meta,
            &cwk,
            &signer,
            "device-test",
            Some(dek),
            Some(nonce),
        )
        .unwrap();

        assert_eq!(env.header.nonce, nonce.to_vec());
        let ct_hex = hex::encode(&env.ciphertext);
        assert_eq!(ct_hex.len(), (5 + 16) * 2);

        let pt = decrypt_object(&env, &cwk, &vk).unwrap();
        assert_eq!(pt, b"hello");
    }

    #[test]
    fn serde_json_roundtrip() {
        let amk = [0x55u8; 32];
        let cwk = derive_content_wrapping_key(&amk).unwrap();
        let signer = make_signer();
        let vk = signer.verifying_key();
        let env = encrypt_object(b"abc", &make_meta(), &cwk, &signer, "device-a").unwrap();
        let json = serde_json::to_string(&env).unwrap();
        let restored: ObjectEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decrypt_object(&restored, &cwk, &vk).unwrap(), b"abc");
    }

    #[test]
    #[ignore]
    fn emit_interop_fixture() {
        let amk = [0x01u8; 32];
        let cwk = derive_content_wrapping_key(&amk).unwrap();
        let dek = [0x02u8; 32];
        let nonce = [0x03u8; 24];
        let signer = SigningKey::from_bytes(&[0x04u8; 32]);
        let meta = ObjectMetadata {
            object_type: "session_message".into(),
            object_id: "fixed-object".into(),
            owner_id: "fixed-owner".into(),
            created_at: "2025-01-01T00:00:00Z".into(),
            content_length: 5,
            mime_type: "text/plain".into(),
        };
        let env = encrypt_object_with(
            b"hello",
            &meta,
            &cwk,
            &signer,
            "device-test",
            Some(dek),
            Some(nonce),
        )
        .unwrap();
        let j = serde_json::json!({
            "amk_hex": hex::encode(amk),
            "cwk_hex": hex::encode(cwk),
            "dek_hex": hex::encode(dek),
            "nonce_hex": hex::encode(nonce),
            "signing_seed_hex": hex::encode(signer.to_bytes()),
            "verifying_key_hex": hex::encode(signer.verifying_key().to_bytes()),
            "plaintext_utf8": "hello",
            "envelope": env,
        });
        println!("FIXTURE:{}", serde_json::to_string(&j).unwrap());
    }
}
