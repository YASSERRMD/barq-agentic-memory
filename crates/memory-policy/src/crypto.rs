//! Content encryption for records at rest in untrusted backends.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryResult};

/// Symmetric content encryption over record text.
///
/// Encryptors apply to `content.text` (and structured payloads) before
/// providers persist; decryption happens after hydration. Keys never
/// leave the encryptor.
#[async_trait]
pub trait Encryptor: Send + Sync {
    fn name(&self) -> &str;

    async fn encrypt(&self, plaintext: &str) -> MemoryResult<Vec<u8>>;
    async fn decrypt(&self, ciphertext: &[u8]) -> MemoryResult<String>;
}

/// Passthrough used by embedded/test deployments.
pub struct NoopEncryptor;

#[async_trait]
impl Encryptor for NoopEncryptor {
    fn name(&self) -> &str {
        "noop"
    }
    async fn encrypt(&self, plaintext: &str) -> MemoryResult<Vec<u8>> {
        Ok(plaintext.as_bytes().to_vec())
    }
    async fn decrypt(&self, ciphertext: &[u8]) -> MemoryResult<String> {
        String::from_utf8(ciphertext.to_vec())
            .map_err(|e| MemoryError::storage("noop", e.to_string()))
    }
}

/// AES-256-GCM with random 96-bit nonces.
///
/// The key is provided at construction (32 bytes). Nonce uniqueness per
/// key is guaranteed by the OS RNG; reusing a `(key, nonce)` pair would
/// break GCM catastrophically, so this type never derives nonces
/// deterministically.
pub struct AesGcmEncryptor {
    cipher: Aes256Gcm,
}

impl AesGcmEncryptor {
    /// Creates an encryptor from a 32-byte key.
    pub fn new(key: &[u8]) -> MemoryResult<Self> {
        if key.len() != 32 {
            return Err(MemoryError::validation("key", "must be exactly 32 bytes"));
        }
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| MemoryError::storage("aes-gcm", e.to_string()))?;
        Ok(Self { cipher })
    }

    /// Generates a fresh key (development convenience).
    pub fn generate_key() -> [u8; 32] {
        use aes_gcm::aead::rand_core::RngCore;
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    }
}

#[async_trait]
impl Encryptor for AesGcmEncryptor {
    fn name(&self) -> &str {
        "aes-256-gcm"
    }

    async fn encrypt(&self, plaintext: &str) -> MemoryResult<Vec<u8>> {
        // 96-bit random nonce prepended to the ciphertext.
        use aes_gcm::aead::rand_core::RngCore;
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| MemoryError::storage("aes-gcm", e.to_string()))?;
        let mut out = nonce_bytes.to_vec();
        out.extend(ct);
        Ok(out)
    }

    async fn decrypt(&self, ciphertext: &[u8]) -> MemoryResult<String> {
        if ciphertext.len() < 12 {
            return Err(MemoryError::validation(
                "ciphertext",
                "shorter than the nonce prefix",
            ));
        }
        let (nonce_bytes, body) = ciphertext.split_at(12);
        let pt = self
            .cipher
            .decrypt(Nonce::from_slice(nonce_bytes), body)
            .map_err(|_| MemoryError::validation("ciphertext", "authentication failed"))?;
        String::from_utf8(pt).map_err(|e| MemoryError::storage("aes-gcm", e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_recovers_plaintext() {
        let key = AesGcmEncryptor::generate_key();
        let enc = AesGcmEncryptor::new(&key).expect("key");
        let ct = enc
            .encrypt("Customer prefers email")
            .await
            .expect("encrypt");
        assert_ne!(ct, b"Customer prefers email");
        assert!(
            enc.encrypt("same input twice").await != enc.encrypt("same input twice").await,
            "random nonces must vary ciphertexts"
        );
        assert_eq!(
            enc.decrypt(&ct).await.expect("decrypt"),
            "Customer prefers email"
        );
    }

    #[tokio::test]
    async fn tampered_ciphertext_fails_authentication() {
        let key = AesGcmEncryptor::generate_key();
        let enc = AesGcmEncryptor::new(&key).unwrap();
        let mut ct = enc.encrypt("do not tamper").await.expect("encrypt");
        let last = ct.len() - 1;
        ct[last] ^= 0xFF;
        assert!(enc.decrypt(&ct).await.is_err());
    }

    #[tokio::test]
    async fn wrong_key_fails_authentication() {
        let enc1 = AesGcmEncryptor::new(&AesGcmEncryptor::generate_key()).unwrap();
        let enc2 = AesGcmEncryptor::new(&AesGcmEncryptor::generate_key()).unwrap();
        let ct = enc1.encrypt("secret").await.unwrap();
        assert!(enc2.decrypt(&ct).await.is_err());
    }

    #[test]
    fn keys_must_be_32_bytes() {
        assert!(AesGcmEncryptor::new(&[0u8; 16]).is_err());
        assert!(AesGcmEncryptor::new(&[0u8; 32]).is_ok());
    }

    #[tokio::test]
    async fn noop_roundtrips_identity() {
        let e = NoopEncryptor;
        let ct = e.encrypt("plain").await.unwrap();
        assert_eq!(e.decrypt(&ct).await.unwrap(), "plain");
    }
}
