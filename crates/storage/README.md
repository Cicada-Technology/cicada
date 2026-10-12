# cicada-storage

文件存储：`Storage` trait 之上的驱动抽象。

## 职责

- `Storage` trait：put / get / delete / stat，字节进字节出。
- `LocalStorage`：本地文件系统（开发默认），路径按租户分目录。
- `MemoryStorage`：测试用。
- 内容寻址：sha256 参与对象名，重复上传不产生第二份。

## 公开 API

`Storage` / `StorageError` / `ObjectStat`、`LocalStorage::new(root)`、`MemoryStorage::new()`。

## 依赖

`async-trait` / `tokio` / `sha2` / `uuid` / `thiserror`。

## 不做什么

- 不生成业务路径、不判 MIME、不落 `sys_file`：这些在接入层（`cicada-web::file`）。
- 不含对象存储驱动：S3 / OSS / RustFS 按 feature 裁剪点在后续里程碑接入，
  换驱动不改业务代码。
