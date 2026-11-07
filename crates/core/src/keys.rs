//! WireGuard keys in the text form `wg genkey` and `wg pubkey` use: 32 bytes, standard base64.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use boringtun::x25519::{PublicKey, StaticSecret};

use crate::CoreError;

fn decode32(what: &str, text: &str) -> Result<[u8; 32], CoreError> {
    let bytes = STANDARD
        .decode(text.trim())
        .map_err(|e| CoreError::InvalidKey { reason: format!("{what}: {e}") })?;
    bytes.try_into().map_err(|b: Vec<u8>| CoreError::InvalidKey {
        reason: format!("{what}: {} bytes, expected 32", b.len()),
    })
}

pub fn parse_private_key(text: &str) -> Result<StaticSecret, CoreError> {
    Ok(StaticSecret::from(decode32("private key", text)?))
}

pub fn parse_public_key(text: &str) -> Result<PublicKey, CoreError> {
    Ok(PublicKey::from(decode32("public key", text)?))
}

pub fn encode_key(bytes: &[u8; 32]) -> String {
    STANDARD.encode(bytes)
}

/// A new private key, clamped the way `wg genkey` prints one (X25519 clamps on use anyway).
#[uniffi::export]
pub fn generate_private_key() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's random source failed");
    bytes[0] &= 248;
    bytes[31] &= 127;
    bytes[31] |= 64;
    encode_key(&bytes)
}

/// The public key that goes with `private_key`, for showing on screen and for writing the Mac's config.
#[uniffi::export]
pub fn public_key_from_private(private_key: String) -> Result<String, CoreError> {
    let secret = parse_private_key(&private_key)?;
    Ok(encode_key(PublicKey::from(&secret).as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_derives() {
        let private = generate_private_key();
        let public = public_key_from_private(private.clone()).unwrap();
        assert_eq!(STANDARD.decode(&private).unwrap().len(), 32);
        assert_eq!(parse_public_key(&public).unwrap().as_bytes().len(), 32);
        assert_ne!(private, public);
    }

    #[test]
    fn rejects_wrong_length() {
        let err = parse_public_key(&STANDARD.encode([0u8; 31])).unwrap_err();
        assert!(err.to_string().contains("31 bytes"), "{err}");
    }
}
