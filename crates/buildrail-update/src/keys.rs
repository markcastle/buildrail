//! Ed25519 keys: parsing, encoding, key ids, and the key embedded at build time.

use std::fmt;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

/// Environment variable holding the base64 signing seed (release workflow only).
pub const SIGNING_KEY_ENV: &str = "BUILDRAIL_UPDATE_SIGNING_KEY";

/// Environment variable holding the base64 public key.
///
/// Read at *compile* time by [`embedded_public_key`], and at run time by
/// `buildrail-sign verify`.
pub const PUBLIC_KEY_ENV: &str = "BUILDRAIL_UPDATE_PUBLIC_KEY";

/// Why a key could not be read. Never contains the key material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The text is not standard base64.
    #[error("the key is not valid base64")]
    NotBase64,
    /// The decoded key is not 32 bytes long.
    #[error("the key must be 32 bytes, found {0}")]
    WrongLength(usize),
    /// The bytes are not a valid Ed25519 public key.
    #[error("the key is not a valid Ed25519 public key")]
    InvalidPoint,
}

fn decode_32(text: &str) -> Result<[u8; 32], KeyError> {
    let bytes = STANDARD
        .decode(text.trim())
        .map_err(|_| KeyError::NotBase64)?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| KeyError::WrongLength(bytes.len()))
}

/// An Ed25519 public key that verifies update signatures.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey(pub(crate) ed25519_dalek::VerifyingKey);

impl PublicKey {
    /// Parses a standard-base64 public key (surrounding whitespace is ignored).
    pub fn from_base64(text: &str) -> Result<Self, KeyError> {
        Self::from_bytes(&decode_32(text)?)
    }

    /// Builds a key from its 32 raw bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, KeyError> {
        ed25519_dalek::VerifyingKey::from_bytes(bytes)
            .map(Self)
            .map_err(|_| KeyError::InvalidPoint)
    }

    /// The 32 raw bytes.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Standard base64 of the raw bytes.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.to_bytes())
    }

    /// First 16 lowercase hex characters of SHA-256 of the raw key bytes.
    ///
    /// Written into `latest.json` so a copy built with another key can say
    /// "signed with a different key" instead of a generic failure.
    pub fn key_id(&self) -> String {
        let digest = Sha256::digest(self.to_bytes());
        let mut hex = crate::signing::to_hex(&digest);
        hex.truncate(16);
        hex
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.key_id())
    }
}

/// An Ed25519 signing key, made from a 32-byte seed.
///
/// `Debug` shows only the key id; the seed is never printed.
#[derive(Clone)]
pub struct SigningKey(pub(crate) ed25519_dalek::SigningKey);

impl SigningKey {
    /// Parses a standard-base64 seed (surrounding whitespace, such as the
    /// newline a secrets store may keep, is ignored).
    pub fn from_base64_seed(text: &str) -> Result<Self, KeyError> {
        Ok(Self::from_seed(&decode_32(text)?))
    }

    /// Builds the key from its 32-byte seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(seed))
    }

    /// The matching public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SigningKey(key id {}, seed hidden)",
            self.public_key().key_id()
        )
    }
}

/// Encodes a 32-byte seed as standard base64 (what `buildrail-sign generate` prints).
pub fn encode_seed(seed: &[u8; 32]) -> String {
    STANDARD.encode(seed)
}

/// The public key compiled into this build from `BUILDRAIL_UPDATE_PUBLIC_KEY`.
///
/// `None` when the variable was absent, empty or not a valid key at compile
/// time; such a build never installs updates.
pub fn embedded_public_key() -> Option<PublicKey> {
    parse_embedded_public_key(option_env!("BUILDRAIL_UPDATE_PUBLIC_KEY"))
}

/// The rule behind [`embedded_public_key`]: absent, blank or invalid → `None`.
pub fn parse_embedded_public_key(value: Option<&str>) -> Option<PublicKey> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| PublicKey::from_base64(text).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [7; 32];

    #[test]
    fn seed_round_trips_through_base64() {
        let text = encode_seed(&SEED);
        let key = SigningKey::from_base64_seed(&format!("{text}\n")).unwrap();
        assert_eq!(key.public_key(), SigningKey::from_seed(&SEED).public_key());
    }

    #[test]
    fn public_key_round_trips_and_has_a_16_char_id() {
        let public = SigningKey::from_seed(&SEED).public_key();
        let parsed = PublicKey::from_base64(&public.to_base64()).unwrap();
        assert_eq!(parsed, public);
        let id = public.key_id();
        assert_eq!(id.len(), 16);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // Pin the definition: SHA-256 of the raw public key bytes.
        let full = crate::signing::sha256_hex(&public.to_bytes());
        assert_eq!(id, full[..16]);
    }

    #[test]
    fn bad_keys_are_rejected_without_echoing_them() {
        assert_eq!(
            PublicKey::from_base64("not base64!"),
            Err(KeyError::NotBase64)
        );
        assert_eq!(
            PublicKey::from_base64(&STANDARD.encode([1u8; 16])),
            Err(KeyError::WrongLength(16))
        );
        assert!(SigningKey::from_base64_seed("%%%").is_err());
        // Not a point on the curve.
        let mut bad = [0u8; 32];
        bad[0] = 2;
        bad[31] = 0x80;
        let err = (0u8..=255)
            .filter_map(|b| {
                bad[1] = b;
                PublicKey::from_bytes(&bad).err()
            })
            .next();
        assert_eq!(err, Some(KeyError::InvalidPoint));
        assert_eq!(
            KeyError::WrongLength(3).to_string(),
            "the key must be 32 bytes, found 3"
        );
    }

    #[test]
    fn debug_never_shows_the_seed() {
        let key = SigningKey::from_seed(&SEED);
        let shown = format!("{key:?}");
        assert!(shown.contains("seed hidden"));
        assert!(!shown.contains(&encode_seed(&SEED)));
        assert!(format!("{:?}", key.public_key()).starts_with("PublicKey("));
    }

    #[test]
    fn embedded_key_rule() {
        let public = SigningKey::from_seed(&SEED).public_key().to_base64();
        assert_eq!(parse_embedded_public_key(None), None);
        assert_eq!(parse_embedded_public_key(Some("")), None);
        assert_eq!(parse_embedded_public_key(Some("  ")), None);
        assert_eq!(parse_embedded_public_key(Some("garbage")), None);
        assert!(parse_embedded_public_key(Some(&public)).is_some());
        // Tests are built without the variable.
        assert_eq!(
            embedded_public_key(),
            parse_embedded_public_key(option_env!("BUILDRAIL_UPDATE_PUBLIC_KEY"))
        );
    }
}
