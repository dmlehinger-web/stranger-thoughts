use crate::ids::ReaderId;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// A mailing address. Debug output is redacted so a log of this value cannot
/// carry the street.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostalAddress {
    pub line1: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line2: Option<String>,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
}

impl fmt::Debug for PostalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PostalAddress([redacted])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedAddress {
    pub reader_id: String,
    pub ciphertext: String,
    pub encrypted_data_key: String,
    pub nonce: String,
}

impl fmt::Debug for SealedAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SealedAddress {{ reader_id: {}, ciphertext_len: {}, wrapped_key_len: {} }}",
            self.reader_id,
            self.ciphertext.len(),
            self.encrypted_data_key.len()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("encryption failed")]
    Failed,
}

/// 32-byte AES key that zeroes itself on drop.
pub struct PlainDataKey(Vec<u8>);

impl PlainDataKey {
    pub fn new(bytes: Vec<u8>) -> Result<Self, CryptoError> {
        if bytes.len() == 32 {
            Ok(Self(bytes))
        } else {
            Err(CryptoError::Failed)
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for PlainDataKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

fn aad(reader_id: &ReaderId) -> Vec<u8> {
    format!("st-address-v1:{}", reader_id.as_str()).into_bytes()
}

pub fn seal_address(
    reader_id: &ReaderId,
    address: &PostalAddress,
    data_key: &PlainDataKey,
    encrypted_data_key: &[u8],
) -> Result<SealedAddress, CryptoError> {
    if encrypted_data_key.is_empty() {
        return Err(CryptoError::Failed);
    }
    let plaintext = serde_json::to_vec(address).map_err(|_| CryptoError::Failed)?;
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let cipher = Aes256Gcm::new_from_slice(data_key.as_bytes()).map_err(|_| CryptoError::Failed)?;
    let bound = aad(reader_id);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &plaintext,
                aad: &bound,
            },
        )
        .map_err(|_| CryptoError::Failed)?;
    Ok(SealedAddress {
        reader_id: reader_id.as_str().to_string(),
        ciphertext: STANDARD.encode(ciphertext),
        encrypted_data_key: STANDARD.encode(encrypted_data_key),
        nonce: STANDARD.encode(nonce_bytes),
    })
}

pub fn open_address(
    reader_id: &ReaderId,
    sealed: &SealedAddress,
    data_key: &PlainDataKey,
) -> Result<PostalAddress, CryptoError> {
    if sealed.reader_id != reader_id.as_str() {
        return Err(CryptoError::Failed);
    }
    let ciphertext = STANDARD.decode(&sealed.ciphertext).map_err(|_| CryptoError::Failed)?;
    let nonce = STANDARD.decode(&sealed.nonce).map_err(|_| CryptoError::Failed)?;
    if nonce.len() != 12 {
        return Err(CryptoError::Failed);
    }
    let cipher = Aes256Gcm::new_from_slice(data_key.as_bytes()).map_err(|_| CryptoError::Failed)?;
    let bound = aad(reader_id);
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: &bound,
            },
        )
        .map_err(|_| CryptoError::Failed)?;
    serde_json::from_slice(&plaintext).map_err(|_| CryptoError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address() -> PostalAddress {
        PostalAddress {
            line1: "PO Box 000".into(),
            line2: None,
            city: "Example City".into(),
            region: "ST".into(),
            postal_code: "00000".into(),
            country: "US".into(),
        }
    }

    fn key() -> PlainDataKey {
        PlainDataKey::new(vec![7u8; 32]).unwrap()
    }

    #[test]
    fn round_trip_and_binding() {
        let reader = ReaderId::new("reader-cedar").unwrap();
        let other = ReaderId::new("reader-other").unwrap();
        let sealed = seal_address(&reader, &address(), &key(), b"wrapped-key").unwrap();
        let opened = open_address(&reader, &sealed, &key()).unwrap();
        assert_eq!(opened.line1, "PO Box 000");
        assert!(open_address(&other, &sealed, &key()).is_err());

        let raw = serde_json::to_string(&sealed).unwrap();
        assert!(!raw.contains("PO Box"));
        assert!(!raw.contains("Example City"));
        assert!(!format!("{sealed:?}").contains("PO Box"));
        assert_eq!(format!("{:?}", address()), "PostalAddress([redacted])");
    }

    #[test]
    fn tampered_ciphertext_fails_closed() {
        let reader = ReaderId::new("reader-cedar").unwrap();
        let mut sealed = seal_address(&reader, &address(), &key(), b"wrapped-key").unwrap();
        let mut bytes = STANDARD.decode(&sealed.ciphertext).unwrap();
        bytes[0] ^= 0xff;
        sealed.ciphertext = STANDARD.encode(bytes);
        assert!(open_address(&reader, &sealed, &key()).is_err());
    }
}
