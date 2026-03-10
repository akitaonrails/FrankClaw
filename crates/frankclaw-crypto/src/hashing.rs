use argon2::{
    password_hash::{PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::rngs::OsRng;
use secrecy::ExposeSecret;

use crate::CryptoError;

/// Opaque password hash string (Argon2id PHC format).
#[derive(Debug, Clone)]
pub struct PasswordHash(String);

impl PasswordHash {
    /// Get the PHC-format hash string for storage.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wrap a previously stored PHC hash string.
    pub fn from_stored(s: String) -> Self {
        Self(s)
    }
}

/// Hash a password with Argon2id (t=3, m=64MB, p=4).
///
/// Returns a PHC-format string suitable for database storage.
/// Uses OS random for salt generation.
pub fn hash_password(password: &secrecy::SecretString) -> Result<PasswordHash, CryptoError> {
    let salt = SaltString::generate(&mut OsRng);
    let params = argon2::Params::new(64 * 1024, 3, 4, None)
        .map_err(|_| CryptoError::HashingFailed)?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let hash = argon2
        .hash_password(password.expose_secret().as_bytes(), &salt)
        .map_err(|_| CryptoError::HashingFailed)?;

    Ok(PasswordHash(hash.to_string()))
}

/// Verify a password against a stored Argon2id hash.
///
/// Uses constant-time comparison internally (provided by argon2 crate).
pub fn verify_password(
    password: &secrecy::SecretString,
    hash: &PasswordHash,
) -> Result<bool, CryptoError> {
    let parsed = argon2::password_hash::PasswordHash::new(hash.as_str())
        .map_err(|_| CryptoError::VerificationFailed)?;

    let argon2 = Argon2::default();
    match argon2.verify_password(password.expose_secret().as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(_) => Err(CryptoError::VerificationFailed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;

    #[test]
    fn hash_and_verify() {
        let pw = SecretString::from("my-secure-password");
        let hash = hash_password(&pw).unwrap();
        assert!(verify_password(&pw, &hash).unwrap());
    }

    #[test]
    fn wrong_password_fails() {
        let pw = SecretString::from("correct");
        let wrong = SecretString::from("wrong");
        let hash = hash_password(&pw).unwrap();
        assert!(!verify_password(&wrong, &hash).unwrap());
    }

    #[test]
    fn hash_format_is_phc() {
        let pw = SecretString::from("test");
        let hash = hash_password(&pw).unwrap();
        assert!(hash.as_str().starts_with("$argon2id$"));
    }
}
