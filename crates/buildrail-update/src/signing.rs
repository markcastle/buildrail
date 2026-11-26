//! The signed message, signing and verification.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::Signer as _;
use sha2::{Digest, Sha256};

use crate::keys::{PublicKey, SigningKey};

/// First line of every signed message; changes if the format ever does.
pub const MESSAGE_TAG: &str = "buildrail-update-v1";

/// Why a signature did not verify.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    /// The signature text is not base64 of 64 bytes.
    #[error("the signature is not well formed")]
    Malformed,
    /// The signature does not match the key and message.
    #[error("the signature does not match")]
    Mismatch,
}

/// The exact bytes signed for one target of one release:
/// `buildrail-update-v1\n{version}\n{target}\n{sha256_hex}\n`.
///
/// Binding the version and target means an old signed payload cannot be
/// relabelled as a newer version, nor one platform's build passed off as
/// another's.
pub fn signed_message(version: &str, target: &str, sha256_hex: &str) -> Vec<u8> {
    format!("{MESSAGE_TAG}\n{version}\n{target}\n{sha256_hex}\n").into_bytes()
}

/// Signs one target's payload; returns the base64 signature for `latest.json`.
pub fn sign(key: &SigningKey, version: &str, target: &str, sha256_hex: &str) -> String {
    let signature = key.0.sign(&signed_message(version, target, sha256_hex));
    STANDARD.encode(signature.to_bytes())
}

/// Verifies a base64 signature over [`signed_message`] (strict Ed25519).
pub fn verify(
    key: &PublicKey,
    version: &str,
    target: &str,
    sha256_hex: &str,
    signature_base64: &str,
) -> Result<(), SignatureError> {
    let bytes = STANDARD
        .decode(signature_base64.trim())
        .map_err(|_| SignatureError::Malformed)?;
    let bytes = <[u8; 64]>::try_from(bytes.as_slice()).map_err(|_| SignatureError::Malformed)?;
    let signature = ed25519_dalek::Signature::from_bytes(&bytes);
    key.0
        .verify_strict(&signed_message(version, target, sha256_hex), &signature)
        .map_err(|_| SignatureError::Mismatch)
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::from_seed(&[3; 32])
    }

    #[test]
    fn message_format_is_pinned() {
        assert_eq!(
            signed_message("0.3.0", "linux-x86_64", "ab12"),
            b"buildrail-update-v1\n0.3.0\nlinux-x86_64\nab12\n".to_vec()
        );
    }

    #[test]
    fn sha256_of_empty_input_is_the_known_value() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sign_then_verify() {
        let hash = sha256_hex(b"payload");
        let sig = sign(&key(), "0.3.0", "linux-x86_64", &hash);
        let public = key().public_key();
        assert_eq!(
            verify(&public, "0.3.0", "linux-x86_64", &hash, &sig),
            Ok(())
        );
    }

    #[test]
    fn any_changed_field_or_key_fails() {
        let hash = sha256_hex(b"payload");
        let sig = sign(&key(), "0.3.0", "linux-x86_64", &hash);
        let public = key().public_key();
        let other = SigningKey::from_seed(&[4; 32]).public_key();
        let mismatch = Err(SignatureError::Mismatch);
        assert_eq!(
            verify(&public, "0.4.0", "linux-x86_64", &hash, &sig),
            mismatch
        );
        assert_eq!(
            verify(&public, "0.3.0", "macos-aarch64", &hash, &sig),
            mismatch
        );
        assert_eq!(
            verify(&public, "0.3.0", "linux-x86_64", &sha256_hex(b"x"), &sig),
            mismatch
        );
        assert_eq!(
            verify(&other, "0.3.0", "linux-x86_64", &hash, &sig),
            mismatch
        );
    }

    #[test]
    fn malformed_signatures_are_reported_as_such() {
        let public = key().public_key();
        let malformed = Err(SignatureError::Malformed);
        assert_eq!(verify(&public, "1.0.0", "t", "h", "***"), malformed);
        assert_eq!(
            verify(&public, "1.0.0", "t", "h", &STANDARD.encode([0u8; 10])),
            malformed
        );
        assert_eq!(
            SignatureError::Mismatch.to_string(),
            "the signature does not match"
        );
    }
}
