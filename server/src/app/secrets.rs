use base64::Engine;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

pub struct SecretBox {
    key: LessSafeKey,
    random: SystemRandom,
}

impl SecretBox {
    pub fn new(raw_key: &str) -> Self {
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(raw_key.trim())
            .expect("LLM_SETTINGS_ENCRYPTION_KEY must be valid base64");
        assert_eq!(
            key_bytes.len(),
            32,
            "LLM_SETTINGS_ENCRYPTION_KEY must decode to 32 bytes"
        );

        let unbound_key = UnboundKey::new(&AES_256_GCM, &key_bytes)
            .expect("LLM_SETTINGS_ENCRYPTION_KEY must initialize AES-256-GCM");

        Self {
            key: LessSafeKey::new(unbound_key),
            random: SystemRandom::new(),
        }
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>, String> {
        let mut nonce_bytes = [0u8; ring::aead::NONCE_LEN];
        self.random
            .fill(&mut nonce_bytes)
            .map_err(|_| "failed to generate nonce".to_string())?;

        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut ciphertext = plaintext.as_bytes().to_vec();
        self.key
            .seal_in_place_append_tag(nonce, Aad::empty(), &mut ciphertext)
            .map_err(|_| "failed to encrypt secret".to_string())?;

        let mut blob = nonce_bytes.to_vec();
        blob.extend(ciphertext);
        Ok(blob)
    }

    pub fn decrypt(&self, blob: &[u8]) -> Result<String, String> {
        if blob.len() < ring::aead::NONCE_LEN {
            return Err("encrypted secret is truncated".to_string());
        }

        let (nonce_bytes, ciphertext) = blob.split_at(ring::aead::NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce_bytes)
            .map_err(|_| "encrypted secret has invalid nonce".to_string())?;
        let mut plaintext = ciphertext.to_vec();
        let opened = self
            .key
            .open_in_place(nonce, Aad::empty(), &mut plaintext)
            .map_err(|_| "failed to decrypt secret".to_string())?;

        String::from_utf8(opened.to_vec()).map_err(|_| "decrypted secret is not utf-8".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_box_round_trips_without_exposing_plaintext() {
        let secret_box = SecretBox::new("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=");
        let plaintext = "sk-ant-example";

        let encrypted = secret_box.encrypt(plaintext).expect("encrypt");

        assert_ne!(encrypted, plaintext.as_bytes());
        assert_eq!(secret_box.decrypt(&encrypted).expect("decrypt"), plaintext);
    }
}
