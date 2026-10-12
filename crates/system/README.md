# cicada-system

L2 领域：用户 / 角色 / 菜单 / 部门 / 岗位 / 字典 / 参数 / 公告 / 日志，以及平台侧的租户、
套餐、API Key 与配额。

## 职责

- 上述表的 SeaORM 实体（`entity`）与读写：这些表**只在这里**被直接访问。
- `menu::load_menu_tree`：按角色可见性组装菜单树，前端动态路由的唯一数据源。
- `grant`：用户-角色、角色-菜单授权的写入与查询。
- `quota::TenantQuota`：租户套餐限额闸门（用户数 / 存储 / QPS，ADR 0004）。
- `api_key`：开放接口凭证的签发、校验、吊销（ADR 0003）。
- `dept_directory::LocalDeptDirectory`：实现 `cicada-ports::DeptDirectory`，
  让 `cicada-rbac` 不必反向依赖本 crate。
- `menu_registry::DbMenuRegistry`：实现 `cicada-ports::MenuRegistry`，是模块声明式菜单的
  落库出口（模块边界）。按「同权限标识 / 同名同类型」幂等，目录→菜单→按钮顺序插入以便定位父级，
  父级推断不出来就报错而不是悄悄挂到根节点。
- `log_store::SysLogStore`：实现 `cicada-ports::EventHandler`，是 `sys_oper_log` /
  `sys_logininfor` / `sys_audit_log` 的**唯一写入者**（审计设计：审计事件由 relay 投递到这里落地，
  UA→浏览器/OS 的解析也归本表属主）。写入走 `Db::begin_scoped`，租户取自事件本身——
  日志表一旦开 RLS（数据权限白名单里有它们），不带 `app.tenant_id` 的 `INSERT` 会被
  `WITH CHECK` 拒掉，审计就静默停摆。
- `log_retention::LogRetention`：日志分区的生命周期（审计设计保留期 / 部署自动建分区任务）。
  扩窗 → 找「整月都过了保留期」的分区 → 导出 NDJSON 交给 `ArchiveStore` → 才 `DETACH` + `DROP`；
  归档失败就保留分区，`archive=false` 时只报告不动数据。表名与分区名先过白名单和
  `assert_ident` 再拼 DDL——DDL 没有绑定参数可用。
- `entity/audit_log`：字段级变更留痕的实体（按月分区，主键 `(audit_id, oper_time)`）。

## 公开 API

`entity::*`、`load_menu_tree` / `MenuTreeNode`、`grant::*`、
`TenantQuota` / `TenantUsage` / `QuotaError`、`api_key::*`、`LocalDeptDirectory`、
`SysLogStore` / `DbMenuRegistry`、`LogRetention` / `RetentionReport` / `PARTITIONED_TABLES`。

## 依赖

`cicada-kernel` / `cicada-infra` / `cicada-ports` / `cicada-macros` / `sea-orm` / `argon2`。

## 不做什么

- 不定义 HTTP 路由与 DTO：那是 `cicada-web::system`。
- 不计算数据范围：`Scope` 由 `cicada-rbac` 给出，本 crate 只按它读写。
- 不做认证：口令散列与登录在 `cicada-auth`。
