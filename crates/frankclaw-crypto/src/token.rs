use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;

/// Generate a cryptographically secure random token (256 bits, base64url encoded).
///
/// Suitable for: gateway auth tokens, webhook secrets, pairing codes.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Constant-time comparison of two token strings.
///
/// Prevents timing attacks on token validation. Both tokens are compared
/// byte-by-byte in fixed time regardless of where they differ.
pub fn verify_token_eq(provided: &str, expected: &str) -> bool {
    // Length check is not constant-time, but token length is not secret
    // (all generated tokens are the same length).
    if provided.len() != expected.len() {
        return false;
    }
    constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

/// Constant-time byte slice comparison.
/// XORs all bytes and checks if the result is zero.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_unique() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2);
    }

    #[test]
    fn generated_token_length() {
        let t = generate_token();
        // 32 bytes → 43 base64url chars (no padding)
        assert_eq!(t.len(), 43);
    }

    #[test]
    fn verify_same_token() {
        let t = generate_token();
        assert!(verify_token_eq(&t, &t));
    }

    #[test]
    fn verify_different_tokens() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert!(!verify_token_eq(&t1, &t2));
    }

    #[test]
    fn verify_different_lengths() {
        assert!(!verify_token_eq("short", "longer-string"));
    }
}
