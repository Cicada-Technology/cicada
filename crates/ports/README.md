# cicada-ports

跨模块端口：谁需要谁声明的 trait，加上可测试的内存 adapter。

## 职责

- `lock`：`DistributedLock`（集群下定时任务触发侧选主）。
- `job`：`TriggerSource` 与 `RunSink`——把「什么时候跑」和「在哪跑」拆成两条正交的轴。
- `dept`：`DeptDirectory`，让 `cicada-rbac` 计算数据范围时不必依赖 `cicada-system`。
- `cache`：`CacheInspector`，缓存监控页需要的 key 扫描与元信息读取。
- `menu`：`MenuRegistry`，模块声明的菜单与按钮交给 `sys_menu` 的属主落库（模块边界），
  属主缺席即启动失败；`parent` 留空的按钮按 `xxx:view` 约定推断所属菜单。
- `event`：`DomainEvent` 与 `EventHandler` / `EventRouter`——事务性发件箱的投递缝（审计设计 / 模块契约），
  事件负载契约（`OperLogEvent` / `LoginLogEvent`）也定在这里，生产者与消费者共用同一份字段定义。
- `archive`：`ArchiveStore`，到期日志分区的归档出口（审计设计「归档到对象存储而不是直接删除」）。
  只有 `put(key, bytes)` 一个动作：`system`（L2）负责决定导什么、什么时候删，
  对象存储的实现（`Storage`，L3）由组装根桥接进来，两个方向都不互相依赖。

## 公开 API

`DistributedLock` / `InMemoryDistributedLock` / `LockError`、`TriggerSource` / `RunSink` /
`JobTick`、`DeptDirectory` / `InMemoryDeptDirectory`、`CacheInspector` / `CacheInfo` / `KeyMeta`、
`DomainEvent` / `EventRouter` / `EventHandler` / `InMemoryEventHandler` / `OperLogEvent` / `LoginLogEvent`、
`MenuRegistry` / `MenuSpec` / `MenuKind` / `InMemoryMenuRegistry`、
`ArchiveStore` / `InMemoryArchive` / `ArchiveError`。

## 依赖

`cicada-kernel` / `async-trait` / `thiserror` / `serde` / `serde_json`。

## 不做什么

- 不放实现策略：Redis 锁在 `cicada-worker`，真实目录在 `cicada-system`，
  审计落库在 `cicada-system::log_store`，这里只有 trait 与「测试用」的内存版。
- 不定义领域实体或表结构。
- 除 `cicada-kernel` 外不依赖任何 cicada crate（依赖规则）。
