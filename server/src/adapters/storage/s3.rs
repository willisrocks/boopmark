use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use crate::domain::error::DomainError;
use crate::domain::ports::storage::ObjectStorage;

#[derive(Clone)]
pub struct S3Storage {
    client: Client,
    bucket: String,
    public_url: String,
}

impl S3Storage {
    pub fn new(client: Client, bucket: String, public_url: String) -> Self {
        Self { client, bucket, public_url }
    }
}

impl ObjectStorage for S3Storage {
    async fn put(&self, key: &str, data: Vec<u8>, content_type: &str) -> Result<String, DomainError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| DomainError::Internal(format!("S3 put error: {e}")))?;
        Ok(self.public_url(key))
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, DomainError> {
        let resp = self.client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| DomainError::Internal(format!("S3 get error: {e}")))?;
        let bytes = resp.body.collect().await
            .map_err(|e| DomainError::Internal(format!("S3 read error: {e}")))?;
        Ok(bytes.to_vec())
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| DomainError::Internal(format!("S3 delete error: {e}")))?;
        Ok(())
    }

    fn public_url(&self, key: &str) -> String {
        format!("{}/{}", self.public_url.trim_end_matches('/'), key)
    }
}
