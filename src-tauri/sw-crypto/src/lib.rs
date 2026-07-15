pub mod amk;
pub mod envelope;
pub mod errors;
pub mod file_crypto;
pub mod identity;
pub mod kdf;
pub mod object_envelope;
pub mod recovery;

pub use amk::{
    self_unwrap_amk, self_wrap_amk, AccountMasterKey, DeviceIdentity, SelfWrappedAmk, AMK_LEN,
};

pub use envelope::{
    open, open_checked, open_json, open_json_checked, seal, seal_json, BYOEnvelope, ContentType,
    ReplayGuard, SenderRole, ENVELOPE_VERSION,
};
pub use errors::CryptoError;
pub use file_crypto::{decrypt_bytes, decrypt_file, encrypt_bytes, encrypt_file};
pub use identity::{generate_local_root_key, IdentityKeypair, IdentityPublicKey};
pub use kdf::{
    derive_audio_key, derive_cache_key, derive_content_wrapping_key, derive_db_key,
    derive_envelope_key, derive_file_key, derive_index_key, derive_key_with_context,
};
pub use object_envelope::{
    decrypt_object, encrypt_object, encrypt_object_with, unwrap_amk_for_recipient,
    wrap_amk_for_recipient, ObjectEnvelope, ObjectEnvelopeHeader, ObjectMetadata,
    ALGORITHM_XCHACHA20_POLY1305, OBJECT_ENVELOPE_VERSION,
};
pub use recovery::{
    derive_recovery_key, generate_bip39_passphrase, unwrap_amk_with_passphrase,
    wrap_amk_with_passphrase, RecoveryKdfParams,
};
