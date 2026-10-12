//! cicada-ports（跨模块端口）
//!
//! 跨模块 trait 端口与本地/远程 adapter 的公共定义（`DeptDirectory`、`EventHandler`、`MenuRegistry`）。
//! 按依赖规则规则：只依赖 `cicada-kernel`；具体 adapter 按需在实现期挂载。

pub mod archive;
pub mod cache;
pub mod dept;
pub mod event;
pub mod job;
pub mod kv;
pub mod lock;
pub mod menu;

pub use archive::{ArchiveError, ArchiveStore, InMemoryArchive};
pub use cache::{CacheError, CacheInfo, CacheInspector, InMemoryCacheInspector, KeyMeta, ScanPage};
pub use dept::{DeptDirectory, DeptDirectoryError, InMemoryDeptDirectory};
pub use event::{
    AGGREGATE_AUDIT, ChangeLogEvent, DomainEvent, EVENT_CHANGE_LOG, EVENT_LOGIN_LOG,
    EVENT_OPER_LOG, EventError, EventHandler, EventRouter, InMemoryEventHandler, LoginLogEvent,
    OperLogEvent,
};
pub use job::{
    FnHandler, JobContext, JobError, JobHandler, JobRegistry, JobRuntime, NoopRunSink,
    NoopTriggerSource, RunSink, SkipReason, TriggerOutcome, TriggerSource,
};
pub use kv::{
    EMPTY_MARKER, EMPTY_TTL_SECS, InMemoryKeyValueStore, KeyValueStore, KvCache, jittered_ttl,
};
pub use lock::{DistributedLock, InMemoryDistributedLock, LockError};
pub use menu::{InMemoryMenuRegistry, MenuKind, MenuRegistry, MenuRegistryError, MenuSpec};
