use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::CryptoError;

/// Encrypted blob: nonce + ciphertext. Safe to store/transmit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedBlob {
    /// 96-bit random nonce (never reused with same key).
    pub nonce: [u8; 12],
    /// ChaCha20-Poly1305 ciphertext with 128-bit authentication tag.
    pub ciphertext: Vec<u8>,
}

/// Encrypt plaintext with a 256-bit key using ChaCha20-Poly1305.
///
/// Each call generates a fresh random nonce. The nonce is stored
/// alongside the ciphertext for decryption.
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<EncryptedBlob, CryptoError> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CryptoError::InvalidKeyLength)?;

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::EncryptionFailed)?;

    Ok(EncryptedBlob {
        nonce: nonce_bytes,
        ciphertext,
    })
}

/// Decrypt an `EncryptedBlob` with the same 256-bit key used to encrypt it.
///
/// Returns the plaintext bytes. Zeroizes the intermediate buffer on error.
pub fn decrypt(key: &[u8; 32], blob: &EncryptedBlob) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CryptoError::InvalidKeyLength)?;

    let nonce = Nonce::from_slice(&blob.nonce);

    let mut plaintext = cipher
        .decrypt(nonce, blob.ciphertext.as_ref())
        .map_err(|_| CryptoError::DecryptionFailed)?;

    // If we return Ok, caller owns the plaintext.
    // On error path, zeroize before returning.
    if plaintext.is_empty() {
        plaintext.zeroize();
    }

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = [99u8; 32];
        let plaintext = b"hello, frankclaw!";
        let blob = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &blob).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let blob = encrypt(&key1, b"secret").unwrap();
        assert!(decrypt(&key2, &blob).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = [99u8; 32];
        let mut blob = encrypt(&key, b"secret").unwrap();
        if let Some(byte) = blob.ciphertext.first_mut() {
            *byte ^= 0xFF;
        }
        assert!(decrypt(&key, &blob).is_err());
    }

    #[test]
    fn unique_nonces() {
        let key = [99u8; 32];
        let b1 = encrypt(&key, b"same").unwrap();
        let b2 = encrypt(&key, b"same").unwrap();
        assert_ne!(b1.nonce, b2.nonce);
        assert_ne!(b1.ciphertext, b2.ciphertext);
    }
}
