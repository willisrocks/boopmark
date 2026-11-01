use crate::domain::error::DomainError;

#[allow(dead_code)]
#[trait_variant::make(Send)]
pub trait ObjectStorage: Send + Sync {
    async fn put(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> Result<String, DomainError>;
    async fn get(&self, key: &str) -> Result<Vec<u8>, DomainError>;
    async fn delete(&self, key: &str) -> Result<(), DomainError>;
    fn public_url(&self, key: &str) -> String;

    /// Whether an object is stored under `key`, without reading it.
    async fn exists(&self, key: &str) -> Result<bool, DomainError>;
}

/// Whether `key` is a plain relative object key: `/`-separated segments of
/// ASCII letters, digits, `.`, `_` and `-`, none empty and none starting
/// with a dot. So no `..`, no absolute path, no backslash and no percent
/// encoding can reach a storage backend through it.
pub fn is_plain_object_key(key: &str) -> bool {
    !key.is_empty()
        && key.split('/').all(|segment| {
            !segment.is_empty()
                && !segment.starts_with('.')
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_object_keys() {
        for key in [
            "images/base/0f8e/a1.jpg",
            "images/overrides/u/x-y_z.png",
            "avatars/abc.jpg",
        ] {
            assert!(is_plain_object_key(key), "{key}");
        }
        for key in [
            "",
            "/etc/passwd",
            "images/../../dev/zero",
            "images/./a.jpg",
            "images//a.jpg",
            "images/.hidden",
            "images/%2e%2e/a.jpg",
            "images\\..\\a.jpg",
            "images/a.jpg?x=1",
            "images/",
        ] {
            assert!(!is_plain_object_key(key), "{key:?}");
        }
    }
}
