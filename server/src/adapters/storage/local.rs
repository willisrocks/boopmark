use crate::domain::error::DomainError;
use crate::domain::ports::storage::ObjectStorage;
use std::path::PathBuf;

#[derive(Clone)]
pub struct LocalStorage {
    base_dir: PathBuf,
    public_url_prefix: String,
}

impl LocalStorage {
    pub fn new(base_dir: PathBuf, public_url_prefix: String) -> Self {
        std::fs::create_dir_all(&base_dir).ok();
        Self { base_dir, public_url_prefix }
    }
}

impl ObjectStorage for LocalStorage {
    async fn put(&self, key: &str, data: Vec<u8>, _content_type: &str) -> Result<String, DomainError> {
        let path = self.base_dir.join(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| DomainError::Internal(format!("mkdir error: {e}")))?;
        }
        tokio::fs::write(&path, &data).await
            .map_err(|e| DomainError::Internal(format!("write error: {e}")))?;
        Ok(self.public_url(key))
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, DomainError> {
        tokio::fs::read(self.base_dir.join(key)).await
            .map_err(|e| DomainError::Internal(format!("read error: {e}")))
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        tokio::fs::remove_file(self.base_dir.join(key)).await
            .map_err(|e| DomainError::Internal(format!("delete error: {e}")))?;
        Ok(())
    }

    fn public_url(&self, key: &str) -> String {
        format!("{}/{}", self.public_url_prefix.trim_end_matches('/'), key)
    }
}
