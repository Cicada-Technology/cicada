# cicada-macros

过程宏：把「声明」变成编译期事实，避免运行时的字符串约定。

## 职责

- `#[require_perm("system:user:list")]`：为 handler 生成 `<name>__perms()`，
  路由注册时读进 `RoutePerms`，启动期由 `cicada-web::Assembler` 与权限点目录对账。
- `#[audit(module, action, kind)]`：生成 `<name>__audit()`，`audit_middleware` 据此写操作日志。
- `#[derive(DataScoped)]` + `#[data_scope(tenant=…, owner=…, deleted=…)]`：
  实体归属维度的 typestate 声明，`ScopedQuery` 据此在编译期挡住漏加过滤的查询。
- `#[derive(Maskable)]` + `#[mask(phone | id_card | email | name | bank_card | password)]`：
  序列化时脱敏，规则本身在 `cicada-kernel::mask`。

## 公开 API

四个宏本身。生成的方法名固定为 `<handler>__perms` / `<handler>__audit`，由
`cicada-web::routes` 通过 `RoutePerms` / `RouteAudit` trait 消费。

## 依赖

`proc-macro2` / `quote` / `syn`；`cicada-kernel`（宏展开代码里引用的类型路径）。

## 不做什么

- 不做运行时逻辑：宏只生成代码，规则与状态都在被生成的那一侧。
- 不校验权限标识是否存在于目录：那是启动期 `Assembler` 与 `perm_coverage` 测试的职责。
