//! 归档对象存储端口（到期归档到对象存储而不是直接删除）。
//!
//! `system` 拥有日志表，但它（L2）不能依赖 `cicada-storage`（L3）——所以这里只声明
//! 一个「把字节流放到某个键下」的缝，真实实现由组装根把 `cicada_storage::Storage`
//! 适配进来，测试用 [`InMemoryArchive`]（依赖规则规则 2/7）。

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("归档写入失败：{0}")]
    Storage(String),
}

/// 归档写入缝。实现必须保证同键重复写入是覆盖而不是报错——重放是常态。
#[async_trait]
pub trait ArchiveStore: Send + Sync + 'static {
    async fn put(&self, key: &str, body: &[u8]) -> Result<(), ArchiveError>;
}

/// 测试夹具：留在内存里，断言用。
#[derive(Default)]
pub struct InMemoryArchive {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl InMemoryArchive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keys(&self) -> Vec<String> {
        self.objects.lock().unwrap().keys().cloned().collect()
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.objects.lock().unwrap().get(key).cloned()
    }
}

#[async_trait]
impl ArchiveStore for InMemoryArchive {
    async fn put(&self, key: &str, body: &[u8]) -> Result<(), ArchiveError> {
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_string(), body.to_vec());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fixture_keeps_latest_bytes_per_key() {
        let archive = InMemoryArchive::new();
        archive.put("logs/a", b"first").await.unwrap();
        archive.put("logs/a", b"second").await.unwrap();
        assert_eq!(archive.keys(), vec!["logs/a".to_string()]);
        assert_eq!(archive.get("logs/a").unwrap(), b"second".to_vec());
    }
}
