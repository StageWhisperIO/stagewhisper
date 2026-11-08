use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("key generation failed: {0}")]
    KeyGeneration(String),

    #[error("key derivation failed: {0}")]
    KeyDerivation(String),

    #[error("encryption failed: {0}")]
    Encryption(String),

    #[error("decryption failed: authenticated decryption rejected")]
    Decryption,

    #[error("invalid envelope: {0}")]
    InvalidEnvelope(String),

    #[error("replay detected: message_id {0} already seen")]
    ReplayDetected(String),

    #[error("sender role mismatch: expected {expected}, got {got}")]
    SenderRoleMismatch { expected: String, got: String },

    #[error("unsupported envelope version: {0}")]
    UnsupportedVersion(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("keychain error: {0}")]
    Keychain(String),
}

impl From<serde_json::Error> for CryptoError {
    fn from(e: serde_json::Error) -> Self {
        CryptoError::Serialization(e.to_string())
    }
}
