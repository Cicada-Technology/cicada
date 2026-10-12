# cicada-rbac

L3 能力：授权——权限点解析、角色合并、数据范围计算与授权缓存。

## 职责

- `permission`：`user → roles → menus(perms) → 权限集合`，去重成 `PermissionSet`（BTreeSet，
  因此可稳定序列化进缓存与日志）。
- `data_scope`：把多角色的 6 种范围声明（`All` / `Custom` / `Dept` / `DeptAndChild` / `SelfOnly` /
  内部兜底 `None`）合并成一个 `DataScope`。
  `DeptAndChild` 的**后代部门在这里展开**进 `dept_ids`，消费侧只做 `IN` 判断，
  不需要跨模块 JOIN 部门树（datascope-typestate.md ）。
  命中 `role_key = admin`（`SUPER_ADMIN_ROLE_KEY`）即短路为 `All`。
- `authorize`：一次授权判定，产出随请求挂在 `RequestContext` 上的 `Authorization`
  （数据范围 + 权限集合 + 超管标记 + 角色键）。
- `cache`：`AuthzCache` 键为 `cicada:authz:{tenant}:{user}:v{version}`，版本号在
  `cicada:permver:{tenant}`；角色 / 角色-菜单 / 用户-角色 / 菜单权限变更时
  `invalidate_tenant` 把版本号整体 +1，旧键靠 TTL 自然回收。
  **缓存是纯优化**：读写失败一律降级为直接查库，绝不让请求失败。

## 公开 API

`authorize` / `Authorization`、`PermissionSet` / `resolve_permissions`、
`compute_data_scope` / `is_super_admin` / `SUPER_ADMIN_ROLE_KEY`、
`AuthzCache`（`get` / `put` / `load_or_compute` / `invalidate_tenant`）/
`AuthzStore` / `RedisAuthzStore` / `InMemoryAuthzStore`，
并透传 `cicada_kernel::RoleGrant`（纯值契约，省一次导入）。

## 依赖

`cicada-kernel`（`DataScope` / `Permission` / `RoleGrant` 等值对象）、
`cicada-infra`（`Cache` → Redis adapter）、`cicada-ports`（`DeptDirectory` 端口）、
`serde` / `serde_json` / `async-trait` / `tracing`。

## 不做什么

- **不含查询构造**：`DataScope` 如何变成 SQL 条件、以及「未过滤就编译不过」的 typestate 约束
  都在 `cicada-kernel::scope`；本 crate 只算出范围值。
- 不做 RLS：`cicada-infra::rls` 是兜底的第二道闸（ADR 0001），开关与表名不由这里决定。
- 不出 HTTP 面：角色/菜单的管理路由在 `cicada-web::system`（`role.rs` / `menu.rs`），
  权限点的声明与对账在 `Module::manifest` + `Assembler`。
- 不读库：角色与权限集合由调用方装载后传入，部门树只经 `DeptDirectory` 端口获取。
