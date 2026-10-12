//! cicada-storage（L3 能力）
//!
//! 文件存储：`Storage` trait 之上的驱动抽象，业务代码只依赖 trait，
//! 换驱动（本地 / S3 / OSS / RustFS）不改业务。
//!
//! 本层只做「字节进、字节出」：路径生成、MIME 判定、`sys_file` 落库都在接入层。
//! 驱动选择由 `build(&config.storage)` 在组装根完成（`storage.driver`）：
//! 本地文件系统（开发默认）与内存用原生实现，RustFS / OSS / S3 走 opendal
//! （`object.rs`）。三条路径共用同一份穿越校验与错误语义。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub mod grant;
pub mod object;

pub use grant::{
    DownloadGrant, DownloadGrantStore, GRANT_KEY_PREFIX, InMemoryGrantStore, RedisGrantStore,
};

pub use object::{OpendalStorage, build};

use async_trait::async_trait;
use sha2::{Digest, Sha256};

/// 存储错误。
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("文件不存在：{0}")]
    NotFound(String),
    #[error("非法路径：{0}")]
    InvalidPath(String),
    #[error("存储 IO 失败：{0}")]
    Io(String),
    /// 配置不足以建出驱动（缺桶、缺凭证之类）。
    ///
    /// 与 `Io` 分开：`Io` 是「后端活着但这次失败了」，运维要处置的是配置本身，
    /// 混在一起会把部署期的拼写错误伪装成存储故障。
    #[error("存储配置不完整：{0}")]
    Config(String),
    /// 该驱动没有这项能力（典型情形：本地驱动不会签预签名 URL）。
    ///
    /// 单独一档是因为调用方需要的是「退回另一条路径」，不是「报错给用户」。
    #[error("{0} 驱动不支持该能力")]
    Unsupported(&'static str),
}

/// 文件存储的窄接口。路径由调用方生成，实现只负责读写。
#[async_trait]
pub trait Storage: Send + Sync {
    /// 驱动名，写入 `sys_file.storage`。
    fn driver(&self) -> &'static str;
    async fn put(&self, path: &str, bytes: &[u8]) -> Result<(), StorageError>;
    async fn get(&self, path: &str) -> Result<Vec<u8>, StorageError>;
    async fn delete(&self, path: &str) -> Result<(), StorageError>;
    async fn exists(&self, path: &str) -> Result<bool, StorageError>;

    /// 预签名下载 URL（对象存储走这一条）。
    ///
    /// 默认不支持——本地与内存驱动没有「把授权外包给存储」这回事，
    /// 它们的一次性凭证走 `DownloadGrantStore`。
    async fn presign_get(&self, path: &str, ttl: Duration) -> Result<String, StorageError> {
        let _ = (path, ttl);
        Err(StorageError::Unsupported(self.driver()))
    }
}

/// 内容 sha256（十六进制小写）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 按 magic number 判定 MIME（不信扩展名）。
pub fn detect_mime(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'P', b'K', 0x03, 0x04, ..] => "application/zip",
        [b'%', b'P', b'D', b'F', ..] => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// 生成不可猜测的存储路径：`{tenant}/{uuid}`。
pub fn object_path(tenant_id: i64, original_name: &str) -> String {
    let ext = Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();
    format!("{tenant_id}/{}{ext}", uuid::Uuid::new_v4())
}

pub(crate) fn safe_relative(path: &str) -> Result<PathBuf, StorageError> {
    let p = Path::new(path);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(StorageError::InvalidPath(path.to_string()));
    }
    Ok(p.to_path_buf())
}

/// 本地文件系统驱动（开发默认， `storage.driver = local`）。
#[derive(Clone)]
pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, StorageError> {
        Ok(self.root.join(safe_relative(path)?))
    }
}

#[async_trait]
impl Storage for LocalStorage {
    fn driver(&self) -> &'static str {
        "local"
    }

    async fn put(&self, path: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let target = self.resolve(path)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StorageError::Io(e.to_string()))?;
        }
        tokio::fs::write(&target, bytes)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>, StorageError> {
        let target = self.resolve(path)?;
        match tokio::fs::read(&target).await {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StorageError::NotFound(path.to_string()))
            }
            Err(e) => Err(StorageError::Io(e.to_string())),
        }
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let target = self.resolve(path)?;
        match tokio::fs::remove_file(&target).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::Io(e.to_string())),
        }
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        Ok(tokio::fs::try_exists(self.resolve(path)?)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?)
    }
}

/// 内存驱动：单测 / 无文件系统环境用。
#[derive(Default)]
pub struct MemoryStorage {
    inner: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Storage for MemoryStorage {
    fn driver(&self) -> &'static str {
        "memory"
    }

    async fn put(&self, path: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let path = safe_relative(path)?;
        self.inner
            .lock()
            .unwrap()
            .insert(path.to_string_lossy().into_owned(), bytes.to_vec());
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>, StorageError> {
        let path = safe_relative(path)?;
        self.inner
            .lock()
            .unwrap()
            .get(&path.to_string_lossy().into_owned())
            .cloned()
            .ok_or_else(|| StorageError::NotFound(path.to_string_lossy().into_owned()))
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let path = safe_relative(path)?;
        self.inner
            .lock()
            .unwrap()
            .remove(&path.to_string_lossy().into_owned());
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let path = safe_relative(path)?;
        Ok(self
            .inner
            .lock()
            .unwrap()
            .contains_key(&path.to_string_lossy().into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_from_magic_number() {
        assert_eq!(detect_mime(&[0x89, b'P', b'N', b'G', 0x0d]), "image/png");
        assert_eq!(detect_mime(b"%PDF-1.7"), "application/pdf");
        assert_eq!(detect_mime(b"hello"), "application/octet-stream");
    }

    #[test]
    fn object_path_is_unguessable_and_keeps_ext() {
        let p = object_path(1, "report.PDF");
        assert!(p.starts_with("1/"), "{p}");
        assert!(p.ends_with(".pdf"), "{p}");
    }

    #[test]
    fn rejects_traversal() {
        assert!(safe_relative("../etc/passwd").is_err());
        assert!(safe_relative("/etc/passwd").is_err());
        assert!(safe_relative("1/abc.png").is_ok());
    }

    #[tokio::test]
    async fn memory_roundtrip() {
        let s = MemoryStorage::new();
        s.put("1/a.bin", b"data").await.unwrap();
        assert!(s.exists("1/a.bin").await.unwrap());
        assert_eq!(s.get("1/a.bin").await.unwrap(), b"data");
        s.delete("1/a.bin").await.unwrap();
        assert!(!s.exists("1/a.bin").await.unwrap());
    }

    #[tokio::test]
    async fn local_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cicada-storage-{}", uuid::Uuid::new_v4()));
        let s = LocalStorage::new(&dir);
        s.put("1/x.txt", b"hi").await.unwrap();
        assert_eq!(s.get("1/x.txt").await.unwrap(), b"hi");
        s.delete("1/x.txt").await.unwrap();
        assert!(s.get("1/x.txt").await.is_err());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
