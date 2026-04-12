use crate::foundation::{Error, Result};

/// Standalone utility for generating cryptographically secure random tokens.
///
/// No config or container needed — pure functions.
///
/// ```rust
/// use forge::Token;
///
/// let api_key = Token::generate(32)?;     // "aB3xY9z..."
/// let hex_token = Token::hex(16)?;        // "4a7b2c..." (32 hex chars)
/// let b64_token = Token::base64(32)?;     // URL-safe base64
/// ```
pub struct Token;

impl Token {
    /// Generate a random alphanumeric string of the given length.
    /// Uses characters: a-z, A-Z, 0-9 (62 possible chars).
    pub fn generate(length: usize) -> Result<String> {
        let bytes = Self::bytes(length)?;
        let chars = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let result: String = bytes
            .iter()
            .map(|&b| chars[b as usize % chars.len()] as char)
            .collect();
        Ok(result)
    }

    /// Generate cryptographically secure random bytes.
    pub fn bytes(length: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; length];
        getrandom::fill(&mut buf)
            .map_err(|e| Error::message(format!("failed to generate random bytes: {e}")))?;
        Ok(buf)
    }

    /// Generate a random hex string. Output length is `bytes * 2`.
    pub fn hex(bytes: usize) -> Result<String> {
        let raw = Self::bytes(bytes)?;
        Ok(raw.iter().map(|b| format!("{:02x}", b)).collect())
    }

    /// Generate a URL-safe base64 random string (no padding).
    /// Input is the number of random bytes; output length varies.
    pub fn base64(bytes: usize) -> Result<String> {
        let raw = Self::bytes(bytes)?;
        use base64::Engine;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_returns_correct_length() {
        let token = Token::generate(32).unwrap();
        assert_eq!(token.len(), 32);
    }

    #[test]
    fn generate_produces_different_values() {
        let a = Token::generate(32).unwrap();
        let b = Token::generate(32).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn generate_uses_only_alphanumeric() {
        let token = Token::generate(100).unwrap();
        assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn bytes_returns_correct_length() {
        let bytes = Token::bytes(16).unwrap();
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn hex_returns_double_length() {
        let hex = Token::hex(16).unwrap();
        assert_eq!(hex.len(), 32);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn base64_roundtrip() {
        let bytes = Token::bytes(32).unwrap();
        use base64::Engine;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes);
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&encoded)
            .unwrap();
        assert_eq!(bytes, decoded);
    }

    #[test]
    fn zero_length_returns_empty() {
        assert_eq!(Token::generate(0).unwrap(), "");
        assert_eq!(Token::bytes(0).unwrap(), Vec::<u8>::new());
        assert_eq!(Token::hex(0).unwrap(), "");
    }
}
