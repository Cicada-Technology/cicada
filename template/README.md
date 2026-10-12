# {{project-name}}

由 `cicada new` 生成的使用者工程。它与 cicada 框架是**依赖关系**，
不是 fork：框架只升版本号，业务代码永远留在本仓库。

## 目录

```
{{project-name}}/
├── src/
│   ├── main.rs            # 组合根：读配置 → 声明模块与任务 → 起服务
│   └── bin/migrate.rs     # 迁移入口（框架迁移 → 业务迁移 → 种子 → admin）
├── business/              # 业务模块（代码生成器的落点），永不与框架代码混放
│   └── goods/             # 样例模块（生成器产物）：entity / dto / handler / module
├── migrations/            # 业务迁移，与框架迁移分目录
├── templates/             # 代码生成模板覆写点（可选）
└── config/default.yaml    # 分层配置
```

## 起步

```bash
# 1. 依赖：PostgreSQL + Redis（用 cicada 仓库的 docker compose up -d 起开发环境）
# 2. 建表 + 种子 + 默认租户 admin
cargo run --bin migrate -- up --seed --admin
# 3. 起服务
cargo run
```

起来之后：`/healthz`、`/readyz`、`/metrics` 是健康与指标，`/docs` 是 OpenAPI（Swagger UI），
`/api/v1/auth/login` 用上一步的 admin 账号换 token。

## 加一个业务模块

1. 建表（可以直接建，也可以让生成器从既有表反推）；
2. `cicada-cli gen --table <表名>` 产出 entity / dto / handler / module / 迁移 / 前端页；
3. 把 `business/<模块>/` 放进本工程，迁移放进 `migrations/` 并在 `migrations/mod.rs` 登记；
4. 在 `src/main.rs` 的 `AppBuilder` 上加一行 `.with_module(Box::new(...))`。

`business/goods/` 是生成器产物的样例（表 `biz_goods`），看懂后可以直接删掉换成自己的表：
`owns`（数据属主）、`perms`（权限点）、`routes`（带权限元数据的路由）三者会在启动期
双向对账，漏一个都起不来。

## 升级框架

改 `Cargo.toml` 里的 tag 即可：

```toml
cicada = { git = "https://github.com/<org>/cicada", tag = "v0.2.0", features = ["full"] }
```

数据库迁移 append-only；废弃接口保留一个版本的弃用期。逐版本「改了什么、你要做什么」
看框架仓库的 `CHANGELOG.md`，升级动作与迁移纪律看 `docs/UPGRADE.md`。
要临时改框架行为，先看 `templates/`、`config/` 与 `ports`（trait adapter）这三处覆写点，
实在需要才用 `[patch]`。
