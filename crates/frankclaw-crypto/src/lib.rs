#![forbid(unsafe_code)]
#![doc = "Cryptographic primitives for FrankClaw."]
#![doc = ""]
#![doc = "All secret material is wrapped in types that zeroize on drop."]
#![doc = "No raw key bytes are ever exposed in Debug output or logs."]

mod encryption;
mod hashing;
mod keys;
mod token;

pub use encryption::{decrypt, encrypt, EncryptedBlob};
pub use hashing::{hash_password, verify_password, PasswordHash};
pub use keys::{derive_subkey, MasterKey};
pub use token::{generate_token, verify_token_eq};

/// Crypto errors — never leak key material in messages.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("encryption failed")]
    EncryptionFailed,

    #[error("decryption failed (wrong key or corrupted data)")]
    DecryptionFailed,

    #[error("key derivation failed")]
    KeyDerivationFailed,

    #[error("password hashing failed")]
    HashingFailed,

    #[error("password verification failed")]
    VerificationFailed,

    #[error("invalid key length")]
    InvalidKeyLength,
}
