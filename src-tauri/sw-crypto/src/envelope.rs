use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use uuid::Uuid;

use crate::errors::CryptoError;

pub const ENVELOPE_VERSION: &str = "static_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderRole {
    Desktop,
    Plugin,
}

impl SenderRole {
    pub fn opposite(&self) -> Self {
        match self {
            SenderRole::Desktop => SenderRole::Plugin,
            SenderRole::Plugin => SenderRole::Desktop,
        }
    }
}

impl std::fmt::Display for SenderRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SenderRole::Desktop => write!(f, "desktop"),
            SenderRole::Plugin => write!(f, "plugin"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    ReasoningInput,
    ReasoningOutput,
    ToolIntent,
    ToolResult,
    TaskContent,
    TaskReply,
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BYOEnvelope {
    pub version: String,
    pub sender_role: SenderRole,
    pub message_id: String,
    pub session_id: String,
    pub correlation_id: String,
    pub content_type: ContentType,
    #[serde(with = "base64_bytes")]
    pub nonce: Vec<u8>,
    #[serde(with = "base64_bytes")]
    pub ciphertext: Vec<u8>,
}

mod base64_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)
    }
}

const REPLAY_GUARD_MAX_IDS: usize = 10_000;

pub struct ReplayGuard {
    seen: HashSet<String>,
    order: VecDeque<String>,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
        }
    }

    pub fn check(&mut self, message_id: &str) -> Result<(), CryptoError> {
        if self.seen.contains(message_id) {
            return Err(CryptoError::ReplayDetected(message_id.to_string()));
        }
        while self.seen.len() >= REPLAY_GUARD_MAX_IDS {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            } else {
                break;
            }
        }
        self.seen.insert(message_id.to_string());
        self.order.push_back(message_id.to_string());
        Ok(())
    }
}

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

pub fn open_checked(
    key: &[u8; 32],
    envelope: &BYOEnvelope,
    replay_guard: &mut ReplayGuard,
) -> Result<Vec<u8>, CryptoError> {
    replay_guard.check(&envelope.message_id)?;
    open(key, envelope)
}

pub fn open_json_checked<T: for<'de> Deserialize<'de>>(
    key: &[u8; 32],
    envelope: &BYOEnvelope,
    replay_guard: &mut ReplayGuard,
) -> Result<T, CryptoError> {
    replay_guard.check(&envelope.message_id)?;
    open_json(key, envelope)
}

impl BYOEnvelope {
    fn build_aad(&self) -> Vec<u8> {
        let aad_data = serde_json::json!({
            "version": self.version,
            "sender_role": self.sender_role,
            "message_id": self.message_id,
            "session_id": self.session_id,
            "correlation_id": self.correlation_id,
            "content_type": self.content_type,
        });
        serde_json::to_vec(&aad_data).expect("AAD serialization cannot fail")
    }
}

pub fn seal(
    key: &[u8; 32],
    sender_role: SenderRole,
    session_id: &str,
    correlation_id: &str,
    content_type: ContentType,
    plaintext: &[u8],
) -> Result<BYOEnvelope, CryptoError> {
    let message_id = Uuid::new_v4().to_string();

    let mut nonce_bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);

    let envelope = BYOEnvelope {
        version: ENVELOPE_VERSION.to_string(),
        sender_role,
        message_id,
        session_id: session_id.to_string(),
        correlation_id: correlation_id.to_string(),
        content_type,
        nonce: nonce_bytes.to_vec(),
        ciphertext: Vec::new(),
    };

    let aad = envelope.build_aad();
    let nonce = XNonce::from_slice(&nonce_bytes);
    let cipher = XChaCha20Poly1305::new(key.into());

    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Encryption("XChaCha20-Poly1305 seal failed".into()))?;

    Ok(BYOEnvelope {
        ciphertext,
        ..envelope
    })
}

pub fn open(key: &[u8; 32], envelope: &BYOEnvelope) -> Result<Vec<u8>, CryptoError> {
    if envelope.version != ENVELOPE_VERSION {
        return Err(CryptoError::UnsupportedVersion(envelope.version.clone()));
    }

    if envelope.nonce.len() != 24 {
        return Err(CryptoError::InvalidEnvelope(format!(
            "nonce must be 24 bytes, got {}",
            envelope.nonce.len()
        )));
    }

    let aad = envelope.build_aad();
    let nonce = XNonce::from_slice(&envelope.nonce);
    let cipher = XChaCha20Poly1305::new(key.into());

    cipher
        .decrypt(
            nonce,
            Payload {
                msg: &envelope.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Decryption)
}

pub fn seal_json<T: Serialize>(
    key: &[u8; 32],
    sender_role: SenderRole,
    session_id: &str,
    correlation_id: &str,
    content_type: ContentType,
    value: &T,
) -> Result<BYOEnvelope, CryptoError> {
    let plaintext = serde_json::to_vec(value)?;
    seal(
        key,
        sender_role,
        session_id,
        correlation_id,
        content_type,
        &plaintext,
    )
}

pub fn open_json<T: for<'de> Deserialize<'de>>(
    key: &[u8; 32],
    envelope: &BYOEnvelope,
) -> Result<T, CryptoError> {
    let plaintext = open(key, envelope)?;
    serde_json::from_slice(&plaintext).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityKeypair;

    fn test_key() -> [u8; 32] {
        let alice = IdentityKeypair::generate();
        let bob = IdentityKeypair::generate();
        alice.derive_envelope_key(&bob.public_key()).unwrap()
    }

    #[test]
    fn seal_and_open_roundtrip() {
        let key = test_key();
        let plaintext = b"hello world";
        let envelope = seal(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            plaintext,
        )
        .unwrap();

        assert_eq!(envelope.version, ENVELOPE_VERSION);
        assert_eq!(envelope.sender_role, SenderRole::Desktop);
        assert_ne!(&envelope.ciphertext, plaintext);

        let decrypted = open(&key, &envelope).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = test_key();
        let key2 = test_key();

        let envelope = seal(
            &key1,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            b"secret",
        )
        .unwrap();

        let result = open(&key2, &envelope);
        assert!(matches!(result, Err(CryptoError::Decryption)));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = test_key();
        let mut envelope = seal(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            b"original",
        )
        .unwrap();

        if let Some(byte) = envelope.ciphertext.first_mut() {
            *byte ^= 0xFF;
        }

        let result = open(&key, &envelope);
        assert!(matches!(result, Err(CryptoError::Decryption)));
    }

    #[test]
    fn tampered_aad_session_id_fails() {
        let key = test_key();
        let mut envelope = seal(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            b"original",
        )
        .unwrap();

        envelope.session_id = "session-TAMPERED".to_string();

        let result = open(&key, &envelope);
        assert!(matches!(result, Err(CryptoError::Decryption)));
    }

    #[test]
    fn tampered_aad_sender_role_fails() {
        let key = test_key();
        let mut envelope = seal(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            b"original",
        )
        .unwrap();

        envelope.sender_role = SenderRole::Plugin;

        let result = open(&key, &envelope);
        assert!(matches!(result, Err(CryptoError::Decryption)));
    }

    #[test]
    fn tampered_aad_content_type_fails() {
        let key = test_key();
        let mut envelope = seal(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            b"original",
        )
        .unwrap();

        envelope.content_type = ContentType::TaskContent;

        let result = open(&key, &envelope);
        assert!(matches!(result, Err(CryptoError::Decryption)));
    }

    #[test]
    fn unique_message_ids() {
        let key = test_key();
        let e1 = seal(
            &key,
            SenderRole::Desktop,
            "s",
            "c",
            ContentType::ReasoningInput,
            b"a",
        )
        .unwrap();
        let e2 = seal(
            &key,
            SenderRole::Desktop,
            "s",
            "c",
            ContentType::ReasoningInput,
            b"a",
        )
        .unwrap();
        assert_ne!(e1.message_id, e2.message_id);
    }

    #[test]
    fn unique_nonces() {
        let key = test_key();
        let e1 = seal(
            &key,
            SenderRole::Desktop,
            "s",
            "c",
            ContentType::ReasoningInput,
            b"a",
        )
        .unwrap();
        let e2 = seal(
            &key,
            SenderRole::Desktop,
            "s",
            "c",
            ContentType::ReasoningInput,
            b"a",
        )
        .unwrap();
        assert_ne!(e1.nonce, e2.nonce);
    }

    #[test]
    fn json_roundtrip() {
        let key = test_key();
        let payload = serde_json::json!({
            "system_instruction": "You are an analyst",
            "user_prompt": "Analyze the transcript",
            "response_schema": {"type": "object"}
        });

        let envelope = seal_json(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            &payload,
        )
        .unwrap();

        let decrypted: serde_json::Value = open_json(&key, &envelope).unwrap();
        assert_eq!(decrypted, payload);
    }

    #[test]
    fn envelope_serialization_roundtrip() {
        let key = test_key();
        let envelope = seal(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            b"test payload",
        )
        .unwrap();

        let json = serde_json::to_string(&envelope).unwrap();
        let restored: BYOEnvelope = serde_json::from_str(&json).unwrap();

        let decrypted = open(&key, &restored).unwrap();
        assert_eq!(decrypted, b"test payload");
    }

    #[test]
    fn unsupported_version_rejected() {
        let key = test_key();
        let mut envelope = seal(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            b"data",
        )
        .unwrap();

        envelope.version = "static_v99".to_string();

        let result = open(&key, &envelope);
        assert!(matches!(result, Err(CryptoError::UnsupportedVersion(_))));
    }

    #[test]
    fn replay_guard_detects_duplicate() {
        let key = test_key();
        let envelope = seal(
            &key,
            SenderRole::Desktop,
            "session-1",
            "corr-1",
            ContentType::ReasoningInput,
            b"data",
        )
        .unwrap();

        let mut guard = ReplayGuard::new();
        let first = open_checked(&key, &envelope, &mut guard);
        assert!(first.is_ok());

        let second = open_checked(&key, &envelope, &mut guard);
        assert!(matches!(second, Err(CryptoError::ReplayDetected(_))));
    }

    #[test]
    fn replay_guard_allows_different_messages() {
        let key = test_key();
        let e1 = seal(
            &key,
            SenderRole::Desktop,
            "s",
            "c",
            ContentType::ReasoningInput,
            b"a",
        )
        .unwrap();
        let e2 = seal(
            &key,
            SenderRole::Desktop,
            "s",
            "c",
            ContentType::ReasoningInput,
            b"b",
        )
        .unwrap();

        let mut guard = ReplayGuard::new();
        assert!(open_checked(&key, &e1, &mut guard).is_ok());
        assert!(open_checked(&key, &e2, &mut guard).is_ok());
    }

    #[test]
    fn aad_is_alphabetically_ordered_json() {
        let envelope = BYOEnvelope {
            version: ENVELOPE_VERSION.to_string(),
            sender_role: SenderRole::Desktop,
            message_id: "test-id".to_string(),
            session_id: "sess-1".to_string(),
            correlation_id: "corr-1".to_string(),
            content_type: ContentType::ReasoningInput,
            nonce: vec![],
            ciphertext: vec![],
        };
        let aad = envelope.build_aad();
        let aad_str = String::from_utf8(aad).unwrap();
        let expected = r#"{"content_type":"reasoning_input","correlation_id":"corr-1","message_id":"test-id","sender_role":"desktop","session_id":"sess-1","version":"static_v1"}"#;
        assert_eq!(aad_str, expected);
    }
}
