//! opendal 驱动：RustFS（S3 兼容，自建默认）、阿里云 OSS、
//! S3 及其他兼容对象存储共用一套 `Storage` 接口，按 `storage.driver` 切换。
//!
//! 为什么本地驱动不也走 opendal：`local` 与 `memory` 只需要 `std::fs`，把它们塞进
//! opendal 会把整条 HTTP 传输栈变成开发环境的运行时依赖，而文件存储要的「一套 API」
//! 是 `Storage` trait 本身——业务代码（`crates/web/src/file.rs`）拿到的始终是
//! `Arc<dyn Storage>`，换驱动不改一行调用方。

use std::sync::Arc;

use async_trait::async_trait;
use opendal::Operator;

use crate::{Storage, StorageError, safe_relative};

use cicada_infra::config::StorageConfig;

/// opendal 之上的驱动适配：`driver` 名写进 `sys_file.storage`，用于事后判断
/// 某个对象当初落在哪个后端（换驱动时存量对象仍在原桶，字段不能撒谎）。
pub struct OpendalStorage {
    driver: &'static str,
    op: Operator,
}

impl OpendalStorage {
    pub fn new(driver: &'static str, op: Operator) -> Self {
        Self { driver, op }
    }

    /// opendal 的内存服务：不需要真实对象存储也能跑通这条实现路径（测试用）。
    pub fn in_memory() -> Result<Self, StorageError> {
        let op = Operator::new(opendal::services::Memory::default())
            .map_err(|e| StorageError::Config(e.to_string()))?;
        Ok(Self::new("memory", op))
    }

    /// 把内部路径转成对象键：沿用本地驱动那套穿越校验（`../` 与绝对路径一律拒绝），
    /// 之后才是 opendal 自己的路径规范化。
    fn key(path: &str) -> Result<String, StorageError> {
        let rel = safe_relative(path)?;
        Ok(rel.to_string_lossy().into_owned())
    }
}

fn map_err(path: &str, e: opendal::Error) -> StorageError {
    if e.kind() == opendal::ErrorKind::NotFound {
        StorageError::NotFound(path.to_string())
    } else {
        StorageError::Io(e.to_string())
    }
}

#[async_trait]
impl Storage for OpendalStorage {
    fn driver(&self) -> &'static str {
        self.driver
    }

    async fn put(&self, path: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let key = Self::key(path)?;
        self.op
            .write(&key, bytes.to_vec())
            .await
            .map_err(|e| map_err(&key, e))?;
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>, StorageError> {
        let key = Self::key(path)?;
        let buffer = self.op.read(&key).await.map_err(|e| map_err(&key, e))?;
        Ok(buffer.to_vec())
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let key = Self::key(path)?;
        self.op.delete(&key).await.map_err(|e| map_err(&key, e))
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let key = Self::key(path)?;
        self.op.exists(&key).await.map_err(|e| map_err(&key, e))
    }

    /// 桶端签名（文件存储）：授权发生在签发这一刻，之后的字节传输不占应用带宽，
    /// 也不需要把物理路径暴露给客户端——URL 里只有桶与签名参数。
    async fn presign_get(
        &self,
        path: &str,
        ttl: std::time::Duration,
    ) -> Result<String, StorageError> {
        let key = Self::key(path)?;
        let signed = self
            .op
            .presign_read(&key, ttl)
            .await
            .map_err(|e| map_err(&key, e))?;
        Ok(signed.uri().to_string())
    }
}

/// 按 `storage.driver` 建驱动（驱动按配置切换）。
///
/// 缺参在 `Config::validate` 就已经 fail fast，这里仍逐项检查：工厂也可能被
/// 组装根之外的代码（CLI、测试）直接调用，不该依赖调用方先跑过校验。
pub fn build(cfg: &StorageConfig) -> Result<Arc<dyn Storage>, StorageError> {
    let storage: Arc<dyn Storage> = match cfg.driver.as_str() {
        "local" => Arc::new(crate::LocalStorage::new(&cfg.root)),
        "memory" => Arc::new(crate::MemoryStorage::new()),
        "rustfs" => Arc::new(s3_compatible(cfg, "rustfs")?),
        "s3" => Arc::new(s3_compatible(cfg, "s3")?),
        "oss" => Arc::new(aliyun_oss(cfg)?),
        other => {
            return Err(StorageError::Config(format!(
                "未知存储驱动 {other:?}（可用：local / rustfs / oss / s3 / memory）"
            )));
        }
    };
    Ok(storage)
}

/// `rustfs` 与 `s3` 共用 opendal 的 S3 服务——这正是选 opendal 的理由：
/// S3 兼容协议不用为每个厂商写一个驱动。
fn s3_compatible(
    cfg: &StorageConfig,
    driver: &'static str,
) -> Result<OpendalStorage, StorageError> {
    let require = |v: &str, field: &str| -> Result<(), StorageError> {
        if v.trim().is_empty() {
            return Err(StorageError::Config(format!(
                "storage.driver = {driver}: {field} 不能为空"
            )));
        }
        Ok(())
    };
    require(&cfg.bucket, "storage.bucket")?;
    require(&cfg.access_key_id, "storage.access_key_id")?;
    require(
        cfg.access_key_secret.expose_secret(),
        "storage.access_key_secret",
    )?;
    // 自建服务（rustfs）没有可推的默认端点；AWS 才有 region → endpoint 解析。
    if cfg.endpoint.trim().is_empty() && cfg.driver == "rustfs" {
        return Err(StorageError::Config(
            "storage.driver = rustfs: storage.endpoint 不能为空（自建服务没有默认地址）"
                .to_string(),
        ));
    }
    if cfg.endpoint.trim().is_empty() && cfg.region.trim().is_empty() {
        return Err(StorageError::Config(
            "storage.driver = s3: storage.endpoint 与 storage.region 至少填一个".to_string(),
        ));
    }
    let mut builder = opendal_service_s3::S3::default()
        .bucket(&cfg.bucket)
        .access_key_id(&cfg.access_key_id)
        .secret_access_key(cfg.access_key_secret.expose_secret());
    if !cfg.endpoint.trim().is_empty() {
        builder = builder.endpoint(&cfg.endpoint);
    }
    // 签名 v4 需要一个 region；自建网关不校验它，填 `auto` 是 opendal 的惯例。
    builder = builder.region(if cfg.region.trim().is_empty() {
        "auto"
    } else {
        cfg.region.trim()
    });
    // 只有真 AWS 走虚拟主机风格（`bucket.s3.amazonaws.com`，官方推荐）；
    // 指向自定义端点的兼容服务一律 path 风格，这是 rustfs/minio/R2 能连通的前提。
    if cfg.endpoint.trim().is_empty() {
        builder = builder.enable_virtual_host_style();
    }
    let op = Operator::new(builder).map_err(|e| StorageError::Config(e.to_string()))?;
    Ok(OpendalStorage::new(driver, op))
}

fn aliyun_oss(cfg: &StorageConfig) -> Result<OpendalStorage, StorageError> {
    for (value, field) in [
        (&cfg.bucket, "storage.bucket"),
        (&cfg.endpoint, "storage.endpoint"),
        (&cfg.access_key_id, "storage.access_key_id"),
        (
            cfg.access_key_secret.expose_secret(),
            "storage.access_key_secret",
        ),
    ] {
        if value.trim().is_empty() {
            return Err(StorageError::Config(format!(
                "storage.driver = oss: {field} 不能为空"
            )));
        }
    }
    let builder = opendal_service_oss::Oss::default()
        .bucket(&cfg.bucket)
        .endpoint(&cfg.endpoint)
        .access_key_id(&cfg.access_key_id)
        .access_key_secret(cfg.access_key_secret.expose_secret());
    let op = Operator::new(builder).map_err(|e| StorageError::Config(e.to_string()))?;
    Ok(OpendalStorage::new("oss", op))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object_cfg(driver: &str) -> StorageConfig {
        StorageConfig {
            driver: driver.to_string(),
            bucket: "cicada".to_string(),
            endpoint: "https://rustfs.internal:9000".to_string(),
            region: "auto".to_string(),
            access_key_id: "key".to_string(),
            access_key_secret: cicada_infra::config::Secret::new("secret".to_string()),
            ..Default::default()
        }
    }

    /// 文件存储的验收面：一行 `storage.driver` 换后端，业务拿到的还是 `dyn Storage`。
    /// 建驱动不联网（opendal 只在第一次请求时拨号），所以这里能覆盖全部四档。
    #[test]
    fn every_configured_driver_builds_behind_the_same_trait() {
        let cases = [
            ("local", "local"),
            ("memory", "memory"),
            ("rustfs", "rustfs"),
            ("s3", "s3"),
            ("oss", "oss"),
        ];
        for (configured, expected) in cases {
            let cfg = object_cfg(configured);
            let storage = build(&cfg).unwrap_or_else(|e| panic!("{configured}: {e}"));
            assert_eq!(storage.driver(), expected, "{configured} 驱动名写错了");
        }
        // AWS 走 region 解析、不给端点，也要能建出来。
        let aws = StorageConfig {
            driver: "s3".to_string(),
            bucket: "cicada".to_string(),
            region: "ap-northeast-1".to_string(),
            access_key_id: "key".to_string(),
            access_key_secret: cicada_infra::config::Secret::new("secret".to_string()),
            ..Default::default()
        };
        assert_eq!(build(&aws).unwrap().driver(), "s3");
    }

    /// 缺参必须在建驱动时就报错——留到第一次上传只会被当成「存储挂了」。
    #[test]
    fn incomplete_object_storage_config_is_refused() {
        let no_bucket = StorageConfig {
            bucket: String::new(),
            ..object_cfg("s3")
        };
        assert!(matches!(
            build(&no_bucket),
            Err(StorageError::Config(msg)) if msg.contains("storage.bucket")
        ));

        let rustfs_without_endpoint = StorageConfig {
            endpoint: String::new(),
            ..object_cfg("rustfs")
        };
        assert!(matches!(
            build(&rustfs_without_endpoint),
            Err(StorageError::Config(msg)) if msg.contains("storage.endpoint")
        ));

        let no_secret = StorageConfig {
            access_key_secret: cicada_infra::config::Secret::new(String::new()),
            ..object_cfg("oss")
        };
        assert!(matches!(
            build(&no_secret),
            Err(StorageError::Config(msg)) if msg.contains("storage.access_key_secret")
        ));

        // 未知驱动：配置校验会先拦，工厂自己也不能放行。
        let unknown = StorageConfig {
            driver: "minio".to_string(),
            ..Default::default()
        };
        assert!(matches!(
            build(&unknown),
            Err(StorageError::Config(msg)) if msg.contains("minio")
        ));
    }

    /// opendal 这条实现路径用内存服务跑通：与真实桶共用同一份键校验与错误映射。
    #[tokio::test]
    async fn opendal_path_shares_the_path_guard_and_error_semantics() {
        let s = OpendalStorage::in_memory().expect("内存服务应当可直接建");
        s.put("7/blob.png", b"png-bytes").await.unwrap();
        assert!(s.exists("7/blob.png").await.unwrap());
        assert_eq!(s.get("7/blob.png").await.unwrap(), b"png-bytes");

        // 与本地驱动同一条闸门：穿越路径根本不该送到后端。
        for hostile in ["../etc/passwd", "/etc/passwd"] {
            assert!(
                matches!(s.get(hostile).await, Err(StorageError::InvalidPath(_))),
                "{hostile} 应当被路径校验拦下"
            );
        }

        s.delete("7/blob.png").await.unwrap();
        assert!(!s.exists("7/blob.png").await.unwrap());
        let err = s.get("7/blob.png").await.unwrap_err();
        assert!(
            matches!(err, StorageError::NotFound(_)),
            "缺对象要映射成 NotFound，而不是笼统的 IO 失败：{err}"
        );
    }
}
