//! Start tokens from a site in front of this server (serveme.tf): it signs the user in and decides
//! what they may watch, then hands the browser a short-lived token for us. The token is
//! AES-256-GCM encrypted, so the `SourceTV` password inside it is never readable in a URL:
//! base64url(12-byte nonce | ciphertext | 16-byte tag) of `{"connect", "owner", "exp"}`.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

/// What a site's token says: the match to watch, for whom, and until when (Unix seconds).
#[derive(Deserialize)]
struct Claims {
    connect: String,
    owner: Option<String>,
    exp: u64,
}

#[derive(Debug)]
pub struct Start {
    pub connect: String,
    /// None: unlimited (admins, casters); otherwise one match at a time for this user.
    pub owner: Option<String>,
}

pub struct Keys(Aes256Gcm);

/// AES-256-GCM: the key, and the nonce before and tag after the ciphertext in a token.
const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;

impl Keys {
    /// The shared key as 64 hex digits.
    pub fn from_hex(hex: &str) -> Result<Self> {
        let hex = hex.trim();
        // Checked first: slicing a non-ASCII key into pairs of bytes would panic.
        if hex.len() != 2 * KEY_BYTES || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("the token key must be {} hex digits ({KEY_BYTES} bytes)", 2 * KEY_BYTES);
        }
        let bytes = (0..KEY_BYTES)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| anyhow!("the token key must be hex"))?;
        Ok(Keys(Aes256Gcm::new_from_slice(&bytes).map_err(|_| anyhow!("bad token key"))?))
    }

    /// 12-byte random nonce | ciphertext | tag, the layout of the tokens.
    pub fn seal(&self, plain: &[u8]) -> Vec<u8> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut out = nonce.to_vec();
        out.extend(self.0.encrypt(&nonce, plain).expect("AES-GCM encryption of an in-memory buffer"));
        out
    }

    pub fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < NONCE_BYTES + TAG_BYTES {
            bail!("too short");
        }
        let (nonce, data) = sealed.split_at(NONCE_BYTES);
        self.0.decrypt(Nonce::from_slice(nonce), data).map_err(|_| anyhow!("does not decrypt with this key"))
    }

    pub fn open(&self, token: &str, now: u64) -> Result<Start> {
        let raw = URL_SAFE_NO_PAD.decode(token.trim()).map_err(|_| anyhow!("malformed token"))?;
        let plain = self.unseal(&raw).map_err(|_| anyhow!("invalid token"))?;
        let claims: Claims = serde_json::from_slice(&plain).map_err(|_| anyhow!("invalid token"))?;
        if claims.exp < now {
            bail!("this link has expired; open the match again from the site");
        }
        Ok(Start { connect: claims.connect, owner: claims.owner })
    }
}

/// A token as the site makes them, for tests.
#[cfg(test)]
pub fn seal(key_hex: &str, json: &str, nonce: [u8; 12]) -> String {
    let keys = Keys::from_hex(key_hex).unwrap();
    let mut raw = nonce.to_vec();
    raw.extend(keys.0.encrypt(Nonce::from_slice(&nonce), json.as_bytes()).unwrap());
    URL_SAFE_NO_PAD.encode(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    fn seal(json: &str, nonce: [u8; 12]) -> String {
        super::seal(KEY, json, nonce)
    }

    #[test]
    fn opens_a_valid_token() {
        let t = seal(r#"{"connect":"connect 1.2.3.4:27020; password \"tv\"","owner":"76561197960497430","exp":2000}"#, [7; 12]);
        let s = Keys::from_hex(KEY).unwrap().open(&t, 1000).unwrap();
        assert_eq!(s.connect, r#"connect 1.2.3.4:27020; password "tv""#);
        assert_eq!(s.owner.as_deref(), Some("76561197960497430"));
        let unlimited = seal(r#"{"connect":"1.2.3.4:27020","owner":null,"exp":2000}"#, [8; 12]);
        assert!(Keys::from_hex(KEY).unwrap().open(&unlimited, 1000).unwrap().owner.is_none());
    }

    #[test]
    fn seals_and_unseals() {
        let keys = Keys::from_hex(KEY).unwrap();
        let a = keys.seal(b"secret");
        assert_ne!(a, keys.seal(b"secret"), "fresh nonce each time");
        assert_eq!(keys.unseal(&a).unwrap(), b"secret");
        assert!(Keys::from_hex(&"ee".repeat(32)).unwrap().unseal(&a).is_err());
    }

    #[test]
    fn refuses_expired_tampered_and_foreign_tokens() {
        let keys = Keys::from_hex(KEY).unwrap();
        let t = seal(r#"{"connect":"1.2.3.4:27020","exp":1000}"#, [1; 12]);
        assert!(keys.open(&t, 1001).unwrap_err().to_string().contains("expired"));
        let mut raw = URL_SAFE_NO_PAD.decode(&t).unwrap();
        raw[20] ^= 1;
        assert!(keys.open(&URL_SAFE_NO_PAD.encode(raw), 0).is_err());
        let other = Keys::from_hex(&"ff".repeat(32)).unwrap();
        assert!(other.open(&t, 0).is_err());
        assert!(keys.open("not a token", 0).is_err());
        for incomplete in [r#"{"exp":2000}"#, r#"{"connect":"1.2.3.4:27020"}"#, r#"{"connect":7,"exp":2000}"#] {
            assert!(keys.open(&seal(incomplete, [2; 12]), 0).is_err(), "{incomplete}");
        }
        assert!(Keys::from_hex("abc").is_err());
        assert!(Keys::from_hex(&format!("é{}", "0".repeat(62))).is_err(), "non-ASCII, 64 bytes long");
    }
}
