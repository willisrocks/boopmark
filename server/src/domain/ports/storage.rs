use crate::domain::error::DomainError;

#[trait_variant::make(Send)]
pub trait ObjectStorage: Send + Sync {
    async fn put(&self, key: &str, data: Vec<u8>, content_type: &str) -> Result<String, DomainError>;
    async fn get(&self, key: &str) -> Result<Vec<u8>, DomainError>;
    async fn delete(&self, key: &str) -> Result<(), DomainError>;
    fn public_url(&self, key: &str) -> String;
}
