use crate::error::{StoreError, StoreResult};
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedSecret {
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBox(..)")
    }
}

impl SecretBox {
    pub const KEY_LEN: usize = 32;
    const NONCE_LEN: usize = 12;

    pub fn from_key(key: &[u8]) -> StoreResult<Self> {
        if key.len() != Self::KEY_LEN {
            return Err(StoreError::Crypto(format!(
                "ключ шифрования должен быть {} байта",
                Self::KEY_LEN
            )));
        }
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|e| StoreError::Crypto(e.to_string()))?;
        Ok(SecretBox { cipher })
    }

    pub fn from_base64(encoded: &str) -> StoreResult<Self> {
        let key = STANDARD
            .decode(encoded.trim())
            .map_err(|_| StoreError::Crypto("ключ шифрования должен быть в base64".into()))?;
        Self::from_key(&key)
    }

    pub fn generate_key_base64() -> String {
        STANDARD.encode(Aes256Gcm::generate_key(&mut OsRng))
    }

    pub fn seal(&self, context: &str, plaintext: &[u8]) -> StoreResult<SealedSecret> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| StoreError::Crypto("не удалось зашифровать".into()))?;
        Ok(SealedSecret {
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    pub fn open(&self, context: &str, sealed: &SealedSecret) -> StoreResult<Vec<u8>> {
        if sealed.nonce.len() != Self::NONCE_LEN {
            return Err(StoreError::Crypto("повреждённый nonce".into()));
        }
        self.cipher
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| StoreError::Crypto("ключ не подходит или данные повреждены".into()))
    }

    pub fn seal_str(&self, context: &str, plaintext: &str) -> StoreResult<SealedSecret> {
        self.seal(context, plaintext.as_bytes())
    }

    pub fn open_string(&self, context: &str, sealed: &SealedSecret) -> StoreResult<String> {
        String::from_utf8(self.open(context, sealed)?)
            .map_err(|_| StoreError::Crypto("секрет не в UTF-8".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn секрет_шифруется_и_привязан_к_ключу_настройки() {
        let key = SecretBox::generate_key_base64();
        let sealed_box = SecretBox::from_base64(&key).unwrap();
        let sealed = sealed_box
            .seal_str("payments.cryptopay.api_token", "123:abc")
            .unwrap();
        assert_ne!(sealed.ciphertext, b"123:abc");
        assert_eq!(
            sealed_box
                .open_string("payments.cryptopay.api_token", &sealed)
                .unwrap(),
            "123:abc"
        );
        assert!(sealed_box.open_string("remnawave.token", &sealed).is_err());
        let other = SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap();
        assert!(
            other
                .open_string("payments.cryptopay.api_token", &sealed)
                .is_err()
        );
    }

    #[test]
    fn ключ_неверной_длины_не_принимается() {
        assert!(SecretBox::from_key(&[0u8; 16]).is_err());
        assert!(SecretBox::from_base64("не base64").is_err());
    }
}
