<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="web/public/brand/logo-dark.svg" />
    <img src="web/public/brand/logo.svg" alt="cicada" width="420" />
  </picture>
</p>

# cicada

生产级 Rust 后台基座模板。

认证 · RBAC · 数据权限 · 多租户 · 审计 · 代码生成 · 可观测性

一次装好，业务只写自己的模块

[简体中文](README.md) · [English](README.en.md)

[![Rust](https://img.shields.io/badge/Rust-1.98-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Axum](https://img.shields.io/badge/Axum-0.8-0f172a)](https://github.com/tokio-rs/axum)
[![SeaORM](https://img.shields.io/badge/SeaORM-2.0-0ea5e9)](https://www.sea-ql.org/SeaORM/)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-17-336791?logo=postgresql&logoColor=white)](https://www.postgresql.org/)
[![Redis](https://img.shields.io/badge/Redis-7-DC382D?logo=redis&logoColor=white)](https://redis.io/)
[![Vue](https://img.shields.io/badge/Vue-3-42B883?logo=vue.js&logoColor=white)](https://vuejs.org/)
[![License](https://img.shields.io/badge/License-MIT-blue)](#许可)

![cicada 管理后台](docs/assets/dashboard.png)

## 这是什么

cicada 是一个用 Rust 写的**生产级后台基座**。它把后台系统里反复出现、又容易做错的部分——认证与会话、RBAC、数据权限、多租户、审计、文件存储、通知、定时任务、代码生成、可观测性——一次性做完并默认集成。业务方只需要写自己的模块。

它不是 demo，也不是一个需要 fork 的仓库：框架 crate 走依赖，业务代码放在 `business/`，升级只改版本号。

**适合谁：**

- 想用 Rust 做后台管理、运营平台、SaaS 基座的团队；
- 需要 RBAC + 数据权限 + 多租户 + 审计，但不想从零搭基础设施的项目；
- 希望先做**模块化单体**，未来再按端口/适配器拆分服务的架构。

## 为什么是 cicada

- **标准 REST 契约**：成功返回裸 JSON + 真实 HTTP 状态码；失败统一 RFC 9457 `problem+json` + 机器可读业务码，OpenAPI 3.1 快照由 CI 守住。
- **默认生产化**：审计发件箱、日志分区与归档、限流、优雅停机、指标与 OTLP 链路、健康探针，默认就有，不需要上线前再补一轮。
- **编译期正确性**：强类型 ID、类型状态的数据范围查询、权限与审计过程宏；没有反射，没有运行期字符串约定。漏加权限、漏过滤租户、漏做软删除，尽量在编译期或测试期暴露。
- **模块化单体**：L0–L4 分层，依赖方向由 `deploy/check_layering.py` 在 CI 里强制；跨模块协作走 `ports` 与领域事件，不共享实现。
- **模板优先，不 fork**：`cicada new` 生成独立工程，框架是依赖；业务代码与框架代码物理分离，升级路径清晰。
- **全栈且双语**：Rust REST API + Vue 3 管理端，界面与消息支持简体中文 / English。

## 能力一览

| 领域 | 你能得到什么 |
| --- | --- |
| **认证与会话** | JWT + Redis 会话、刷新令牌轮换、图形验证码、登录失败锁定、TOTP、强制下线、在线会话快照与历史查询 |
| **RBAC** | 权限点、菜单树、角色合并、按版本号失效的授权缓存、装配期权限点双向对账 |
| **数据权限** | 5 种数据范围（全部 / 自定义 / 本部门 / 本部门及以下 / 仅本人），类型状态查询 + PostgreSQL RLS 兜底开关 |
| **多租户** | `tenant_id` 贯穿实体与缓存键、租户套餐、席位/存储/QPS 限额、默认租户与租户初始化 |
| **系统域** | 用户、角色、菜单、部门、岗位、字典、参数、公告、在线用户、操作日志、登录日志 |
| **审计** | 操作/登录日志、字段级变更留痕；`outbox_event` + relay 异步落库；日志按月分区，到期整月归档后再删 |
| **文件存储** | 本地 / RustFS / 阿里云 OSS / S3 一套 `Storage` trait；magic-number 校验；一次性下载令牌或预签名 URL |
| **通知中心** | 站内信、邮件（SMTP）、Webhook 三通道；模板、渠道、投递记录 |
| **定时任务** | cron 触发 × 进程内/队列执行；集群选主，不重复执行；在线增删改与执行日志 |
| **代码生成** | 从库表反推 entity / DTO / handler / module / 迁移 / 前端页；单表与树表；模板可覆写；生成物常态化编译校验 |
| **导入导出** | 流式导入导出；大表走 `sys_export_task` 异步任务；10 万行内存稳定有测试守住 |
| **开放 API** | API Key 签发/校验/停用，`scopes` 精确匹配进权限集合 |
| **请求流水线** | request_id、trace、安全头、CORS、gzip、体积上限、单 IP 令牌桶 + 租户 QPS 限流、幂等键 |
| **可观测性** | stdout 结构化日志、`/metrics`（QPS/延迟分位/连接池/缓存命中/任务结果/按码错误率）、OTLP 链路（含 DB 子 span）、慢查询日志 |
| **数据监控** | 只读 PostgreSQL 库/表统计：库大小、连接池、会话数、行数、扫描次数、vacuum/analyze 时间 |
| **工程门禁** | fmt、clippy、`cargo doc -D warnings`、依赖分层、`cargo deny`、`cargo machete`、OpenAPI 快照、模板工程编译、前端 lint/typecheck/build、镜像冒烟 |

## 界面

新增/编辑统一使用弹窗表单；必填校验与后端字段错误都就地显示。

![新增用户弹窗](docs/assets/form-dialog.png)

## 架构

```mermaid
flowchart TB
    UI["Vue 3 管理端<br/>TypeScript · Element Plus · Pinia · Lucide"]
    API["第三方系统 / Open API"]

    subgraph L4["L4 · 接入层"]
        WEB["cicada-web<br/>axum · 中间件链 · OpenAPI"]
        WORKER["cicada-worker<br/>cron · apalis"]
    end

    subgraph L3["L3 · 能力层"]
        AUTH["auth"]
        RBAC["rbac"]
        AUDIT["audit"]
        STORAGE["storage"]
        CODEGEN["codegen"]
    end

    subgraph L2["L2 · 领域层"]
        SYSTEM["system"]
        MONITOR["monitor"]
        JOBS["jobs"]
        NOTIFY["notify"]
    end

    subgraph L1["L1 · 基础设施"]
        INFRA["infra<br/>config · db · cache · telemetry · ratelimit · rls"]
        PORTS["ports<br/>目录 · 事件 · 任务 · 锁"]
    end

    subgraph L0["L0 · 内核"]
        KERNEL["kernel<br/>错误 · 强类型 ID · 上下文 · 权限 · 数据范围"]
    end

    PG[("PostgreSQL 17")]
    REDIS[("Redis 7")]
    OBJ[("本地 / RustFS / OSS / S3")]

    UI --> WEB
    API --> WEB
    WEB --> L3
    WEB --> L2
    WORKER --> JOBS
    L3 --> L1
    L2 --> L1
    L1 --> L0
    INFRA --> PG
    INFRA --> REDIS
    STORAGE --> OBJ
```

**依赖只能向内。** `L4 → L3/L2 → L1 → L0`，L2 领域模块之间不得互相依赖；这条规则由 `deploy/check_layering.py` 在 CI 中强制。跨模块协作通过 `ports` 与领域事件完成。

## 快速开始

### 前置条件

- Rust 1.98+（仓库自带 `rust-toolchain.toml`）
- Docker + Docker Compose（PostgreSQL 17 / Redis 7）
- Node.js 22.12+、pnpm 10（只跑前端时需要）

### 本地开发

```bash
# 1. 起依赖服务（不要起 compose 里的 server，下面用 cargo 直接跑）
docker compose up -d postgres redis

# 2. 建表 → 种子 → 默认租户与管理员
cargo run -p cicada-cli -- migrate
cargo run -p cicada-cli -- seed
cargo run -p cicada-cli -- init-admin

# 3. 起后端
cargo run -p cicada-server
```

后端默认监听 `http://localhost:8081`。

### 前端

```bash
cd web
pnpm install
pnpm dev
```

打开 `http://localhost:5173`。开发服务器把 `/api` 与 `/auth` 代理到 `http://127.0.0.1:8081`，不需要配 CORS。

### 全部用 Docker

```bash
docker compose up -d --build
docker compose exec server cicada-cli migrate
docker compose exec server cicada-cli seed
docker compose exec server cicada-cli init-admin
```

容器内服务监听 8080，映射到宿主机 `http://localhost:8081`。

### 第一次登录

| 项目 | 值 |
| --- | --- |
| 用户名 | `admin` |
| 密码 | `Cicada@2026` |
| 登录接口 | `POST /auth/login`，body：`{"username":"admin","password":"Cicada@2026"}` |
| 管理端 | `http://localhost:5173`（开发）；生产由 Nginx/CDN 托管 `web/dist` |

> **上线前必须改掉默认密码。** 用 `cicada-cli admin reset --user admin --password '<新密码>'`，或登录后在用户管理里重置。

### 常用端点

| 端点 | 说明 |
| --- | --- |
| `GET /healthz` | 进程存活 |
| `GET /readyz` | 就绪探针（检查 PostgreSQL 与 Redis） |
| `GET /metrics` | Prometheus 文本指标 |
| `GET /docs` | Swagger UI |
| `GET /api-docs/openapi.json` | OpenAPI 3.1 文档 |
| `POST /auth/login` | 登录，返回访问令牌与刷新令牌 |
| `/api/v1/...` | 业务 API（统一响应契约） |

## 项目结构

```text
crates/
  kernel/     L0：错误、强类型 ID、RequestContext、权限、数据范围查询状态机
  infra/      L1：config / db / cache / telemetry / ratelimit / rls
  ports/      L1：跨模块端口（目录、事件、任务、锁）与内存 adapter
  system/     L2：用户 / 角色 / 菜单 / 部门 / 岗位 / 字典 / 参数 / 公告 / 日志
  monitor/    L2：在线用户、缓存监控、服务指标
  jobs/       L2：定时任务定义与执行日志
  notify/     L2：通知渠道、模板与投递记录
  auth/       L3：身份解析、密码、会话、JWT、OAuth/OIDC、TOTP、验证码
  rbac/       L3：权限点、角色合并、数据范围计算、授权缓存
  audit/      L3：审计事件、字段级变更、事务性发件箱
  storage/    L3：Storage trait 与本地 / S3 / OSS / RustFS 驱动
  codegen/    L3：库表发现、类型推断、模板渲染、zip 打包
  web/        L4：axum 路由、中间件链、模块注册契约、OpenAPI
  worker/     L4：任务运行时（cron + apalis）
  macros/     权限 / 审计过程宏、DataScoped 派生
  all/        门面 crate `cicada`：bootstrap + 分层别名 + feature 转发
  testkit/    测试夹具（临时 PG、登录辅助、断言宏）
apps/
  server/     框架自身的组装根
  cli/        migrate / seed / init-admin / tenant / admin reset / rls / gen / new
template/     `cicada new` 生成的使用者工程模板
migrations/   sea-orm-migration 迁移工程（从空库建立 schema，append-only）
web/          Vue 3 管理端（独立构建）
config/       分层配置（default / profile / local）
deploy/       Dockerfile、compose、初始化脚本、分层检查
docs/         设计文档、ADR、升级指南、OpenAPI 快照
```

## 建自己的工程（不 fork 框架）

```bash
# 依赖 git tag（推荐）
cargo run -p cicada-cli -- new my-app \
  --cicada 'git = "https://github.com/<org>/cicada", tag = "v0.1.0"'

# 或依赖本地工作区，便于同时改框架
cargo run -p cicada-cli -- new my-app --name my_app --cicada path=$PWD

cd my-app
cargo run --bin migrate -- up --seed --admin
cargo run
```

生成的工程只碰：

- `business/`：业务模块（代码生成器的落点）；
- `migrations/`：业务迁移；
- `templates/`：代码生成模板覆写；
- `config/`：本工程配置。

框架 crate 走依赖，升级只改版本号。装了 `cargo-generate` 也可以 `cargo generate --path template my-app`，产物一致。

## 配置

配置按五档叠加，后者覆盖前者：

```text
代码默认值 < config/default.yaml < config/{profile}.yaml < config/local.yaml < CICADA_* 环境变量
```

- `CICADA_PROFILE=prod` 选择 `config/prod.yaml`；
- `config/local.yaml` 适合本地私有配置（已被 gitignore）；
- 嵌套键用 `__` 分隔，例如 `CICADA_AUTH__JWT__PRIVATE_KEY`；
- `DATABASE_URL` / `REDIS_URL` 是具名变量，优先级更高。

生产环境至少检查这些：

```bash
CICADA_PROFILE=prod
DATABASE_URL=postgres://user:pass@host:5432/cicada
REDIS_URL=redis://host:6379/0
CICADA_AUTH__JWT__PRIVATE_KEY="$(cat jwt-private.pem)"
CICADA_AUTH__JWT__PUBLIC_KEY="$(cat jwt-public.pem)"
CICADA_STORAGE_ACCESS_KEY_SECRET=...
```

`profile=prod` 会拒绝示例 JWT 密钥、过短的 HS256 密钥与不安全的默认值。生产建议使用 EdDSA（Ed25519），并按需配置 CORS、存储驱动与 OTLP 端点。

## 测试与门禁

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# 需要真实 PostgreSQL / Redis
cargo test --workspace -- --ignored
cargo test -p cicada-migrations --test roundtrip -- --ignored

# 依赖与供应链
python3 deploy/check_layering.py
cargo deny check
cargo machete

# 前端
cd web
pnpm lint
pnpm typecheck
pnpm build
```

CI 有四个 job：**rust**（fmt / clippy / doc / 单元 / 集成 / 迁移往返 / 分层 / deny / machete / OpenAPI 快照）、**template**（`cicada new` → 编译 → 迁移）、**frontend**（lint / typecheck / build）、**image**（构建镜像 → 迁移种子 → 冒烟 → 健康检查）。

接口变更需要重新生成 OpenAPI 快照，并把 diff 留在 PR 里：

```bash
CICADA_OPENAPI_BLESS=1 cargo test -p cicada-web --test openapi_snapshot
```

## 安全清单

- 修改默认管理员密码，删除或禁用不需要的初始账号；
- 生产使用 `CICADA_PROFILE=prod`，配置 EdDSA 密钥并轮换 `kid`；
- 数据库、Redis、对象存储凭证只从环境变量或密钥管理注入，不写进仓库；
- 前端与后端不同源时再开 CORS，并列出明确的 origin；
- 高风险租户表按需开启 RLS 兜底（`cicada-cli rls enable --table ...`）；
- 审计日志保留期与归档策略按合规要求调整（`audit.retention_days`）。

## 状态与路线图

当前版本 **0.1.0**，核心基座已交付：

- ✅ 认证、RBAC、数据权限、多租户、系统域、审计、文件、通知、任务、代码生成、导入导出、开放 API、可观测性、Vue 管理端；
- ⏳ K8s 部署清单与压测基线；
- ⏳ 代码生成的主子表（`sub`）模板；
- ⏳ 表单构建等少量前端页面目前是占位页（服务监控与数据监控已实现）。

迁移遵循 append-only；接口与配置的 Breaking 变更记录在 [`CHANGELOG.md`](CHANGELOG.md)，升级动作见 [`docs/UPGRADE.md`](docs/UPGRADE.md)。

## 文档

- [English README](README.en.md)
- [设计文档](docs/superpowers/specs/2026-09-30-cicada-rust-base-design.md)
- [架构决策记录（ADR）](docs/adr/)
- [升级指南](docs/UPGRADE.md)
- [变更日志](CHANGELOG.md)
- 各 crate 的职责、公开 API 与边界：`crates/*/README.md`
- 运行时 OpenAPI：`/docs` 或 `/api-docs/openapi.json`

## 许可

本项目采用 **MIT** 许可，见 [LICENSE](LICENSE)。
