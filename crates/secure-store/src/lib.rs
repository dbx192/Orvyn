//! Secure credential storage abstraction.

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use db_common::{AppError, CredentialRef, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf};
use tokio::task;
use uuid::Uuid;

#[async_trait]
pub trait SecureStore: Send + Sync {
    async fn put_secret(&self, label: &str, secret: &str) -> Result<CredentialRef>;

    async fn get_secret(&self, credential_ref: &CredentialRef) -> Result<Option<String>>;

    async fn delete_secret(&self, credential_ref: &CredentialRef) -> Result<()>;
}

const PROVIDER: &str = "encrypted-file";

/// Encrypted JSON file credential store.
///
/// This is a portable first-stage implementation. Platform keychains can later
/// implement the same trait without changing app-core.
#[derive(Debug, Clone)]
pub struct EncryptedFileStore {
    path: PathBuf,
    key: [u8; 32],
}

impl EncryptedFileStore {
    pub fn new(path: impl Into<PathBuf>, master_key: impl AsRef<[u8]>) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(master_key.as_ref());
        let digest = hasher.finalize();
        let mut key = [0_u8; 32];
        key.copy_from_slice(&digest);

        Self {
            path: path.into(),
            key,
        }
    }

    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(Key::from_slice(&self.key))
    }

    async fn read_file(&self) -> Result<SecretFile> {
        let path = self.path.clone();
        task::spawn_blocking(move || read_secret_file(path))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "secret read task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn write_file(&self, file: SecretFile) -> Result<()> {
        let path = self.path.clone();
        task::spawn_blocking(move || write_secret_file(path, &file))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "secret write task failed")
                    .with_detail(error.to_string())
            })?
    }

    fn encrypt(&self, secret: &str) -> Result<EncryptedSecret> {
        let uuid = Uuid::new_v4();
        let mut nonce_bytes = [0_u8; 12];
        nonce_bytes.copy_from_slice(&uuid.as_bytes()[..12]);
        let cipher = self.cipher();
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), secret.as_bytes())
            .map_err(|_| AppError::new(ErrorKind::Credential, "failed to encrypt secret"))?;

        Ok(EncryptedSecret {
            nonce: STANDARD.encode(nonce_bytes),
            ciphertext: STANDARD.encode(ciphertext),
        })
    }

    fn decrypt(&self, encrypted: &EncryptedSecret) -> Result<String> {
        let nonce = STANDARD
            .decode(&encrypted.nonce)
            .map_err(|error| {
                AppError::new(ErrorKind::Credential, "invalid secret nonce")
                    .with_detail(error.to_string())
            })?;
        let ciphertext = STANDARD
            .decode(&encrypted.ciphertext)
            .map_err(|error| {
                AppError::new(ErrorKind::Credential, "invalid secret ciphertext")
                    .with_detail(error.to_string())
            })?;

        if nonce.len() != 12 {
            return Err(AppError::new(
                ErrorKind::Credential,
                "invalid secret nonce length",
            ));
        }

        let cipher = self.cipher();
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
            .map_err(|_| AppError::new(ErrorKind::Credential, "failed to decrypt secret"))?;

        String::from_utf8(plaintext).map_err(|error| {
            AppError::new(ErrorKind::Credential, "secret is not valid UTF-8")
                .with_detail(error.to_string())
        })
    }
}

#[async_trait]
impl SecureStore for EncryptedFileStore {
    async fn put_secret(&self, label: &str, secret: &str) -> Result<CredentialRef> {
        let mut file = self.read_file().await?;
        let key = Uuid::new_v4().to_string();
        file.secrets.insert(
            key.clone(),
            SecretRecord {
                label: label.to_string(),
                encrypted: self.encrypt(secret)?,
            },
        );
        self.write_file(file).await?;

        Ok(CredentialRef::new(PROVIDER, key))
    }

    async fn get_secret(&self, credential_ref: &CredentialRef) -> Result<Option<String>> {
        if credential_ref.provider != PROVIDER {
            return Ok(None);
        }

        let file = self.read_file().await?;
        let Some(record) = file.secrets.get(&credential_ref.key) else {
            return Ok(None);
        };

        Ok(Some(self.decrypt(&record.encrypted)?))
    }

    async fn delete_secret(&self, credential_ref: &CredentialRef) -> Result<()> {
        if credential_ref.provider != PROVIDER {
            return Ok(());
        }

        let mut file = self.read_file().await?;
        file.secrets.remove(&credential_ref.key);
        self.write_file(file).await
    }
}

fn read_secret_file(path: PathBuf) -> Result<SecretFile> {
    if !path.exists() {
        return Ok(SecretFile::default());
    }

    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Ok(SecretFile::default());
    }

    Ok(serde_json::from_slice(&bytes)?)
}

fn write_secret_file(path: PathBuf, file: &SecretFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(file)?;
    fs::write(path, bytes)?;
    Ok(())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SecretFile {
    #[serde(default)]
    secrets: BTreeMap<String, SecretRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SecretRecord {
    label: String,
    encrypted: EncryptedSecret,
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedSecret {
    nonce: String,
    ciphertext: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stores_and_loads_secret() {
        let path = std::env::temp_dir().join(format!("secure-store-{}.json", Uuid::new_v4()));
        let store = EncryptedFileStore::new(path.clone(), "test-master-key");

        let credential_ref = store.put_secret("local", "secret").await.expect("put secret");
        let secret = store
            .get_secret(&credential_ref)
            .await
            .expect("get secret")
            .expect("secret exists");

        assert_eq!(secret, "secret");
        let _ = fs::remove_file(path);
    }
}
