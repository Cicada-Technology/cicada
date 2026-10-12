# cicada-testkit

测试侧的共享夹具与测试分层测试分层的落地点。不是运行时依赖：任何业务 crate 都不应把它
放进 `[dependencies]`。

## 职责

- `fixtures`：带完整归属维度的样例实体 `sample_doc`（`DeptAndOwner` 形态，
  `DeriveEntityModel` + `DataScoped`），供 typestate 的编译失败测试与组件测试复用。
- 承载两类**只有在这里才有意义**的测试：
  - `tests/permission_matrix.rs` — 测试分层「权限矩阵全分支」：5 种数据范围 × 3 类实体
    （`DeptOnly` / `DeptAndOwner` / `OwnerOnly`）表驱动，断言真实 PostgreSQL 返回的 id 集合，
    不是 SQL 字符串；期望值取自 `docs/superpowers/design-it-twice/datascope-typestate.md`。
  - `tests/data_scope.rs` — 端到端组件测试：`system` 实体 + `LocalDeptDirectory` +
    `rbac::compute_data_scope` + `kernel::Query` typestate 作用域过滤一次跑通。
  - `tests/ui.rs` + `tests/ui/` — `trybuild` 把 typestate 保证钉成**编译失败**
    （未作用域过滤就消费查询、实体未声明 `#[data_scope]`）。

## 公开 API

`fixtures::{Model, Entity, Column, ActiveModel, ...}`（仅测试用）。

## 依赖

`cicada-kernel`（`testkit` feature）/ `cicada-infra` / `cicada-ports` / `cicada-macros` /
`sea-orm`；dev 侧另有 `trybuild`、`cicada-system`、`cicada-rbac`、`cicada-migrations`。

## 不做什么

- **不起 testcontainers**：组件测试连真实 PostgreSQL（`DATABASE_URL`，缺省指向本机
  `cicada` 库）。SQL 语义必须被真实验证，因此没有 mock ORM。
- 不做 HTTP 面测试 helper：接口测试（`tower::ServiceExt::oneshot`、登录辅助）在
  `crates/web/tests/`，那里能拿到 `AppState`。
- 不含断言宏：跨 crate 复用的宏归 `cicada-macros`。
