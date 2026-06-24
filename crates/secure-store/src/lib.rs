//! Secure credential storage abstraction.

use async_trait::async_trait;
use db_common::{CredentialRef, Result};

#[async_trait]
pub trait SecureStore: Send + Sync {
    async fn put_secret(&self, label: &str, secret: &str) -> Result<CredentialRef>;

    async fn get_secret(&self, credential_ref: &CredentialRef) -> Result<Option<String>>;

    async fn delete_secret(&self, credential_ref: &CredentialRef) -> Result<()>;
}
