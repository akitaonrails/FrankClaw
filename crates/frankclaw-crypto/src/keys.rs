use argon2::Argon2;
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use zeroize::ZeroizeOnDrop;

use crate::CryptoError;

type HmacSha256 = Hmac<Sha256>;

/// 256-bit master key derived from user passphrase.
/// Zeroed from memory on drop. Never printed in Debug.
#[derive(Clone, ZeroizeOnDrop)]
pub struct MasterKey {
    #[zeroize]
    bytes: [u8; 32],
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey([REDACTED])")
    }
}

impl MasterKey {
    /// Derive a master key from a passphrase using Argon2id.
    ///
    /// Parameters: t=3 iterations, m=64MB memory, p=4 parallelism.
    /// These are OWASP-recommended minimums for interactive logins.
    pub fn from_passphrase(passphrase: &SecretString, salt: &[u8; 16]) -> Result<Self, CryptoError> {
        let params = argon2::Params::new(64 * 1024, 3, 4, Some(32))
            .map_err(|_| CryptoError::KeyDerivationFailed)?;
        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        let mut key_bytes = [0u8; 32];
        argon2
            .hash_password_into(passphrase.expose_secret().as_bytes(), salt, &mut key_bytes)
            .map_err(|_| CryptoError::KeyDerivationFailed)?;

        Ok(Self { bytes: key_bytes })
    }

    /// Create from raw bytes (for testing or loading from secure storage).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    /// Access the raw key bytes. Use sparingly — prefer `derive_subkey`.
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

/// Derive a context-specific subkey using HMAC-SHA256.
///
/// Each context string (e.g., "session", "config", "media") produces
/// a unique 256-bit subkey from the same master key.
///
/// Uses a simple HMAC-based extract-and-expand pattern:
///   PRK = HMAC-SHA256(key=master, data="frankclaw-kdf")
///   OKM = HMAC-SHA256(key=PRK, data=context || 0x01)
pub fn derive_subkey(master: &MasterKey, context: &str) -> Result<[u8; 32], CryptoError> {
    // Extract: PRK = HMAC(key=master, msg="frankclaw-kdf")
    let mut extract = HmacSha256::new_from_slice(master.as_bytes())
        .map_err(|_| CryptoError::KeyDerivationFailed)?;
    extract.update(b"frankclaw-kdf");
    let prk = extract.finalize().into_bytes();

    // Expand: OKM = HMAC(key=PRK, msg=context || 0x01)
    let mut expand = HmacSha256::new_from_slice(&prk)
        .map_err(|_| CryptoError::KeyDerivationFailed)?;
    expand.update(context.as_bytes());
    expand.update(&[0x01]);
    let okm = expand.finalize().into_bytes();

    let mut out = [0u8; 32];
    out.copy_from_slice(&okm);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_different_contexts_produce_different_keys() {
        let master = MasterKey::from_bytes([42u8; 32]);
        let k1 = derive_subkey(&master, "session").unwrap();
        let k2 = derive_subkey(&master, "config").unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn same_context_produces_same_key() {
        let master = MasterKey::from_bytes([42u8; 32]);
        let k1 = derive_subkey(&master, "session").unwrap();
        let k2 = derive_subkey(&master, "session").unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn from_passphrase_works() {
        let passphrase = SecretString::from("test-passphrase-123");
        let salt = [1u8; 16];
        let key = MasterKey::from_passphrase(&passphrase, &salt).unwrap();
        assert_ne!(key.as_bytes(), &[0u8; 32]);
    }
}
