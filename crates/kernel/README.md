# cicada-kernel

L0 内核：错误类型、值对象、数据范围、权限标识与请求上下文。

## 职责

- `AppError`：全workspace唯一的错误类型，HTTP 面据此映射状态码与错误码。
- 新类型 ID（`TenantId` / `UserId` / `RoleId` / `DeptId` / `MenuId` / `PostId` / `JobId`），
  避免裸 `i64` 在函数签名之间串位。
- `Scope`：数据范围的事实（租户 + 可见部门集合 + 是否超管），`Serialize + Deserialize`
  因此能作为异步导出任务的快照存库。
- `Permission`：权限标识的解析与比较（`system:user:list` 三段式）。
- `RequestContext` / `RequestId` / `TraceId`：一次请求的身份与追踪信息。
- `mask`：脱敏值对象与规则（配合 `cicada-macros` 的 `Maskable` 派生）。
- `page`：分页入参与 `PageResult` 的统一形状。

## 公开 API

`AppError`、`Scope`、`Permission`、`RequestContext` / `RequestId` / `TraceId`、
各类 ID、`PageQuery` / `PageResult`、`ConcurrencyPolicy` / `TriggerKind`、`mask::*`、
`principal::*` / `role::*`。

## 依赖

`serde` / `sea-orm`（只为 `ActiveValue` 等类型出现在签名里）/ `thiserror` / `chrono` / `uuid`。
**不依赖任何其它 cicada crate**（依赖规则的分层规则）。

## 不做什么

- 不做 IO：没有数据库、Redis、HTTP 的任何痕迹。
- 不做业务判断：这里只有值与契约，规则属各能力 crate。
- 不定义实体：表结构归各领域 crate（`cicada-system::entity` 等）。
