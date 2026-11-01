use crate::domain::error::DomainError;
use crate::domain::ports::storage::{ObjectStorage, is_plain_object_key};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct LocalStorage {
    base_dir: PathBuf,
    public_url_prefix: String,
}

impl LocalStorage {
    pub fn new(base_dir: PathBuf, public_url_prefix: String) -> Self {
        std::fs::create_dir_all(&base_dir).ok();
        Self {
            base_dir,
            public_url_prefix,
        }
    }

    /// The file for `key`. Only plain relative keys are accepted, and the
    /// resolved path (symlinks followed, as far as it exists) must stay
    /// inside `base_dir`.
    async fn path_for(&self, key: &str) -> Result<PathBuf, DomainError> {
        if !is_plain_object_key(key) {
            return Err(DomainError::InvalidInput(format!(
                "invalid storage key: {key:?}"
            )));
        }
        let path = self.base_dir.join(key);
        let base = tokio::fs::canonicalize(&self.base_dir)
            .await
            .map_err(|e| DomainError::Internal(format!("storage dir error: {e}")))?;
        // The deepest part of the path that exists decides where it really is.
        let mut existing: &Path = &path;
        let resolved = loop {
            match tokio::fs::canonicalize(existing).await {
                Ok(resolved) => break resolved,
                Err(_) => match existing.parent() {
                    Some(parent) => existing = parent,
                    None => break base.clone(),
                },
            }
        };
        if !resolved.starts_with(&base) {
            return Err(DomainError::InvalidInput(format!(
                "storage key leaves the storage directory: {key:?}"
            )));
        }
        Ok(path)
    }
}

impl ObjectStorage for LocalStorage {
    async fn put(
        &self,
        key: &str,
        data: Vec<u8>,
        _content_type: &str,
    ) -> Result<String, DomainError> {
        let path = self.path_for(key).await?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| DomainError::Internal(format!("mkdir error: {e}")))?;
        }
        tokio::fs::write(&path, &data)
            .await
            .map_err(|e| DomainError::Internal(format!("write error: {e}")))?;
        Ok(self.public_url(key))
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, DomainError> {
        tokio::fs::read(self.path_for(key).await?)
            .await
            .map_err(|e| DomainError::Internal(format!("read error: {e}")))
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        tokio::fs::remove_file(self.path_for(key).await?)
            .await
            .map_err(|e| DomainError::Internal(format!("delete error: {e}")))?;
        Ok(())
    }

    fn public_url(&self, key: &str) -> String {
        format!("{}/{}", self.public_url_prefix.trim_end_matches('/'), key)
    }

    async fn exists(&self, key: &str) -> Result<bool, DomainError> {
        let path = self.path_for(key).await?;
        Ok(tokio::fs::metadata(path)
            .await
            .is_ok_and(|meta| meta.is_file()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(dir: &Path) -> LocalStorage {
        LocalStorage::new(dir.join("uploads"), "http://localhost:4000/uploads".into())
    }

    #[tokio::test]
    async fn stores_reads_and_checks_plain_keys() {
        let dir = tempdir();
        let storage = storage(&dir);
        let url = storage
            .put("images/base/u/a.jpg", vec![1, 2], "image/jpeg")
            .await
            .unwrap();
        assert_eq!(url, "http://localhost:4000/uploads/images/base/u/a.jpg");
        assert_eq!(
            storage.get("images/base/u/a.jpg").await.unwrap(),
            vec![1, 2]
        );
        assert!(storage.exists("images/base/u/a.jpg").await.unwrap());
        assert!(!storage.exists("images/base/u/missing.jpg").await.unwrap());
        storage.delete("images/base/u/a.jpg").await.unwrap();
        assert!(!storage.exists("images/base/u/a.jpg").await.unwrap());
    }

    #[tokio::test]
    async fn refuses_keys_that_leave_the_storage_directory() {
        let dir = tempdir();
        std::fs::write(dir.join("secret.txt"), b"outside").unwrap();
        let storage = storage(&dir);
        for key in [
            "../secret.txt",
            "images/../../secret.txt",
            "images/%2e%2e/%2e%2e/secret.txt",
            "/etc/passwd",
            "images\\..\\..\\secret.txt",
        ] {
            assert!(
                matches!(storage.get(key).await, Err(DomainError::InvalidInput(_))),
                "get {key}"
            );
            assert!(storage.exists(key).await.is_err(), "exists {key}");
            assert!(
                storage.put(key, vec![0], "image/png").await.is_err(),
                "put {key}"
            );
        }
        // A symlink inside the directory cannot point the key outside it.
        std::fs::create_dir_all(dir.join("uploads/images")).unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("uploads/images/escape")).unwrap();
        assert!(matches!(
            storage.get("images/escape/secret.txt").await,
            Err(DomainError::InvalidInput(_))
        ));
    }

    /// A fresh directory under the system temp dir, unique per test.
    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("boopmark-local-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
