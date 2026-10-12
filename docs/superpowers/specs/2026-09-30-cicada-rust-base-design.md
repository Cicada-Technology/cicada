# cicada 设计文档

> 状态：草案 v0.1（待评审）
> 日期：2026-09-30
> 范围：cicada 基座的技术选型、总体架构、数据层、认证授权、关键机制、里程碑
> 参考实现：同类 Java 后台脚手架的单体、微服务与移动端形态
> 本机环境：rustc 1.98.0 / cargo 1.98.0，PostgreSQL + SeaORM

---

## 0. 文档说明

本文是 cicada 的第一版设计基线。

- **依赖版本策略**：默认跟随最新稳定版，只有在最新版不可用（编译失败、行为回归、许可或安全不合规）时才降级到最近的可用版本。第 4 章的版本号是 2026-09-30 当天 crates.io 的实测快照，仅作参考；实现时以当时的稳定版为准，锁定在 `Cargo.lock`。
- 凡标为「决策」的条目是已定选择，后续变更需要以 ADR 形式记录；凡标为「待决策」的条目集中在第 15 章。
- 附录 C–G 是正文的补充目录：错误码、权限点、接口与权限点对照、P1/P2 表结构、数据边界。

---

## 1. 背景与目标

### 1.1 从参考实现得到的结论

对参考实现的源码与 SQL 的实际盘点结果：

| 事实 | 数据 |
| --- | --- |
| 内置权限点（sys_menu 记录） | 85 条：4 个目录（M）+ 19 个菜单（C，含 1 个外链）+ 62 个按钮（F）；其中 80 条带权限标识（18 个 C + 62 个 F），完整清单见附录 D |
| 系统表 | 21 张（不含 Quartz 的 11 张 QRTZ_* 表） |
| 官方形态 | 单体、单应用、微服务、移动端 |
| 核心机制 | Shiro 认证授权、AOP 数据权限、MyBatis 动态 SQL、Thymeleaf 服务端渲染、Quartz、代码生成器 |

这类后台基座真正可复用的只有四类东西：**RBAC 权限模型**、**组织/字典/参数等基础数据**、**审计日志**、**代码生成器与定时任务等开发效率工具**。它作为业务系统的价值约等于 0，作为脚手架的价值接近全部。cicada 对准的就是这个位置。

### 1.2 cicada 的目标

1. 在 Rust 生态中提供一套等价甚至更强的后台基座：拿得走、改得动、编得过。
2. 以编译期正确性替代运行期反射：漏加权限、漏过滤租户、漏做软删除都应在编译期或测试期暴露。
3. 数据库统一 PostgreSQL + SeaORM，一期不追求多数据库方言兼容。
4. 沿用参考实现的权限标识与菜单命名（`system:user:add`、`M`/`C`/`F`），便于理解和录入；前端与响应包体都按 cicada 自己的规范（标准 REST，见 7.1、8、10），不复用同类 Vue3 前端，也不沿用其 `{code,msg,...}` 包体。

**一句话定位**：cicada 是一个「开箱即用的生产级后台基座模板」——与参考实现目标一致，它不是产品，而是脚手架：把后台系统里反复出现、又容易做错的技术选择（RBAC 与数据权限、多租户、审计、认证与身份、代码生成、存储、迁移、测试脚手架、部署）一次性做完做好，并**默认集成**，开发者只需写自己的业务模块。其中最核心的价值是**代码生成**：把 CRUD/树表/主子表、权限点、前端页面、迁移这些重复劳动一键产出，且开箱可编译。

### 1.3 非目标（明确不做）

- 服务端页面渲染（Thymeleaf 那套）。cicada 只做 API。
- 在线拖拽表单构建器、Druid 式内嵌数据库控制台。
- 一期不做多数据库适配；不自研 BPMN 工作流引擎（走适配层对接外部引擎）。
- 不做 JVM 式的运行期插件热加载。

### 1.4 成功判据

- 从空库到可登录、可管理用户/角色/菜单，全流程 ≤ 10 分钟（含迁移与初始化管理员）。
- 新增一个业务模块（表 → 实体 → 迁移 → 增删改查 → 权限点 → 前端页面）由代码生成器在 5 分钟内产出，且无需手改即可编译运行。
- 单元测试 + 集成测试覆盖 P0 权限矩阵全部分支；关键路径有并发的会话踢出与乐观锁测试。
- 交付物为单个静态链接二进制 + `docker compose up` 可跑通的全栈环境。

---

## 2. 形态与设计原则

### 2.1 形态决策

**决策：cicada = 内核 crate 组 + 可插拔领域模块 + 可运行的组装工程，三者同仓（Cargo workspace）。**

对比过的三种形态：

| 形态 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| A. 模板仓库（clone 即改，等价参考实现单体） | 上手快 | 升级困难，分叉后无法回灌上游补丁 | 否 |
| B. 库 crate 组 + 组装工程 | 可依赖、可升级、可裁剪；编译期边界清晰 | 需维护 crate 版本与 feature 矩阵 | **采用** |
| C. 微服务底座（等价微服务形态） | 独立部署、独立扩缩 | 复杂度高于单体，需额外承担服务发现/链路/契约成本 | **同期交付**：与单体共享同一套 crate，只在组合根与 adapter 分叉（16.2、12 章） |

形态 B 需要额外补两件东西，才能真正覆盖 A 和 C 的用法（详见 16.3 与 16.2）：

- 补齐「模板仓库」体验：用 `cargo-generate` 模板（`cicada new`）生成**使用者自己的工程**，业务代码落在使用者的 `business/` 目录，框架 crate 通过 git tag 依赖，永不被 fork 修改。这样既保留 A 的低门槛，又避免 A 的升级困境。
- 补齐「微服务」体验：L2 模块之间必须靠**数据所有权 + port/adapter** 隔离，而不是靠共享 `Db` 与跨模块 JOIN（16.2 给出规则）。

### 2.2 命名与接口策略

**决策：沿用后台系统通用的数据与权限标识命名，但不复用其实现方式。**

- 保留：表名 `sys_*` / `gen_*`、权限标识 `system:user:add`、菜单树结构（`M` 目录 / `C` 菜单 / `F` 按钮）。
- 不保留：参考实现的响应包体 `{code,msg,total,rows}` 与 `X-Biz-Code`——cicada 用标准 REST（HTTP 语义 + RFC 9457 `problem+json`，见 7.1、8）；前端也不复用同类 Vue3 前端（v1 是 cicada 自研 Vue 工程，见 10）。
- 替换：`del_flag char(1)` → `deleted_at timestamptz`；`datetime` → `timestamptz`；Shiro 会话 → JWT + Redis；AOP 数据权限 → 显式 `Scope`（数据范围）参数 + PG RLS 兜底；Thymeleaf → 无。
- 迁移只从空库建立 cicada 自己的 schema；已有数据由使用者在业务侧自行处理。

### 2.3 设计原则（「最 Rust」的那部分）

1. **类型即契约**：新类型 `UserId` / `DeptId` / `TenantId` / `Permission` 避免 ID 混用；非法状态用枚举而非字符串。
2. **显式优于隐式**：没有 AOP、没有反射、没有运行期容器扫描。横切关注点走 tower `Layer`，数据权限走方法参数，装配走组合根。
3. **错误是类型**：`thiserror` 定义 `AppError`，只有传输层做 `IntoResponse`；库 crate 的公开 API 不出现 `anyhow`。
4. **所有权即边界**：跨模块只传 `Arc<dyn Trait>` 或值对象，不传具体实现；业务代码不持有连接，只持有 `DatabaseConnection` 句柄。
5. **共享不可变状态**：`AppState` 只放 `Arc` 共享的只读依赖，可变状态一律进数据库或 Redis；杜绝全局可变与 `lazy_static<Mutex<_>>`。
6. **唯一扩展点**：所有可替换能力（存储、通知、鉴权策略、任务执行器）都以 trait 暴露，默认实现内置于模块，使用者可注册自定义实现。
7. **编译期校验优先**：能由类型系统保证的不写运行期断言；能在 CI 跑 clippy `-D warnings` 的不靠 review 记忆。

---

## 3. 总体架构

### 3.1 分层

```
┌─ L4 接入层 ─────────────────────────────────────────────────────────────┐
│  cicada-web     axum 路由、中间件链、统一响应、OpenAPI、模块契约        │
│  cicada-worker  后台任务/定时任务运行时（可选独立进程）                 │
│  cicada-rpc      tonic gRPC 服务间通信（仅微服务版）                    │
├─ L3 能力层 ─────────────────────────────────────────────────────────────┤
│  cicada-auth     认证：账号/手机/邮箱/微信身份解析与 JWT 签发校验       │
│  cicada-rbac     授权：权限点解析、角色合并、数据范围计算、缓存         │
│  cicada-audit    审计：审计宏、字段级变更留痕、审计记录生产者           │
│  cicada-storage  文件存储（opendal：本地/OSS/RustFS/S3）                │
│  cicada-codegen  代码生成：库表发现 → 模板渲染 → 打包下载               │
├─ L2 领域层 ─────────────────────────────────────────────────────────────┤
│  cicada-system   用户/身份绑定/角色/菜单/部门/岗位/字典/参数/公告/日志  │
│  cicada-monitor  在线用户/定时任务/缓存/服务指标/日志只读管理视图       │
├─ L1 基础设施 ───────────────────────────────────────────────────────────┤
│  cicada-infra    db / cache / config / telemetry / mq 作为内部模块      │
│  cicada-ports    跨模块端口（trait）与本地/远程 adapter 的公共契约      │
├─ L0 内核 ───────────────────────────────────────────────────────────────┤
│  cicada-kernel   错误类型、分页、RequestContext、ID、时间、脱敏、       │
│                 值对象、trait 契约（Clock/IdGenerator/CurrentUser）     │
└─────────────────────────────────────────────────────────────────────────┘
```

### 3.2 crate 布局

```
cicada/
├── Cargo.toml                 # workspace：统一 edition 2024、依赖版本、lint 策略
├── rust-toolchain.toml        # 固定工具链，保证 CI 与本地一致
├── crates/
│   ├── kernel/                # L0：错误、值对象、RequestContext、Scope、trait 契约
│   ├── infra/                 # L1：config / telemetry / db / cache / mq 为内部模块
│   ├── ports/                 # 跨模块端口（trait）+ 本地/远程两种 adapter 的公共定义
│   ├── auth/  rbac/  audit/  storage/  codegen/     # L3 能力，feature 可裁剪
│   ├── system/  monitor/  notify/   # L2 领域模块，各自含 entity/service/handler/dto
│   ├── web/                   # 中间件链、模块注册契约（Module/Manifest）、路由装配、OpenAPI 汇总
│   ├── worker/
│   ├── macros/                # cicada-macros 过程宏：权限声明宏、审计宏
│   ├── all/                   # 单个门面 crate：re-export + feature 转发，供快速上手
│   └── testkit/               # 测试夹具：临时 PG、登录辅助、断言宏
├── apps/
│   ├── server/                # 组装根：读配置 → 建依赖 → 挂模块 → 起服务
│   └── cli/                   # cicada-cli：migrate / gen / admin reset / tenant init
├── template/                  # cargo-generate 模板（生成使用者的独立工程）
├── migrations/                # sea-orm-migration 迁移工程
│   └── src/                   # Rust 迁移（类型安全、可回滚）
├── web/                       # 前端（Vue3 + TS + Element Plus），独立构建
├── docs/                      # 本文档、ADR、OpenAPI 产物
└── deploy/                    # Dockerfile、compose、k8s、初始化脚本
```

使用者工程的形态（由 `template/` 生成，业务代码与框架彻底分离）：

```
my-app/
├── Cargo.toml                 # cicada = { git = "...", tag = "v0.1.0", features = [...] }
├── src/main.rs                # 组合根：装配 cicada 模块 + 自己的业务模块
├── business/                  # 业务模块（代码生成器产出的落点），永不与框架代码混放
├── migrations/                # 业务迁移，与框架迁移分目录、分表前缀
├── templates/                 # 覆写代码生成模板（可选）
└── config/                    # 分层配置
```

### 3.3 依赖规则（CI 强制）

1. `kernel` 不依赖任何其他 cicada crate。
2. L1 只能依赖 L0；L2 只能依赖 L1/L0；L3 可依赖 L1/L0，并通过 trait 使用 L2 的抽象。
3. L2 领域模块之间**不得**互相依赖具体实现；跨模块协作走 `ports` 定义的端口（trait）或领域事件（见 3.3 第 6 条、16.2）。
4. 只有 `apps/*` 允许依赖所有层；只有 `apps/*` 允许读环境变量。
5. `cargo-deny` 卡重复版本、许可证与已知漏洞；`cargo-machete` 清理无用依赖；`cargo-hakari` 可选加速编译。
6. **数据所有权**：任何模块只读写自己拥有的表；跨模块取数只能经 `ports` 中的端口（trait），不得直接 JOIN 别人的表。领域模块的 facade 是它唯一的对外接口（16.2）。
7. **缝（seam）纪律**：只有当同一个接口存在两个 adapter（本地实现 + 远程/内存实现）时才把它提成 `ports` 中的端口；只有一个实现的"端口"只是多余的间接层，不入库。
8. **模块只声明事实**：领域模块通过注册契约声明自己拥有的表、路由、权限点、菜单与种子（16.1），装配顺序、冲突检测与校验由组装根统一完成；模块作者不排顺序、不碰中间件链，也不直接写别人的表。

### 3.4 请求生命周期

tower Layer 由外向内（axum 的 `layer` 调用顺序与执行顺序相反，装配代码里会显式标注）：

```
请求
 └─ 1  request_id        生成/透传 X-Request-Id
    └─ 2  trace          tracing span：方法/路径/耗时
       └─ 3  security_headers  X-Content-Type-Options / CSP / HSTS
          └─ 4  cors
             └─ 5  compression
                └─ 6  body_limit    请求体上限（上传路由例外）
                   └─ 7  rate_limit governor 令牌桶，按 IP/用户/接口维度
                      └─ 8  auth      Bearer → 校验 JWT → 载入 Principal
                         └─ 9  rbac   校验权限标识 + 计算 DataScope → RequestContext
                            └─ 10 handler  提取器取参 → service → repository(SeaORM)
                               └─ 11 audit   按审计宏记录操作日志与变更快照
                                  └─ 12 response  序列化资源 + problem+json 错误映射
```

`RequestContext` 是随请求流转的不可变快照，挂在请求扩展上：

```rust
pub struct RequestContext {
    pub request_id: RequestId,
    pub principal: Option<Principal>,   // 匿名请求为 None
    pub tenant_id: TenantId,
    pub data_scope: DataScope,          // 已按角色合并计算完毕
    pub ip: IpAddr,
    pub user_agent: Option<HeaderValue>,
    pub trace_id: String,
}
```

持久层不直接消费 `RequestContext`：service 在边界把它收敛为窄接口 `Scope { tenant_id, user_id, data_scope }`（见 6.3），因此 DB 测试无需构造 HTTP 上下文。

**实现状态（2026-10-10）**：1–8 层与 11 层均已按上述顺序装配（`crates/web/src/router.rs` 显式倒序
书写并逐步注释）：1/2/4/5 层用 `tower-http`，3 `security_headers` 与 6 `body_limit` 自研
（CSP 需按路径放开 Swagger UI，请求体上限需按路由分档：上传/导入例外），7 限流与 8 认证见
ADR 0005，9 权限守卫与 11 审计为路由级 Layer。细节与代价见 ADR 0006。

---

## 4. 技术选型

### 4.1 选型表（版本为 2026-09-30 crates.io 实测快照；实现时跟随最新稳定版，不可用再降级）

| 关注点 | 选型 | 版本 | 说明 |
| --- | --- | --- | --- |
| 异步运行时 | tokio | 1.53.1 | 按需启用 feature，不用 `full` 蒙上去 |
| Web 框架 | **axum** | 0.8.9 | 与 tower/tower-http 同一模型，中间件即 Layer |
| 中间件 | tower / tower-http | 0.5.3 / 0.7.1 | trace、cors、compression、limit、timeout |
| 限流 | governor（直接用，未用 tower_governor） | 0.10.4 | 令牌桶；KeyExtractor 即 key 类型（IP / 租户），见 ADR 0005 |
| ORM | **sea-orm** | 2.0.4 | 底层 sqlx 0.9，动态查询用 sea-query 1.0.2 |
| 迁移 | sea-orm-migration | 2.0.4 | Rust 写迁移，可回滚，可复用实体类型 |
| 库表发现 | 直查 information_schema / pg_catalog | — | 表注释、自增、主键用 `pg_catalog` 一次查全，比 sea-schema 少一层抽象（7.7） |
| CLI | sea-orm-cli | 2.0.4 | `generate entity` / `migration` |
| 密码 | argon2 | 0.6.0 | Argon2id，参数与升级策略见 9.1 |
| JWT | jsonwebtoken | 11.1.0 | EdDSA(Ed25519) 优先，支持 HS256 回退 |
| 缓存 | redis / deadpool-redis / moka | 1.7.1 / 0.23.1 / 0.12.16 | Redis 分布式 + moka 进程内两级 |
| 服务端会话 | tower-sessions（+ sqlx/redis store） | 0.15.0 | 一期未采用（会话见 6.1 自研 JWT + Redis），仅作备选保留 |
| 授权 | 自研 RBAC + sea-orm `rbac` 特性（评估） | 2.0.4 | 详见 6.2 |
| 校验 | validator / garde | 0.21.0 / 0.23.0 | derive 校验 + 条件校验 |
| 序列化 | serde / serde_json | 1.0.229 / 1.0.151 | — |
| 时间 | chrono（SeaORM `with-chrono`） | 0.4.45 | 统一 UTC 存储，列类型 `timestamptz` |
| ID | uuid | 1.26.1 | 主键策略见 5.2 |
| 金额 | rust_decimal | 1.43.0 | 禁止用 f64 表示金额 |
| 配置 | config + serde + dotenvy | 0.15.27 | YAML + 环境变量覆盖 |
| 日志/追踪 | tracing / tracing-subscriber | 0.1.44 / 0.3.23 | JSON 输出，字段化 |
| 可观测 | opentelemetry / opentelemetry-otlp | 0.33.0 | OTLP 导出 |
| 指标 | metrics-exporter-prometheus | 0.18.3 | `/metrics` 暴露 |
| API 文档 | utoipa / utoipa-swagger-ui | 6.0.0 / 10.0.1 | derive 生成 OpenAPI 3.1 |
| 模板（仅代码生成/邮件） | minijinja | 2.24.0 | 语法接近 Jinja2，便于从参考实现模板迁移 |
| 邮件 | lettre | 0.11.23 | SMTP |
| HTTP 客户端 | reqwest（rustls） | 0.12.x | 微信 OAuth2、跨服务 adapter、外部 Webhook；不用 openssl |
| 第三方登录 | 自研协议 adapter（reqwest） | — | 微信开放平台与通用 OIDC 共用同一端口，见 ADR 0007；未引入 `oauth2` crate |
| 文件存储 | opendal | 0.59.3 | 本地 / RustFS（S3 兼容）/ 阿里云 OSS / S3 一套 API，按配置切换驱动 |
| Excel 导出 | rust_xlsxwriter | 0.99.1 | 写 xlsx |
| Excel 导入 | calamine | 0.36.1 | 读 xlsx/xls |
| 定时任务触发 | tokio-cron-scheduler / cron | 0.15.1 / 3.0.0 | 进程内 cron 到点产出（集群下仅 leader） |
| 任务队列执行 | apalis + apalis-sql | 0.7.4 | `PostgresStorage`（`SKIP LOCKED` + `NOTIFY`）做可跨重启的任务队列，多消费者 |
| 消息 | lapin（RabbitMQ） | 4.12.0 | 可选模块 |
| 验证码/TOTP | captcha / totp-rs | 1.0.0 / 6.0.0 | 图形验证码与二次验证 |
| 国际化 | rust-i18n | 4.2.4 | 服务端消息；前端用 vue-i18n |
| 打包 | zip | 8.6.0 | 代码生成器打包下载 |

### 4.2 关键取舍

**axum 而非 actix-web**：actix-web 4.15 性能指标略好、生态也成熟，但它的中间件是自有的 `Transform` 模型，与本项目「一切横切关注点都是 tower Layer」的设计不一致；axum 与 tower、tower-sessions、tower_governor、axum-extra 同族，能把请求流水线做成可组合、可单测的 Layer 栈。管理后台的 QPS 量级是 10^2–10^3，性能不是瓶颈，一致性更重要。

**SeaORM 而非裸 sqlx**：本题已定 SeaORM，需要认清它的边界。

- 优点：实体、关系、迁移、分页、软删、乐观锁开箱即用；底层就是 sqlx 0.9，必要时可通过 `DatabaseConnection` 拿到 `sqlx::PgPool` 逃生。
- 代价：复杂聚合与报表不如手写 SQL 直观；动态条件要靠 `QueryFilter` 链式拼装或 `sea_query` 表达式。
- 约定：**CRUD 用 SeaORM 实体 API；复杂查询用 `sea_query` 构建 `Statement` 再交给 SeaORM 执行**，全程不拼字符串。

**PostgreSQL 而非 MySQL**：`timestamptz`、`jsonb`、部分索引、`ROW LEVEL SECURITY`、窗口函数、`COPY` 批量导入都是本项目的实际刚需；RLS 还能作为数据权限的第二道防线（见 6.3）。

**JWT + Redis 会话而非纯无状态 JWT**：管理后台必须支持强制下线、在线用户、改密即失效、同账号最大会话数——这些都是参考实现的既有能力，纯无状态 JWT 做不到。具体见 6.1。

### 4.3 客户端技术栈

后端只出 REST API，与前端解耦。第一版只交付 Web（Vue），其余为后续版本；所有端共用同一套 REST 接口。

| 端 | 技术 | 说明 |
| --- | --- | --- |
| Web（v1） | Vue 3 + TS + Vite + Element Plus + Pinia + vue-i18n | 前后端分离，独立部署；详见 10 章 |
| Web（后续） | React 19 + TS、Svelte + TS | 复用同一 REST API 与权限标识，各自独立成库 |
| 小程序 | uni-app（Vue 语法） | 小程序内部登录（`wx.login`），见 6.1 |
| 桌面/移动 | Tauri + TS + Rust | Rust 只做壳（窗口/系统能力/自动更新），业务走后端 REST |

权限判断（`hasPermi` 一类）由各端自行实现，后端只下发权限标识集合与菜单树，不假设任何前端框架。

---

## 5. 数据层设计

### 5.1 连接与事务

```rust
// cicada-infra::db
pub struct Db {
    conn: DatabaseConnection,   // 内部即连接池句柄，克隆成本极低
}

impl Db {
    pub async fn connect(cfg: &DbConfig) -> Result<Self, DbError> {
        let mut opt = ConnectOptions::new(cfg.url.expose_secret().to_owned());
        opt.max_connections(cfg.max_connections)
            .min_connections(cfg.min_connections)
            .acquire_timeout(Duration::from_secs(cfg.acquire_timeout_secs))
            .sqlx_logging(false)                 // 由 tracing 统一接管
            .set_schema_search_path(cfg.schema.clone());
        let conn = sea_orm::Database::connect(opt).await?;
        Ok(Self { conn })
    }
}
```

约定：

- 事务用 `TransactionTrait::begin()`。`DatabaseTransaction` 自身实现 `ConnectionTrait`，因此 service 方法写成 `impl ConnectionTrait` 就能「既能在事务里跑、也能在事务外跑」。
- 事务边界放在 service 层，repository 不自行开事务；跨模块写操作由调用方统一开启事务。
- 连接池默认 `max_connections = CPU 核数 * 2`，PG 前置 pgbouncer 时下调。
- 慢查询阈值可配，超阈值以 `warn` 记录 SQL 摘要与耗时（不记录参数值）。
- 关库前 `close().await` 等待连接归还，配合优雅停机。

### 5.2 建模规范

| 维度 | 规范 | 理由 |
| --- | --- | --- |
| 表名 | 系统表沿用 `sys_*` / `gen_*` | 沿用既有命名习惯 |
| 主键 | 系统表 `BIGINT GENERATED BY DEFAULT AS IDENTITY`；对外暴露或需分布式生成的资源另加 `uuid uuid NOT NULL DEFAULT gen_random_uuid()` 唯一列 | 索引小、迁移省事；对外不暴露自增序列 |
| 时间 | 一律 `timestamptz`，Rust 侧 `DateTime<Utc>` | 避免时区歧义，展示层本地化 |
| 软删除 | `deleted_at timestamptz NULL`；实体含该列时 SeaORM 自动把 `delete()` 变成软删 | 保留「删除可追溯」语义 |
| 乐观锁 | 可变业务实体加 `version int NOT NULL DEFAULT 0`；更新走 `UPDATE … WHERE version = 期望值` 并把版本 +1（SeaORM 2.0.4 **没有** `#[sea_orm(version)]` 这个属性，也没有原生 opt-lock，只能手写 CAS，见 §13 补记） | 并发编辑报明确冲突，而非静默覆盖 |
| 审计列 | `create_by / create_time / update_by / update_time / remark` | 与参考实现一致 |
| 状态 | 枚举列 + `DeriveActiveEnum`，不用魔法字符串 | 编译期穷尽匹配 |
| 命名 | 库内 `snake_case`，Rust `PascalCase`，字段映射显式写 `column_name` | 消除隐式转换 |
| 金额 | `numeric(18,4)` + `rust_decimal::Decimal` | 禁止 f64 |
| 大对象 | 不入库，存 opendal 路径 + 元数据表 | 备份与容量解耦 |

示例实体：

```rust
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "sys_user")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub user_id: i64,
    pub tenant_id: i64,
    #[sea_orm(unique)]
    pub user_name: String,
    pub nick_name: String,
    pub dept_id: Option<i64>,
    #[serde(skip_serializing)]
    pub password: String,
    pub email: Option<String>,
    pub phonenumber: Option<String>,
    pub status: UserStatus,              // DeriveActiveEnum: Active / Disabled
    pub login_ip: Option<String>,
    pub login_date: Option<DateTime<Utc>>,
    pub version: i32,
    pub deleted_at: Option<DateTime<Utc>>,
    pub create_by: Option<String>,
    #[sea_orm(column_type = "TimestampWithTimeZone")]
    pub create_time: DateTime<Utc>,
}
```

### 5.3 分页与动态查询

- 分页参数统一 `page`（从 1 开始）+ `page_size`（默认 20，上限 200；超限报错而不是静默截断）。
- 列表查询用 `QueryFilter` 链式拼装 + `PaginatorTrait::paginate(db, page_size).fetch_page(page)`；注意 SeaORM 的 `fetch_page` 以 0 为首页，`page`（从 1 开始）需减 1 再传入。
- 模糊查询用 PG 原生 `ILIKE`（`sea_query::extension::postgres::PgExpr`），高频模糊列建 `pg_trgm` GIN 索引。
- 排序字段白名单：由实体列枚举映射得到，杜绝 `ORDER BY {用户输入}`。
- 复杂聚合走 `sea_query` 构建 `Statement`，参数全部绑定。

```rust
// repository：只接收窄接口 Scope，不接触 RequestContext（见 6.3）
let mut q = SysUser::find();

if let Some(kw) = params.keyword.as_deref() {
    q = q.filter(
        Condition::any()
            .add(Column::UserName.ilike(format!("%{kw}%")))
            .add(Column::NickName.ilike(format!("%{kw}%"))),
    );
}
q = scope.apply::<SysUser>(q);   // 一次叠加租户 + 软删 + 数据范围，详见 6.3

let page = q.order_by_desc(Column::UserId)
    .paginate(db, params.page_size)
    .fetch_page(params.page.saturating_sub(1))   // fetch_page 0 基：第 1 页传 0
    .await?;
```

### 5.4 迁移策略

- 用 `sea-orm-migration` 把迁移写成 Rust 代码而非裸 SQL，以便复用 `sea_query` 类型与枚举；每个迁移实现 `up` / `down`。只有插入原始数据允许 SQL 片段。
- 一张表只在一个迁移里建出**最终形态**，不做「先建表、后续迁移再加列/改列」。
- 已合并到主干的迁移文件不再修改，只追加新迁移。CI 校验「空库 up → down → 空库」可往返。
- 生产遵循 expand/contract：加可空列 → 双写与回填 → 切读 → 删旧列，避免停机。
- 初始化分三档且全部幂等：`migrate`（结构）→ `seed`（字典/菜单/权限点 upsert）→ `init-admin`（初始租户与管理员）。
- `migrate` 只从空库建立 cicada 的 schema；已有数据由使用者在业务侧自行处理。

### 5.5 数据模型清单

**P0（系统域）**

| 表 | 说明 | cicada 的设计 |
| --- | --- | --- |
| `sys_tenant` | 租户 | 新增 |
| `sys_user` | 用户（账号，一人一行） | + `tenant_id`、`version`、`deleted_at` |
| `sys_user_identity` | 登录身份绑定（账号名/手机/邮箱/微信…） | 新增；属主 `system`；唯一键 `(tenant_id, identity_type, identifier)`，见 6.1 |
| `sys_dept` | 部门（树） | + `tenant_id`，保留 `ancestors` |
| `sys_role` | 角色 | + `tenant_id`，保留 `data_scope` 枚举 |
| `sys_menu` | 菜单/权限点 | 保留 `menu_type` M/C/F 与 `perms` |
| `sys_post` | 岗位 | + `tenant_id` |
| `sys_user_role` / `sys_user_post` / `sys_role_menu` / `sys_role_dept` | 关联表 | 加租户一致性约束 |
| `sys_dict_type` / `sys_dict_data` | 字典 | — |
| `sys_config` | 参数 | 支持按租户覆盖 |
| `sys_notice` / `sys_notice_read` | 公告 | 已读用 `(user_id, notice_id)` 唯一键 |
| `sys_oper_log` / `sys_logininfor` | 操作/登录日志（属主 `system`） | 按月分区 + 归档策略；`monitor` 只读管理，`audit` 经 outbox 生产（见 7.2） |
| `sys_user_online` | 在线会话（审计快照，属主 `auth`） | 权威在 Redis；此表为异步 write-behind 快照，可重建、不参与踢出/上限判定。会话生命周期写它，历史读走 `GET /api/v1/monitor/online-sessions`（见 6.1 补记） |
| `sys_job` / `sys_job_log` | 定时任务 | cron 表达式，执行日志分区；队列执行侧另建 `apalis.jobs` / `apalis.workers`（由 apalis-sql 自带迁移创建，见 7.6） |
| `gen_table` / `gen_table_column` | 代码生成元数据 | 复用 |

**P1 新增**：`sys_file`（文件元数据）、`sys_audit_log`（字段级变更留痕）、`sys_api_key`（开放接口凭证）、`sys_export_task`（异步导出）、`outbox_event`（事务性发件箱，见 16.2）。字段设计见附录 F。

**P2 新增**：`sys_oauth_client` / `sys_oauth_token`（未建，见附录 F 同段的说明）、`sys_notify_channel` / `sys_notify_template` / `sys_notify_record`（通知中心）、`sys_tenant_package`（租户套餐与限额）。字段设计见附录 F。

---

## 6. 认证与授权

### 6.1 认证

**决策：一账号多身份（identity）+ 双令牌 + Redis 会话表。**

```
POST /auth/login          标识符（账号名/手机/邮箱）+ 密码(+验证码|TOTP) → access_token(JWT 15min) + refresh_token(不透明 7d)
POST /auth/login/wechat   微信扫码登录（开放平台网站应用 OAuth2）：code → openid/unionid → 已绑定则发令牌，未绑定则进入绑定
GET  /auth/login/{provider}/authorize-url  第三方登录通用入口：取授权地址 + 一次性 state
POST /auth/login/{provider}/callback       第三方登录回调：已绑定发令牌 / 未绑定发绑定票据
POST /auth/identities/oauth                用绑定票据把外部身份绑到当前账号
POST /auth/login/miniapp  小程序登录：wx.login 的 code → code2session 换 openid → 已绑定则发令牌
POST /auth/refresh        用 refresh_token 换新 access_token，并轮换 refresh_token
POST /auth/logout         撤销当前会话
GET  /auth/profile        当前用户信息 + 已绑定身份 + 角色 + 权限标识集合 + 菜单树
                          （cicada 把这三份事实拆成三个接口：profile 给「用户 + 角色 + 权限」，
                           已绑定身份见下面的 `GET /auth/identities`，菜单树见 `GET /auth/routers`——
                           前端登录后各拿一次即可合成，不必让 profile 每次重载整棵树）
GET  /auth/routers        当前用户可见的前端路由（菜单树）
GET  /auth/identities     当前账号已绑定的登录身份
POST /auth/identities     绑定新身份（手机/邮箱需验证码；微信走 OAuth2 回调）
DELETE /auth/identities/{id}  解绑身份（须保留至少一个可登录身份）
```

**登录身份模型（一人一账号、多身份绑定）**

- **账号与身份分离**：`sys_user` 是账号（一个自然人一行），`sys_user_identity` 是它可用来登录的身份，一对多。这是「一人一账号、手机/邮箱/微信都能绑在一起」的落点。
- 身份类型 `identity_type`：`Username`（账号名，与参考实现 `user_name` 对齐）/ `Phone` / `Email` / `WeChatOpen`（PC 扫码）/ `WeChatMp`（小程序）；后续可扩展。外部身份另存 `provider` 列（如 `wechat:open` / `wechat:mp`），`identifier` 存 `openid`（拿到时优先 `unionid`）。
- **两种微信登录是不同流程**：扫码登录用于 PC Web（OAuth2 授权码）；小程序登录走小程序内部登录（`wx.login` 拿 code → 后端 `code2session` 换 `openid`/`session_key`，`session_key` 不落库，仅用于解密手机号等）。两者绑到同一个 `sys_user`，互不冲突。
- 唯一键 `(tenant_id, identity_type, identifier)`：同一租户内一个手机/邮箱/微信只能绑定到一个账号，杜绝两个账号共用一个手机号。
- 账号凭据仍留在账号上：`sys_user.password`（Argon2id）是账号级密码，手机/邮箱登录即「解析身份 → 账号 → 校验该密码」，不引入第二套密码。
- 镜像列：`sys_user.user_name` / `phonenumber` / `email` 作为对应**主身份**（`is_primary`）的镜像，在同一事务内同步；登录查找一律以 `sys_user_identity` 为准。
- 登录失败统一返回同一业务码，不区分「身份不存在」与「凭据错误」，避免账号枚举。
- 自助注册默认关闭（管理后台由管理员建号）；若开放，注册时至少写入一个身份。短信验证码登录需要短信通道，作为可选扩展（见 7.10）。

- `access_token`：JWT，claims 含 `sub`(user_id)、`tid`(tenant_id)、`jti`(session_id)、`exp`；签名优先 EdDSA，密钥来自环境变量或密钥管理服务，支持 `kid` 多密钥轮换。
- `refresh_token`：不透明随机串，库中只存哈希。Redis 记录 `session:{jti} → {user_id, tenant_id, refresh_hash, ip, ua, login_time, last_seen}`。
- 强制下线、改密即失效、最大会话数全部通过增删 Redis 会话实现；未过期的 access_token 用 `jti` 黑名单兜底（黑名单 TTL = token 剩余有效期）。
- 在线用户以 **Redis 为唯一权威源**：会话的创建/续期/踢出/最大会话数/过期全部直接落在 Redis（`session:{jti}` 与在线集合）。`sys_user_online` 只是**异步 write-behind 的审计快照**——最终一致、可延迟、可随时从 Redis 重建，**不参与任何踢出/上限判定**；「在线用户」管理页读 Redis，历史审计读快照表。
- 密码：Argon2id，参数写进哈希串，登录时可判断是否需要重哈希升级。登录失败计数与锁定用 Redis 计数器（对应参考实现的 `maxRetryCount`）。
- 可选增强：TOTP 二次验证（totp-rs）；微信扫码登录一期落地（OAuth2 授权码，见上），小程序登录随小程序端落地，企业 SSO（通用 OIDC/OAuth2 提供方）放二期。
- **实现状态（2026-10-10）**：第三方登录已按「端口 + adapter」落地（ADR 0007）：配置里声明
  `auth.oauth.providers`（`kind: wechat | oidc`），走
  `GET /auth/login/{provider}/authorize-url` + `POST /auth/login/{provider}/callback`；
  未绑定**不自动建号**，只发短时效绑定票据，由已登录用户调 `POST /auth/identities/oauth` 绑定。
  小程序登录（`code2session`）仍未实现，`identity_type = WeChatMp` 已预留。

### 6.2 权限模型

三层，语义与参考实现一致：

1. **菜单/路由权限**：`sys_menu` 的 `M` / `C` 节点决定前端菜单树与可见路由，由 `GET /auth/routers` 下发。
2. **接口/按钮权限**：权限标识 `模块:资源:操作`，例如 `system:user:add`、`system:role:edit`。后端每个写接口强制声明，前端按钮由各端自行判断（v1 Vue 用 `v-hasPermi`，其余框架各自实现，见 4.3、10）。
3. **数据范围权限**：见 6.3。

后端强制声明（Rust 没有注解，用过程宏）：

```rust
#[derive(Deserialize, Validate)]
pub struct CreateUserReq { /* ... */ }

#[cicada_macros::require_perm("system:user:add")]
#[cicada_macros::audit(module = "用户管理", action = "新增")]
pub async fn create_user(
    State(st): State<AppState>,
    ctx: RequestContext,
    Json(req): Json<CreateUserReq>,
) -> Result<(StatusCode, Json<SysUserVo>), AppError> {
    let vo = st.system.users.create(&ctx, req).await?;
    Ok((StatusCode::CREATED, Json(vo)))
}
```

> 实现说明：`Perm<"system:user:add">` 这类「字符串常量泛型」在稳定版 Rust 不可用（`adt_const_params` 未稳定），因此权限与审计走过程宏。宏在编译期把权限标识写进路由元数据，注册路由时由 `cicada-web` 统一收集——既能做运行时校验，也能导出权限清单给 OpenAPI 与前端。

权限判定流程：`user → roles → menus(perms) → 权限集合`，集合缓存在 Redis（key 含 `user_id` 与角色版本号）；角色或菜单变更时按版本号整体失效。超级管理员短路通过。

授权引擎的落地顺序：

- **主选**：自研 `PermissionSet` + 过程宏 + Redis 缓存。可控、可测、与参考实现语义完全对齐。
- **评估项**：SeaORM 2.0 的 `rbac` feature（`RbacEngine` / `RestrictedConnection`），在 SQL 层做资源级授权。它要求把用户/角色/权限映射到 SeaORM 约定的表结构，与 `sys_*` 的兼容性需要一次半天量级的 spike 验证。若映射成本低，可作为资源级的第二道防线叠加；否则保持主选方案。
- **不引入 Casbin**：它解决的是通用 ABAC，在本项目里会与自研模型形成两套权限语言，维护成本高于收益。

### 6.3 数据权限（本项目最关键的设计点）

参考实现用 AOP 往 SQL 里拼 `params.dataScope`。Rust 没有等价机制，而且那种做法有注入前科。cicada 的方案是**类型化的数据范围对象 + 强制传递**：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataScopeKind { All, Custom, Dept, DeptAndChild, SelfOnly, None }

/// 声明式、可序列化：既能放进 token，也能随跨服务调用传递（见 16.2）。
pub struct DataScope {
    pub kind: DataScopeKind,
    pub dept_ids: Vec<DeptId>,      // Custom / Dept / DeptAndChild 时生效
    pub subtree_paths: Vec<String>, // DeptAndChild：部门 ancestors 前缀
    pub user_id: UserId,            // SelfOnly 时生效
}

/// 狭接口：持久层只认识「谁、在哪个租户、可见范围多大」，不认识 HTTP。
pub struct Scope {
    pub tenant_id: TenantId,
    pub user_id: Option<UserId>,
    pub data_scope: DataScope,
}

/// 实体声明「我的数据归属哪个列」，作用域条件因此只实现一次。
pub trait DataScoped: EntityTrait {
    fn scope_dept_column() -> Option<Self::Column>;
    fn scope_owner_column() -> Option<Self::Column>;
}

/// repository 的负担只剩这一句：q = scope.apply::<Entity>(q)
impl Scope {
    pub fn apply<E: DataScoped>(&self, q: Select<E>) -> Select<E> { /* ... */ }
}
```

设计要点：

- 用 `DataScoped` trait 把「哪一列是部门、哪一列是归属人」的 schema 知识留在实体定义处，作用域逻辑只写一次。早先那种 `apply(q, dept_expr, owner_expr)` 的写法要求每个调用点自行拼表达式，接口偏浅、样板偏多。
- repository 接收窄接口 `Scope`，而不是整个 `RequestContext`。**把请求上下文一路贯穿到持久层是典型的浅接口**：它会强迫 DB 测试构造假请求，也让持久层绑死在 HTTP 上。`Scope` 在 service 边界派生，可序列化，跨服务可传输。

强制机制：

- `Scope` 由 `rbac` 中间件算好放进 `RequestContext`，service 调用 repository 时显式传入；**不提供无参的 repository 查询入口**，凡涉及部门或归属数据的方法签名必须带 `&Scope`，漏传即编译失败。
- 用「权限矩阵表驱动测试」覆盖 5 种业务数据范围（全部/自定义/本部门/本部门及以下/仅本人，另含内部 `None` 兜底）× 3 类实体，断言的是结果集而不是 SQL 字符串。
- **兜底防线**：PG `ROW LEVEL SECURITY`。为敏感表建策略，把当前用户/租户上下文通过 `SET LOCAL app.current_user_id = ...` 注入会话，数据库层再过滤一次；应用层算错时数据仍不外泄。该层默认关闭，按表开启。
- 全程 `sea_query` 参数绑定，禁止任何形式的 SQL 字符串拼接。

### 6.4 多租户

- 模式：**决策：单库单 schema + `tenant_id` 列（共享表）**。运维成本最低，PG 下配合 RLS 已足够；库/模式级隔离作为二期可选项，不入一期。
- `tenant_id` 不来自请求参数，只来自 `Principal`；缺少租户上下文的请求一律拒绝（登录、健康检查、公开接口除外）。
- 关联表（`sys_user_role` 等）不带 `tenant_id`，改为外键一致性约束，防止跨租户串联。例外是 `sys_user_identity`：它不是纯关联表，而是一张带唯一性约束的第一方数据表，因此带 `tenant_id`——租户内身份唯一性 `(tenant_id, identity_type, identifier)` 必须由本表约束（见 6.1）。
- 租户级覆盖：`sys_config`、字典、公告支持「全局默认 + 租户覆盖」两级取值。
- 租户限额（P2）：用户数、存储量、接口调用量，配套 `sys_tenant_package`。

---

## 7. 关键机制

### 7.1 统一响应与错误

**决策：标准 REST——HTTP 状态码表达结果语义，成功直接返回资源，错误用 RFC 9457 `application/problem+json`。**

- **单资源**：直接返回对象本身（`200`；创建返回 `201` + `Location`；删除返回 `204` 无包体），没有 `code`/`msg` 包装。
- **集合**：返回 `{ "items": [...], "total": 128, "page": 1, "page_size": 20 }`（`200`）。用带 `total` 的对象而不是裸数组 + `X-Total-Count`，是为了前端分页组件少写适配，也便于将来加游标分页。
- **错误**：`Content-Type: application/problem+json`，正文为 RFC 9457 标准字段 + 扩展字段：

```json
{
  "type": "https://cicada.dev/errors/rbac/2001",
  "title": "无权限",
  "status": 403,
  "detail": "缺少权限标识 system:user:add",
  "instance": "/api/v1/system/users",
  "code": "CICADA_RBAC_2001",
  "request_id": "b1f0c3...",
  "errors": [{ "field": "email", "message": "格式不正确" }]
}
```

  - `type` 每个业务码一个稳定 URI，便于检索与文档跳转；`title` 面向人；`code` 是机器可读业务码（全集见附录 C）；`errors` 只在字段级校验失败时出现。
  - 不再设 `X-Biz-Code` 响应头（业务码已在包体）；`X-Request-Id` 保留。

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("未认证")]              Unauthorized,          // 401
    #[error("无权限：{0}")]          Forbidden(Permission), // 403
    #[error("资源不存在：{0}")]      NotFound(String),      // 404
    #[error("参数校验失败：{0}")]    Validation(String),    // 422
    #[error("账号已临时锁定")]        Locked,               // 423
    #[error("请求过于频繁")]         RateLimited,           // 429
    #[error("数据冲突：{0}")]        Conflict(String),      // 409
    #[error("乐观锁冲突")]           OptimisticLock,        // 409
    #[error("上游服务不可用：{0}")]   Upstream(String),      // 502
    #[error(transparent)]            Db(#[from] DbError),               // 503
    #[error(transparent)]            Internal(#[from] anyhow::Error),    // 500
}

impl IntoResponse for AppError { /* → (StatusCode, ProblemJson)，并记结构化日志 */ }
```

`anyhow` 只允许出现在向 `AppError::Internal` 收敛的地方，且必须带 `request_id`。对外只给用户可读的 `detail`，内部细节（SQL、堆栈、路径）只进日志与 trace。

### 7.2 审计日志

- **表属主（与参考实现社区标准一致）**：`sys_oper_log` 与 `sys_logininfor` 由 `system` 域拥有（既有系统里这两张表由 system 域拥有，只有控制器挂在监控包）；`audit` 是**生产者**（审计宏 + outbox），`monitor` 只提供**只读管理视图**（列表/详情/清空按权限）。三者都不得越过这个分工直接写别人的表。
- `#[audit(module, action)]` 宏标注写接口，自动记录：操作人、租户、IP、UA、脱敏后的请求参数、耗时、结果、业务码；登录成功/失败记入 `sys_logininfor`。
- 字段级变更留痕（P1）：更新前后各取一次序列化快照，只记录变化字段，存进 `sys_audit_log` 的 `jsonb` 列。
- 表按月分区，保留期可配（默认 180 天），到期归档到对象存储而不是直接删除。
- 审计写入不阻塞主流程：写失败降级为 `error` 日志 + 指标告警，不影响业务响应。这是对参考实现「日志写入属于业务事务」的有意改进，需在 ADR 中记录取舍（ADR 0008）。
- 落库路径经 `AuditSink` 端口决定：`audit` 只写自己的 `outbox_event`，relay 再投递给 `system` 的落库端口（单体直连、微服务走队列）。**`audit` 不直接写 `sys_oper_log` / `sys_logininfor`**，否则会破坏 16.2 的数据所有权规则。

### 7.3 缓存

- 两级：进程内 moka（短 TTL，防热点击穿）+ Redis（跨实例一致）。
- 缓存对象：权限集合、菜单树、字典、参数、租户信息。
- 键规范：`cicada:{tenant}:{domain}:{key}:v{version}`，`version` 是领域版本号，变更即整体失效。
- 防击穿用单飞（single-flight）加载，防雪崩给 TTL 加随机抖动，防穿透对空结果做短 TTL 缓存。
- 分布式锁（`SET NX PX` + 唯一 token 校验释放）用于定时任务选主、并发导入、租户初始化。
- **可观测与可运维**：Redis 键空间按**域**登记（`cicada-monitor::cache::CACHE_DOMAINS`：权限集合、版本号、会话、会话索引、锁定、任务锁、参数、
  第三方登录 `state`、一次性下载凭证、字典项、菜单树、租户信息），缓存监控只能看/清这些域。观测接口只暴露「键 + 类型 + TTL」，**不提供按任意键取值**；失效接口只提供「整域清空」，正好对上「版本号失效」模型。白名单之外一律 422，失败关闭。


### 7.4 文件存储

- 统一 `Storage` trait，默认 opendal 实现，驱动按配置切换（`storage.driver`）：本地文件系统（开发默认）、**RustFS**（S3 兼容，自建默认）、**阿里云 OSS**、S3 及其他兼容对象存储。业务代码只依赖 `Storage` trait，换驱动不改业务。
- 上传：校验大小 + MIME（读 magic number，不信扩展名）→ 生成不可猜测路径（`tenant/uuid`）→ 落 `sys_file` → 返回引用 ID。
- 下载：先鉴权，再签发短时 URL（本地存储走一次性 token，对象存储走预签名 URL）。禁止暴露物理路径，封死参考实现历史上的任意文件下载漏洞。
- 图片缩略图交给对象存储或独立服务，基座不引入 ffmpeg 这类重依赖。

### 7.5 导入导出

- 导出：`rust_xlsxwriter` 流式写；大表走 `sys_export_task` 异步任务，完成后经通知中心给下载链接。
- 导入：`calamine` 解析 + 逐行 `validator` 校验 + 事务批量写；返回结构化错误报告（行号 + 字段 + 原因），全量失败或部分成功由参数控制。
- 安全：导入内容统一做 XSS 与公式注入清洗（`=`、`+`、`-`、`@` 前缀）。参考实现曾有导入 XSS 漏洞，这里在基座层封死。

### 7.6 定时任务

- 定义存 `sys_job`：cron 表达式、调用目标、并发策略（禁止并发 / 允许并发 / 队列）、超时、失败重试次数；每次执行落 `sys_job_log`：开始/结束时间、耗时、结果、异常摘要、trace_id。
- **触发**与**执行**是两条正交的轴，分开配置，不是「二选一的后端」：
  - 触发侧（`jobs.trigger`）：`cron`（`tokio-cron-scheduler`，进程内到点产出）或 `manual`（只接受人工 / 外部触发）。
  - 执行侧（`jobs.execution`）：`inline`（触发即就地执行，默认）或 `apalis`（投递到 PostgreSQL 队列，由消费者执行）。
- 集群语义同样按轴拆分：**只有触发侧需要选主**（Redis 分布式锁，绑定 `instance_id`），避免多实例重复产出；**执行侧每个实例都启用**——队列模式下即多消费者。
- 队列执行侧用 `apalis` + `apalis-sql` 的 `PostgresStorage`：任务落 `apalis.jobs`，`FOR UPDATE SKIP LOCKED` 取活 + `NOTIFY` 唤醒，重启不丢、可多消费者竞争。队列 schema 与表由 apalis-sql 自带迁移在启动时创建（`apalis` schema + `apalis.jobs` / `apalis.workers`），因此运行账号需要该库的建 schema 权限与 `pgcrypto` 扩展。**基座不使用 `apalis` 的 `MemoryStorage`**——进程内队列重启即丢，不能作为生产默认实现。
- 两侧共享同一个执行器（并发策略 / 超时 / 重试 / 执行日志只有一份实现，落在 `cicada-worker::JobExecutor`）：触发侧只决定「什么时候跑」，执行侧只决定「在哪跑」。对应端口是 `cicada-ports::TriggerSource` 与 `cicada-ports::RunSink`，由 `JobRuntime` 组合后交给 HTTP 面。
- 调用目标是**白名单枚举或注册表 key**，不是字符串反射。这是对参考实现定时任务反序列化 RCE 漏洞的根治方案：Rust 没有反射，字符串只能映射到显式注册的 handler。

### 7.7 代码生成器

流程：

```
选表 → 读库表结构（information_schema + pg_catalog）→ 推断类型/命名/控件/查询方式
     → 写 gen_table / gen_table_column 元数据
     → 可在线改（字段标签、控件类型、查询方式、字典绑定）
     → minijinja 渲染模板 → 打包 zip 下载
```

**本轮（M3）交付范围**：单表 `crud` 全链路 + 树表 `tree` 模板 + 打包下载 +
模板可覆写 + 生成物常态编译校验。主子表 `sub`、在线写源码树、导出按钮模板明确
递延（见下方「递延」）。

产出物（一次生成一套；✔ = 本轮已交付）：

| 产物 | 内容 | 状态 |
| --- | --- | --- |
| 迁移 | `sea-orm-migration` 结构文件（`if_not_exists`，供在别处重建该表） | ✔ |
| 实体 | SeaORM `DeriveEntityModel` + `#[derive(DataScoped)]`（租户 / 软删 / 归属一次声明） | ✔ |
| DTO | 列表查询 / 行视图 / 分页 / 新增修改入参 + utoipa schema | ✔ |
| Handler | axum 路由 + `#[require_perm]` + `#[audit]` + `#[utoipa::path]`，查询走 `ScopedQuery` | ✔ |
| 模块声明 | `impl Module`（`owns` / `perms` / `routes`），装配期双向对账 | ✔ |
| 权限数据 | `sys_menu` 的菜单 + 按钮权限点 SQL upsert（幂等） | ✔ |
| 前端 | Vue3 页面 + TS API 文件（风格与手写一致） | ✔ |
| 说明 | `README.md`：依赖清单、注册步骤、约定 | ✔ |
| Repository / Service | 独立分层文件 | 递延：单表的查询直接写在 handler 里，走 `ScopedQuery`，不生成空壳 Repository |
| 测试骨架 | 权限矩阵 / 集成测试骨架 | 递延 |
| 树表 | `tpl_category = tree`：DTO 带 `children`、handler 出森林接口、`el-tree-select` 选上级 | ✔（形状见下） |
| 主子表 | `tpl_category = sub`：事务写入主 + 从 | 递延：元数据可存，渲染阶段明确报错，不产出半成品 |

设计约束（本轮已落实）：

- 生成的代码必须**开箱可编译**，不允许 `todo!()` / `unimplemented!()`。
  由工作区成员 `crates/codegen/fixture` 常态保证：它用 `#[path]` include
  `crates/codegen/fixtures/` 下**由生成器产出**的模块，因此
  `cargo check --workspace` 每次都会重新编译生成产物；模板一旦产出无法编译的代码，
  构建立即失败。另有快照测试（模板 + 样例表 → 与已提交产物逐字节比对）拦截模板漂移，
  重新生成用 `UPDATE_FIXTURE=1 cargo test -p cicada-codegen --test fixture`。
- 模板与生成逻辑分离，模板可被使用者覆写：`gen.template_dir` 下的
  `<模板名>.j2`（`entity.j2` / `handler.j2` / …）优先于内置模板。
- 生成逻辑只做**确定性**转换（`sys_user_role` → `SysUserRole` / `sys_user_role`），
  同一份元数据永远渲染出同样的字节，便于 diff 与快照。

**业务表契约**（违反即明确报错，不猜、不降级）：

1. 有且只有一个**单列主键**（复合主键暂不支持）；
2. 有 `tenant_id`（受控查询的租户维度）；
3. 有 `deleted_at`（软删；`ScopedQuery` 会叠加 `IS NULL`）。

归属形态由列自动判定：`dept_id + (owner_id | create_by)` → `DeptAndOwner`，
只有其一是 `DeptOnly` / `OwnerOnly`，都没有是 `NoOwnership`。

**树表契约**（`tpl_category = tree`，在第 1–3 条之上再加一条，违反即报错）：

4. 有**非空** `parent_id`，且其类型与主键一致（根节点的父键取 `0`；字符串主键取空串）。
   可空父键被拒——树索引会拿到 `None` 而无法归类。

树表与单表**共用同一套模板**（`entity` / `module` / `migration` / `menu_sql` 完全一致），
差异集中在三处，且都由生成逻辑算好、模板只做插值：

- **DTO**：`Vo` 追加 `children: Vec<Vo>`（`skip_serializing_if = "Vec::is_empty"`），
  分页/详情接口里恒为空，序列化时省略。
- **Handler**：追加 `GET {route}/{tree}`，一次取回受数据范围约束的全部行，在
  `assemble_tree` 里用主键索引组装成森林。**树表不分页**（分页会把子树切碎）。
  组装对脏数据是防御性的：父键指向集合外或自引用即视为根，环引用不会 panic
  （已消费节点再次被引用时跳过），任何环数据下接口都不挂。
- **前端**：树表页（`index-tree.vue.j2`）用 `row-key` + `tree-props` 渲染层级，
  表单里上级节点用 `el-tree-select`（挂一个虚拟「顶级节点」根），并提供展开/折叠。
  父键列仍进 `Vo`（前端组装/选择需要），但**不进查询项**，也不作为表格普通列渲染；
  显示名列放在第一列，展开箭头才落在节点名上。

树表的 `tree_code` / `tree_parent_code` / `tree_name` 不单独落库：节点键取主键、
父键取约定列 `parent_id`、显示名取第一个字符串业务列（优先 `*_name`），
由 `apply_tree_contract` 在「导入时」与「改完再预览时」各推导一次，保证一致。

**安全边界**（参考实现生成器的两个真实风险，这里从根上收紧）：

**实现状态（2026-10-15）**：模板工程跑通时发现并修掉两处产物缺陷——生成器原先用
`class_name`（去前缀后的类型名）当表标识，于是 `biz_goods` 会生成出建 `goods` 表、
列 `TypeCol` 建出 `type_col` 列（真实列是 `type`），与实体里 `#[sea_orm(table_name = "biz_goods")]`
自相矛盾。改为 `#[sea_orm(iden = "{{ table_name }}")]` / 列名不一致时补
`#[sea_orm(iden = "…")]` 与 `#[sea_orm(column_name = "…")]`，由模板快照与
`template/` 端到端（`cicada new` → `migrate` → CRUD 全绿）双向守住。

- **数据面**：只读当前 schema 的 `information_schema` / `pg_catalog`；表名一律以
  绑定参数传入，并先过标识符校验（`^[A-Za-z_][A-Za-z0-9_]{0,62}$`）、
  框架表黑名单（`sys_*` / `gen_table*` / 迁移表…）与可选前缀白名单。
- **代码面**：产物路径由模板常量决定，**不拼接用户输入**；`module_name` /
  `business_name` / `class_name` 在渲染前与落库前各校验一次，杜绝 `../` 路径穿越。
  `gen_type = 1`（在线写源码树）**明确拒绝**——管理接口不应具备任意路径写能力，
  生成物一律以 zip 交付。
- **时间类列**在 DTO 里用字符串（RFC3339 / `YYYY-MM-DD` / `HH:MM:SS`），
  由生成的 `parse_*` 辅助解析，避免给使用者的工程强加 `chrono` 的 serde 特性。

权限点与接口（完整路径表见 §10.3，权限点仍是附录 D 的 5 个，不新增命名空间）：

| 动作 | 权限点 |
| --- | --- |
| 列表 / 详情 / 候选表 | `tool:gen:list` |
| 导入（写元数据）/ 在线改 | `tool:gen:list` / `tool:gen:edit` |
| 预览 | `tool:gen:preview` |
| 打包下载（单表 / 批量） | `tool:gen:code` |
| 删除 | `tool:gen:remove` |

**递延**（下一轮）：主子表 `sub` 模板（`sub_table_name` / `sub_table_fk_name` 落库后
事务写入主 + 从）与对应快照、导出按钮（需接 `export::build` 的分批写盘 + xlsx）、
Repository/Service 分层骨架、写源码树（若做，必须以白名单根目录 + 路径规范化实现）。

这是全项目工程量最大的模块，排在 M3 而非 M1。Rust 没有 Spring 那样的运行期装配，
生成器要生成的是一整套显式代码，成本高于参考实现的 MyBatis 生成。

### 7.8 可观测性

- 日志：tracing 结构化输出，字段包含 `request_id` / `trace_id` / `tenant_id` / `user_id` / `path` / `latency_ms` / `biz_code`。
- 指标：`/metrics` 暴露 QPS、延迟分位、DB 连接池、缓存命中率、任务执行结果、错误率（按业务码分组）。
- 链路：OpenTelemetry OTLP 导出，DB 查询作为 span 子节点。
- 健康检查：`/healthz`（存活，不查依赖）与 `/readyz`（就绪，查 PG 与 Redis；不可用时按 7.1 返回 problem+json，用附录 C 的 `9002` / `9003` 区分是哪个依赖）。
- 服务信息（对应参考实现的服务监控页）：CPU、内存、磁盘、进程启动时间、版本、构建 commit，通过指标与专用接口暴露，不做内嵌 HTML 控制台。
- 缓存监控（对应参考实现的缓存监控页）：`GET /api/v1/monitor/cache/{overview,domains,keys,key}` 观测、`DELETE /api/v1/monitor/cache/keys` 按域失效，**无内嵌页面**。参考实现用 `monitor:cache:view` 一个权限点同时管读与 `clearAll`，这里拆成 `monitor:cache:list`（观测）与 `monitor:cache:clear`（清理，带审计），并把清理收敛到白名单域内。


### 7.9 配置

优先级：环境变量 > 本地 `config/local.yaml`（gitignore）> `config/{profile}.yaml` > `config/default.yaml` > 代码默认值。

```yaml
server: { port: 8080, body_limit_mb: 20, graceful_shutdown_secs: 30 }
db:
  url: "postgres://cicada:${DB_PASSWORD}@localhost:5432/cicada"
  max_connections: 0        # §5.1：0 = CPU 核数 × 2；显式数字则按数字
  acquire_timeout_secs: 8
  slow_query_ms: 200        # §5.1：超阈值以 warn 记 SQL 摘要与耗时（不含参数值）；0 关闭
redis: { url: "redis://localhost:6379/0" }
auth:
  access_ttl_min: 15
  refresh_ttl_days: 7
  max_sessions: -1
  password: { max_retry: 5, lock_minutes: 10, min_length: 8 }
  jwt: { alg: EdDSA, private_key: "${JWT_PRIVATE_KEY}", keys: [] }
  # 第三方登录（§6.1）：providers 为空即关闭（接口 404）；kind: wechat | oidc
  oauth: { state_ttl_secs: 300, ticket_ttl_secs: 600, providers: [] }
storage: { driver: local, root: "./data/upload" }   # driver: local | rustfs | oss | s3
# 对象存储三档（rustfs | oss | s3）补齐这四行；凭证只从环境注入（§7.9）：
# storage: { driver: rustfs, bucket: cicada, endpoint: "http://rustfs:9000", region: auto,
#            access_key_id: "${STORAGE_KEY_ID}", access_key_secret: "${STORAGE_SECRET}" }
tenant: { enabled: true, default_id: 1 }
# 跨域（§3.4 第 4 层）：默认关闭；allowed_origins 写 "*" 时不能开 allow_credentials。
cors: { enabled: false, allowed_origins: [], allow_credentials: false, max_age_secs: 3600 }
# 限流（§3.4 第 7 层）：单 IP 令牌桶 + 租户套餐 QPS（api_qps_limit）。
# per_ip_qps: 0 表示不限；租户额度读库失败按不限放行（可用性优先）。
rate_limit: { enabled: true, per_ip_qps: 200, per_ip_burst: 400, tenant_cache_ttl_secs: 60 }
# 审计发件箱 relay（§7.2）：audit 写 outbox_event，relay 投给 system 的落库处理器。
outbox: { enabled: true, poll_interval_ms: 500, batch: 200, max_attempts: 5 }
# 日志分区生命周期（§7.2 / §12）：整月都过了保留期才归档删除；archive: false 只报告不动数据。
audit: { retention_days: 180, horizon_months: 24, archive: true }
# 可观测（§7.8）：stdout 日志恒开；otlp_endpoint 为空就不导出链路。
telemetry: { otlp_endpoint: null, service_name: cicada, sample_ratio: 1.0 }
```

- 敏感值一律来自环境变量或密钥管理，配置文件里只写占位符。
- 启动时用 serde 做一次完整校验（缺项、越界、URL 非法直接 fail fast），错误信息给出具体字段路径。
- 配置结构实现 `Debug` 时对密钥字段做掩码（`Secret<T>` 新类型），避免日志泄露。

### 7.10 其它横切能力

- **防重复提交**：`Idempotency-Key` 头 + Redis `SET NX` 短 TTL，命中则返回首个请求结果；对写接口按需开启。
- **数据脱敏**：`#[mask(Mobile)]` / `#[mask(IdCard)]` 字段级宏，序列化时按角色决定明文或掩码。
- **国际化**：`rust-i18n`，消息 key 与参考实现的 `messages.properties` 保持同名映射，便于迁移；前端 vue-i18n 共用同一份翻译源。
- **导出/接口版本**：API 前缀 `/api/v1`，破坏性变更走 `/api/v2`；OpenAPI 快照进 CI，接口变更需在 PR 中显式确认。

---

## 8. 接口规范

| 约定 | 内容 |
| --- | --- |
| 前缀 | `/api/v1`（业务）、`/auth`（认证）、`/healthz`、`/metrics`、`/docs`（OpenAPI UI，可按环境关闭） |
| 认证 | `Authorization: Bearer <access_token>` |
| 追踪 | 请求可带 `X-Request-Id`，响应必带 |
| 幂等 | 写接口可选 `Idempotency-Key` |
| 响应 | 成功直接返回资源：单资源为对象，创建 `201` + `Location`，删除 `204`；集合为 `{items,total,page,page_size}` |
| 分页 | 请求 `page` + `page_size` + 排序白名单 `sort_by` / `sort_order`；响应形状见上行。白名单按接口声明（`crates/web/src/sort.rs` 的 `SortSpec`），名单外的列名忽略而不是报错 |
| 时间 | 一律 ISO-8601 UTC（`2026-09-30T08:00:00Z`） |
| 错误 | `application/problem+json`（RFC 9457）：`type/title/status/detail/instance` + 扩展 `code` / `errors` |
| 弃用 | 响应头 `Sunset` + `Deprecation` |

以用户模块为例的接口与权限点对应：

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/system/users` | `system:user:list` |
| GET | `/api/v1/system/users/{id}` | `system:user:list` |
| POST | `/api/v1/system/users` | `system:user:add` |
| PUT | `/api/v1/system/users/{id}` | `system:user:edit` |
| DELETE | `/api/v1/system/users/{id}` | `system:user:remove` |
| PUT | `/api/v1/system/users/{id}/password` | `system:user:resetPwd` |
| PUT | `/api/v1/system/users/{id}/status` | `system:user:edit` |
| GET | `/api/v1/system/users/export` | `system:user:export` |
| POST | `/api/v1/system/users/import` | `system:user:import` |

权限标识与参考实现现有数据完全一致（全集见附录 D，其余模块的接口对照见附录 E），因此 `sys_menu` 的 85 条记录可以整体迁移。注意参考实现的 `:view` 是页面权限、`:list` 是列表权限，cicada 纯 API 下 `:view` 只用做菜单可见性，读取接口统一用 `:list`。

---

## 9. 安全设计

### 9.1 认证与会话

- 密码 Argon2id（`m=19456 KiB, t=2, p=1` 起步），哈希串自带参数，登录时按需升级。
- 密码规则：**至少包含大写字母、小写字母、数字各一（允许特殊字符），长度 ≥ 8**；注册与改密处统一用 validator 拦截。对应参数 `sys.account.chrtype = 3`（参数占位；cicada 实际校验更严格：大小写各一 + 数字）。
- access_token 短 TTL（15 分钟），refresh_token 一次一换并检测重放（旧 token 复用即判定泄露，撤销整条会话链）。
- 登录失败计数 + 指数退避锁定；验证码在连续失败后强制出现。
- 会话绑定 `(ip, user_agent)` 指纹，异常变化触发重新认证而不是直接拒绝（移动网络下 IP 会变，不能做硬绑定）。

### 9.2 传输与输入

- 强制 HTTPS（HSTS），生产禁用明文；CSP、`X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY` 由中间件统一注入。
- CORS 白名单来自配置，禁止 `*` 配合凭证。
- 请求体大小限制、JSON 深度限制、数组长度限制（防炸弹对象）。
- 所有入参经 `validator` 校验；路径与查询参数用强类型提取器，非法直接 400/422。

### 9.3 注入与 XSS

- SQL：全程 `sea_query` / SeaORM 绑定参数，**代码里禁止出现 `format!` 拼 SQL**，CI 用自定义 lint 规则 grep 拦截（`Statement::from_string` 与 `raw_sql` 仅允许在 `cicada-infra` 的 db 模块白名单文件里出现）。
- XSS：输出侧编码（前端框架默认）+ 富文本内容存储前用白名单清洗（`ammonia` 级别）；导入导出做公式注入清洗。
- CSRF：纯 Token 认证 + 不依赖 Cookie 的 API 天然免疫；若启用 Cookie 会话则强制 `SameSite=Lax` + 双提交令牌。

### 9.4 权限与越权

- 每个写接口用 `#[require_perm("...")]` 显式声明权限点；`cicada-web` 在装配期做**双向对账**——路由声明的权限点必须出现在某个模块的 `Manifest.perms` 里，`Manifest.perms` 里的权限点也必须被某个路由使用，任一方向有缺口即启动失败（见 16.1）。
- 水平越权防护：所有按 ID 取数的 repository 方法强制带租户条件，并在返回前做归属校验。
- 数据范围兜底：敏感表开启 PG RLS（见 6.3）。

### 9.5 密钥、依赖与审计

- JWT 私钥、数据库口令、SMTP 口令全部走环境变量或密钥管理；启动时校验长度与格式，拒绝弱密钥（如默认示例值）。
- 依赖安全：`cargo-deny` 检查漏洞与许可证，`cargo-audit` 进 CI；供应链用 `Cargo.lock` 全固定 + 私有 registry 镜像（可选）。
- 审计：登录、权限变更、数据导出、配置修改四类高危操作强制记录且不可关闭。

---

## 10. 前端与客户端方案

**决策：前后端分离，后端只出 REST API；第一版只交付一套 Vue 3 + TypeScript 前端，React / Svelte / uni-app / Tauri 作为后续版本增量提供。**

**v1（本期交付）**：Vue 3 + TypeScript + Vite + Element Plus + Pinia + vue-i18n，**全新重写**，不复用同类 Vue3 前端 代码。选这套是因为后台 CRUD 密度高、Element Plus 的表格/表单/树组件够用，且生态成熟、招人容易。

**v1 设计语言（本期约定）**：界面按 **Apple 设计风格**实现，并沉淀为可复用资产——设计令牌（颜色/间距/圆角/阴影/字距/时长全部来自 token，禁止散落魔法值）、系统字体 + 光学字号/字距/行高、半透明材料（`backdrop-filter`）表达层级、弹性动画（默认临界阻尼，仅动量手势给轻微过冲）、以及 `prefers-reduced-motion / transparency / contrast` 三态无障碍。Element Plus 只作底层组件、不沿用其默认 Material 观感，用主题令牌重皮肤；表格/表单/弹层/抽屉等高频组件封装成项目自有组件以复用。这条不阻塞表设计，属于 M1 前端骨架与 M3 生成器的实现约束。

**后续版本（本期只做架构预留，不交付）**：

| 端 | 技术 | 定位 |
| --- | --- | --- |
| Web | React 19 + TS、Svelte + TS | 与 v1 并列的独立前端，复用同一 REST API 与权限标识 |
| 小程序 | uni-app（Vue 语法） | 独立工程，走小程序内部登录（6.1） |
| 桌面/移动 | Tauri + TS + Rust | **Rust 只当壳**（窗口、系统能力、自动更新），业务全走后端 REST |

**为多前端预留的架构约束**（现在就要守住，否则后面补不上）：

- 后端不假设任何前端框架：只提供 REST + 权限标识 + 菜单树，响应体、分页、错误码对所有前端一致。
- 权限判断各端自实现：v1 用 `v-hasPermi` / `v-hasRole`，React / Svelte / uni-app / Tauri 各自实现等价物；后端只下发权限集合。
- 代码生成器 v1 只产出 Vue 前端；其余框架的模板按需增量补齐（模板与生成逻辑分离，允许项目覆写，见 7.7）。
- 前端工程放 `web/`（v1）；后续框架各自成目录或独立仓库，与 Rust 工作区互不干扰，CI 分别构建。

v1 前端约定：

- 路由由后端菜单树动态生成，`meta.perms` 与后端权限标识一一对应。
- API 层按模块生成：代码生成器同时产出 Vue 页面与 API 调用文件，风格与手写保持一致。
- 构建产物交给 Nginx/CDN 静态托管，不由后端内嵌（生产建议前置 Nginx，负责 TLS、gzip/brotli 与缓存头）。

**实现状态（2026-10 骨架已落地，代码在 `web/`）**：

- 技术栈与目录：`src/{styles,api,stores,router,components,composables,directives,locales,utils,views,layout}`，
  详见 `web/README.md`。
- 设计令牌：`src/styles/tokens.css` 是唯一取值处（颜色/间距/圆角/阴影/字距/时长 + 三态无障碍），
  Element Plus 通过把 `--el-*` 映射到令牌换肤，未 fork 组件样式。
- 动态路由：`GET /auth/routers` → `router/dynamic.ts`（扁平路由 + 嵌套侧栏），视图按
  `component` 或 `path` 推导；菜单已配而界面未做的落 `views/placeholder`（不 404）。
- 权限指令：`v-perm` / `v-hasPermi` / `v-hasRole`（含 `.disable` 修饰符），仅作界面裁剪。
- 容器化组件：`App{Page,SearchForm,Table,Dialog,Drawer,TreeSelect,Form,DictSelect,DictTag,StatusTag,Empty,Icon,ErrorBoundary}`，
  生成器产物用的裸 `el-*` + `.app-container` 由同一套令牌保证观感一致。
- 失败兜底：接口失败归一成 `ApiError` 后由拦截器提示一次；渲染/生命周期错误由
  `AppErrorBoundary` 就地接住（页面级 + 根组件两级），侧栏与顶栏保持可用。
- 会话终止单入口：`clearSession('expired')` 广播 → `main.ts::handleSessionLost` 统一提示并
  回登录页（带 `?redirect=`）；用户主动登出不广播、不提示。守卫重放当前地址时保留
  `query`/`hash`（`{ path: to.fullPath }` 会被 vue-router 吞掉查询串）。
- 前端门禁：`vue-tsc --noEmit`、`eslint .`（含 `vue/no-undef-components`：漏引组件会让
  Vue 把标签当原生元素渲染，`<el-table-column>` 找不到表格即抛错打空应用）、`vite build`。
- 已实现界面：登录、概览、系统管理（用户/角色/菜单/部门/岗位/字典/参数/公告）、
  监控（在线用户/定时任务/操作日志/登录日志/缓存/服务监控/数据监控）、通知（渠道/模板/记录）、
  平台管理（租户/租户套餐/开放接口凭证）、代码生成、403/404/占位。
  未做：表单构建（1.3 的明确非目标：不做在线拖拽表单构建器）；数据监控已提供只读的库/表统计，
  不是 Druid 式内嵌数据库控制台；另有富文本/编辑器未做。
- 服务监控页只消费 7.8 的 `GET /api/v1/monitor/server` 单一采集接口，不在前端二次采样；
  自动刷新默认关闭，开启后每 5 秒重新采集一次。
- 平台管理三页对应的后端读路径均为租户隔离；API 凭证的明文密钥只在签发响应里出现一次，
  因此签发弹层用「一次性展示 + 复制」而不是可回看的详情。
- 因前端数据源需要而在后端补的两条登录可见读路径（含集成测试）：
  `GET /api/v1/system/roles/{id}/menus`、`GET /api/v1/system/dict/data/by-type/{type}`。

---

## 11. 工程规范与测试策略

### 11.1 工程规范

- edition 2024，`rust-toolchain.toml` 固定版本，CI 与本地一致。
- workspace 统一依赖版本（`[workspace.dependencies]`），子 crate 只声明 feature。
- Lint：`clippy -D warnings`、`rustfmt`、`cargo deny`、`cargo udeps`（可选）。
- 每个 crate 有 README 说明「职责 / 公开 API / 依赖 / 不做什么」。
- 公开 API 加文档注释，CI 用 `cargo doc` 检查链接；不稳定 API 用 `#[doc(hidden)]` 或 feature gate 隔离。
- 提交信息与 ADR：架构级决策写 `docs/adr/NNNN-*.md`（记录背景、选项、结论、代价）。

### 11.2 测试分层

| 层次 | 工具 | 覆盖内容 | 目标 |
| --- | --- | --- | --- |
| 单元测试 | 内置 `#[test]` / `tokio::test` | 值对象、权限计算、数据范围条件、脱敏、校验规则 | 快，毫秒级，无 IO |
| 组件测试 | `testkit` + 真实 PG（测试容器） | repository、事务回滚、软删、乐观锁、唯一约束 | 秒级 |
| 接口测试 | axum + `tower::ServiceExt::oneshot` | 路由、提取器、权限中间件、统一响应、错误映射 | 不启端口 |
| 权限矩阵测试 | 表驱动（scope × 实体 × 角色） | 5 种数据范围 × 多角色的可见性断言 | 覆盖全分支 |
| 迁移测试 | 空库 up/down 往返 | 迁移可回滚、种子幂等 | CI 必过 |
| 生成器测试 | 逐字节快照 + fixture crate（`cargo check` 常态编译） | 单表与树表生成代码均可编译；主子表递延 | M3 出口条件（单表 + 树表已达成） |
| 端到端 | 前端 Playwright（可选） | 登录 → 授权 → CRUD → 导出 | 发布前冒烟 |

约定：

- 测试连**真实 PostgreSQL**，不用 mock ORM——SQL 语义必须被真实验证。落地方式：CI 用 service
  容器起 PG 17 / Redis 7，本地用 `docker-compose.yaml`，连接串统一走 `DATABASE_URL`；
  没有引入 `testcontainers`——它满足的是同一条本意（真实库），而 service 容器已经在 CI 里
  提供了同等保证，代价更低（`crates/testkit/README.md` 记录了这条边界）。
- 每个测试用例独立事务或独立 schema，禁止测试间共享可变状态。
- 时间与 ID 通过 `Clock` / `IdGenerator` trait 注入，测试用固定实现（可复现的雪花/递增 ID 与冻结时间）。
- 关键并发行为有专门测试：会话踢出、乐观锁冲突、分布式锁选主、并发导入。

### 11.3 CI 流水线

```
fmt → clippy(-D warnings) → 单元测试 → cargo deny/audit
   → 迁移往返测试 → 集成测试(真实 PG) → 生成器快照+编译测试
   → 前端 lint + typecheck + build → 镜像构建 → 冒烟
```

编译加速：`sccache` + `cargo-nextest` + 按 crate 缓存；必要时用 `cargo-hakari` 合并依赖 feature 减少重复编译。

---

## 12. 部署与运维

**发行版本：两个——大单体版与微服务版，共享同一套 crate，差异只在组合根与传输 adapter（16.2、16.4）。**

- **产物**：单个二进制（`apps/server`、`apps/cli` 各自一个）。Linux x86_64/aarch64，musl 静态链接优先，glibc 版本兜底。
- **开发环境**：`docker compose` 一键起 PostgreSQL（取 Docker 可用的最新版）+ Redis（单实例）+ RustFS + server；**Docker 只用于开发**，不作为生产部署方式。PostgreSQL 需 `max_locks_per_transaction=256`：测试台是「一个用例一个 schema、跑前 `DROP SCHEMA … CASCADE` 重建」（11.2），CASCADE 按对象取共享锁，默认 64 在并行跑分区表用例时会撞 `53200 out of shared memory`。
- **生产部署**：K8s 暂不纳入。二进制 + systemd 直接部署，前置 Nginx（TLS、静态资源、gzip/brotli）。
- **数据库**：PostgreSQL 最新稳定版；连接池上限、RLS 会话变量、分区表的自动建分区任务、`pg_stat_statements` 慢查询分析。
- **Redis**：单体版单实例；微服务版用集群。
- **微服务版**：服务间通信走 gRPC（tonic，见 3.1）；服务注册与发现用 **etcd**（gRPC 有标准 etcd resolver，客户端生态成熟；若更看重与既有微服务体系的 Nacos 运维习惯一致，可换 rnacos，切换点是组合根一处）。MQ（RabbitMQ）按需引入，不预置。
- **对象存储**：经 opendal 对接，驱动可配——RustFS（S3 兼容，自建默认）、阿里云 OSS、S3 等；本地开发用文件系统驱动。
- **无状态**：应用实例不保存状态（会话在 Redis、文件在对象存储），可水平扩展；定时任务通过分布式锁选主。
- **备份**：PG 逻辑备份 + WAL 归档；对象存储版本化；备份恢复演练纳入发布流程。
- **发布**：迁移与代码解耦（先迁移后发版，遵循 expand/contract）；支持金丝雀与快速回滚（二进制回滚不依赖迁移回滚）。
- **可观测接入**：Prometheus 抓 `/metrics`，OTLP 推 trace，日志走 stdout 由采集器统一收集。

---

## 13. 里程碑

| 阶段 | 交付物 | 出口条件（可验证） |
| --- | --- | --- |
| **M0 骨架**（1–2 周） | workspace、kernel、infra（config/db/cache/telemetry/mq）、ports、web 中间件链、模块注册契约、cargo-generate 模板骨架、CLI（migrate/seed/init-admin） | `cargo run -p cicada-server` 起服务；`/healthz`、`/readyz`、`/metrics`、`/docs` 可用；迁移往返测试通过 |
| **M1 认证与权限**（2–3 周） | auth（登录/刷新/登出/改密/TOTP 可选）、rbac（权限点/菜单树/缓存）、用户与角色与菜单三个模块 | 能登录并拿到菜单树；无权限返回 403；权限矩阵单元测试全绿；会话踢出生效 |
| **M2 基础域与审计**（2–3 周） | 部门/岗位/字典/参数/公告、操作与登录日志、在线用户、导入导出、文件上传下载 | 80 个既有权限标识（附录 D）全部有对应接口 ✅（`perm_coverage` + `appendix_e`）；导出 10 万行内存稳定 ✅（`crates/web/tests/export_scale.rs`：10 万行 6.9 秒 / 2.2 MB 产物 / RSS 增量 3 MiB）|
| **M3 效率工具**（3–4 周） | 代码生成器（单表 crud ✅ / 树表 ✅ / 主子表递延）、定时任务 ✅、缓存监控 ✅、多数据源可选、数据脱敏 ✅、国际化 ✅ | 单表与树表生成代码 `cargo check` 通过并可运行 ✅；任务可在线增删改且集群不重复执行 ✅ |
| **M4 基座增强**（3–4 周） | 多租户配额 ✅、通知中心 ✅、开放 API（API Key）✅、RLS 兜底开关 ✅、限流层（§3.4 第 7 层）✅、K8s 部署清单 ⏸（与 §12「K8s 暂不纳入」冲突，待决策） | 两租户数据完全隔离（含 RLS 验证）✅；通知三通道可用 ✅（HTTP 网关，ADR 0002）；限流 429 生效 ✅（ADR 0005）；压测达标 ⏳ |
| **M5 生态**（持续） | 工作流适配层、OAuth2/SSO 登录 ✅（ADR 0007；作为授权服务器的 `sys_oauth_client`/`sys_oauth_token` 一期不做，见 §13 补记）、gRPC 导出、监控告警模板、贡献指南 | 按需推进，每项独立 ADR |

工期为单人全职估算；并行开发时 M1/M2 的领域模块可拆分给不同人，但 M0 必须先完成并被冻结。

> **M4 进展（2026-10-10）**：租户套餐与限额（`sys_tenant_package` + `TenantQuota` 闸门，ADR 0004）、
> 通知中心（`crates/notify`，渠道/模板/记录，ADR 0002）、开放 API（`sys_api_key` + `X-API-Key`，ADR 0003）、
> RLS 兜底（按表开启 + 白名单校验，ADR 0001，验证见 `crates/infra/tests/rls.rs`）、
> 限流层（`crates/infra/src/ratelimit.rs` 原语 + `crates/web/src/ratelimit.rs` 编排：单 IP 令牌桶
> 在最外层、租户套餐 `api_qps_limit` 在鉴权后，ADR 0005，验证见 `crates/web/tests/m4_features.rs`）已落地，
> 因此 `api_qps_limit` 从「只落库回显」变为**实际生效**。
> 同期补齐 §3.4 请求流水线的 1–6 层（request_id / trace / security_headers / cors / compression /
> body_limit，ADR 0006），其中 `server.body_limit_mb` 由此从「只读配置」变为真正生效。
> 未做：压测（需要压测环境与目标值）与 K8s 部署清单（§12 明确「生产部署 K8s 暂不纳入」，
> 与 M4 行冲突，需项目所有者裁决后写入 ADR）。

> **§7.1 补记（2026-10-16）**：错误响应从简化体 `{ "error": "…" }` 升级为 RFC 9457
> `application/problem+json`，附录 C 的业务码自此真正上线（`crates/web/src/problem.rs`）。
>
> - **形状**：`type`（本业务码的稳定 URI `https://cicada.dev/errors/<域>/<序号>`，序列化时改名）、
>   `title`（随业务码固定、**不翻译**）、`status`、`detail`（按 `Accept-Language` 本地化）、
>   `instance`（触发问题的路径）、`code`（`CICADA_<域>_<四位>`），以及只在字段级校验出现的
>   `errors: [{field,message}]`。成功响应不变（集合仍是 `{items,total,page,page_size}`）。
> - **状态码归属**：状态码由 `BizCode` 决定，调用点不再携带；只有状态码时用
>   `BizCode::from_status`（未登记的状态回落 `Internal`，所以流水线层要显式选码），
>   需要偏离附录默认状态时用 `Problem::with_status`（目前只有 TOTP 状态冲突保 409、
>   通知投递失败保 502 两处）。
> - **相对旧实现的状态迁移**（前端与集成测试按此对齐）：TOTP 码错误 401→**422**
>   （`CICADA_AUTH_1006`）、重复绑定身份 422→**409**（`CICADA_AUTH_1008`）、
>   缓存/会话存储不可用统一 **503** `CICADA_INTERNAL_9003`、删除有子节点 **409**
>   `CICADA_SYS_3004`、树形成环 **422** `CICADA_SYS_3005`、幂等请求处理中 **409**
>   `CICADA_SYS_3002`、字段校验 **422** `CICADA_VALIDATION_4001` + `errors[]`。
> - **覆盖面**：`auth` / `system/*` / 通知 / 文件 / 任务 / 缓存 / 日志 / 代码生成 / 幂等 /
>   限流 / 请求体上限 / 权限守卫全部改完，另有两处路由层兜底（`crates/web/src/router.rs`）：
>   未匹配路径由 `not_found` 回 **404** problem+json，提取器（`Json`/`Query`/`Path`）默认那套
>   `text/plain` 拒绝体由 `problem_rejection` 改写成 problem+json（422→`…_4001`、400→`…_4002`、
>   415→`…_4002` 且保留原状态码，serde 原文只进 `tracing::debug!`）；方法不匹配仍由 axum 回空体
>   405——附录 C 未给 405 分配业务码，需要时先补码再改。生成器模板与 fixture 同步换成
>   `ApiErr = Problem`。
> - **消息目录**：`crates/web/build.rs` 声明 `rerun-if-changed=locales`。rust-i18n 只监视它自己
>   crate 的 locales 目录，缺这一步时改 `locales/*.yml` 不触发重编、新增 key 会静默回退成
>   fallback 语言（踩过一次：`common.invalid_payload` 加了 key 但接口仍返回中文 `detail`）。
> - **验证**：`crates/web/tests/m2_extra.rs::error_bodies_are_rfc9457_problem_json`
>   在真实 HTTP 面上断言 content-type 与字段；OpenAPI 组件里注册 `Problem` / `FieldError`
>   （`ErrorBody` 已从 schema 中移除）。前端 `utils/api-error.ts` 只解析 problem+json，
>   `isValidation` 改按 `code`/`errors[]` 判定而不是按 422 状态猜。

> **§7.2 补记（2026-10-16）**：审计落库改成本节要求的 outbox 形态（取舍与代价见 ADR 0008）——此前 `audit` 的
> `DbAuditSink` 直接 `INSERT sys_oper_log` / `sys_logininfor`，正是本节明令禁止的越界写法。
>
> - **表**：`migrations/src/m20261016_000011_outbox.rs` 建 `outbox_event`（附录 F 的列：
>   `id` / `tenant_id` / `aggregate` / `aggregate_id` / `event_type` / `payload jsonb` /
>   `created_at` / `published_at` / `attempts`）+ `(published_at, id)` 索引供 relay 顺序扫描。
> - **生产者**：`crates/audit/src/outbox.rs` 的 `OutboxAuditSink` 只写自己的发件箱；
>   `DbAuditSink` 与 UA→浏览器/OS 解析一并删除（那两张表不属 `audit`，列也不是它的职责）。
> - **契约**：`crates/ports/src/event.rs` 定义 `DomainEvent`、`EventHandler` / `EventRouter`
>   与负载结构 `OperLogEvent` / `LoginLogEvent`。L3 与 L2 不得互相依赖（§3.3 规则 2/3），
>   所以字段契约放 `ports`，两端共用同一份定义，改名会编译失败而不是运行期炸。
> - **消费者**：`crates/system/src/log_store.rs` 的 `SysLogStore` 是 `sys_oper_log` /
>   `sys_logininfor` 的唯一写入者；`oper_time` / `login_time` 取发件箱行的 `created_at`
>   而非投递时刻，relay 的排队延迟不改写操作发生的时间。
> - **投递**：`Outbox::drain_once`（取一批 → 路由 → 成功盖章 / 失败 `attempts+1`），
>   循环由组装根 `crates/all/src/bootstrap.rs` 的后台任务跑，参数进 §7.9 的 `outbox` 段
>   （`enabled` / `poll_interval_ms` / `batch` / `max_attempts`）。语义是**至少一次**：
>   盖章前进程崩溃会重投，日志表可能有重复行，反之丢事件不可接受。
>   未注册类型与反复失败都留在发件箱，到达 `max_attempts` 才盖章离场并打 `error`——
>   否则毒事件会永久占住 `ORDER BY id LIMIT n` 的扫描窗口，饿死后面的事件。
> - **启用范围**（§15 未决 11）：当前只有审计走 outbox。`notify` 与租户配额仍是直写，
>   等接入第二个跨模块副作用时再迁，避免为单一实现造端口（§3.3 规则 7）。
> - **`outbox_event` 不进 §6.3 的 RLS 白名单**：relay 是跨租户的系统任务，而策略是
>   fail-closed 的（没有 `app.tenant_id` 就一行读不到）；给它开 RLS 等于让审计停摆。
>   租户隔离靠 `tenant_id` 列随事件带着走，落库时由属主表自己的策略兜住。
> - **字段级变更留痕（本节 P1 项，同日落地）**：`sys_audit_log` 按月分区建好
>   （`m20261017_000012_sys_audit_log.rs`：分区键进主键 → `(audit_id, oper_time)`，
>   分区按「前 12 / 后 24 个月」在迁移里动态生成，不写死月份）。
>   差分是纯计算：`cicada_audit::change::diff(before, after)` 只留变化字段
>   `{field: [old, new]}`，并按 `is_sensitive` 把 `password` / `secret` / `token` / `totp` /
>   `hash` 一类列**挡在留痕之外**（留痕表是给人长期读的）。生产者仍只写 `audit.change_log`
>   事件，消费者仍是 `SysLogStore`。
>   接入面：12 个更新接口（用户 角色 部门 菜单 岗位 参数 公告 字典类型 字典数据 租户
>   套餐 定时任务）在提交成功后取「本接口能改的列」做快照对，而不是整行序列化——
>   这样实体以后加了敏感列也不会被顺带留痕。
>   读接口 `GET /api/v1/monitor/audit-logs`（`monitor:auditlog:list`，只读，按
>   `table_name` + `record_id` 查历史）。
>   取舍：新增与删除的留痕**不记**——动作枚举留着（附录 F 允许三种），但同一事实
>   `sys_oper_log` 已经按 `business_type=insert/delete` 记过一遍，两处记账只会互相打架。
> - **验证**：`crates/web/tests/monitor_logs.rs`（`--ignored`，真实 PG）先断言写操作后
>   `sys_oper_log` 仍为 0 条、发件箱有 1 条待发，drain 后才读得到；另一条用例断言
>   未注册事件重试 5 轮后第 6 轮被放弃。实机验证另跑过一次完整登录 + 建角色：
>   relay 自动投递，`oper_time` 与发件箱 `created_at` 逐微秒一致。

> **§7.2 / §12 补记（2026-10-17）**：日志表「按月分区 + 到期归档」本节要求从文档变成可跑的东西。
> 此前 `sys_oper_log` / `sys_logininfor` / `sys_job_log` 是普通表，保留期只能逐行 `DELETE`——
> 表越大越慢，还跟业务写抢同一张表的锁；`§12` 的自动建分区任务同样无处安放。
>
> - **改造**：`m20261017_000013_log_partitions.rs` 用 `CREATE TABLE … (LIKE … INCLUDING DEFAULTS
>   INCLUDING CONSTRAINTS INCLUDING COMMENTS) PARTITION BY RANGE (时间列)` 重建，再搬数据、
>   删旧表——列定义只有一处维护。分区键必须进主键，故主键变为 `(id, 时间列)`。
>   `down()` 能还原成普通表，序列与数据都保住（迁移往返有测试）。
> - **identity 列的坑**：SeaORM 的 `auto_increment()` 在 PG 上生成 identity 列，其内部序列
>   不允许提前 `OWNED BY NONE`（`0A000`）。`LIKE` 又不复制 identity，于是改造后统一落成
>   「普通 bigint + 同名序列 + `nextval` default」——正是参考实现的 serial 形态，
>   SeaORM 插入路径不变（它不送主键）。`setval` 传 `max+1` 且 `is_called=false`：
>   传 `max` 会让下一行**重复**已有主键。
> - **常驻函数**：`cicada_ensure_monthly_partitions(表, 时间列, 前补月, 后铺月, 起始月)` 留在库里，
>   迁移与运行期维护任务共用一份逻辑（不各写一套窗口算法）。起始月可显式传入，
>   已有行早于预铺窗口时，按最早一行所在月起铺；默认从「本表最早一行」与「当前月往前 N 个月」里更早的起算，
>   所以历史时间戳的回写不会撞上「no partition of relation found」。
> - **保留期**：`crates/system/src/log_retention.rs` 的 `LogRetention` 按**整月**判定
>   （下一月首日 ≤ `now - retention_days` 才算到期）——半个月还在期内就删，等于把
>   「默认 180 天」偷偷变成「最多 150 天」。空分区直接跳过，不归档也不删。
> - **先归档后删**：`run()` 的顺序是扩窗 → 找到期 → 导出 NDJSON 放进归档存储 → `DROP`。
>   归档写失败就保留分区，宁可多留一天也不把证据弄丢；`audit.archive = false` 时
>   只报告不删（dry-run），因为本节明确反对「到期直接删除」。
>   对象存储侧走 `ArchiveStore` 端口（`crates/ports/src/archive.rs`），实现由组装根用
>   `Storage` 桥接（`StorageArchive`）——`system`（L2）不该为写归档去依赖 `storage`（L3）。
>   键名 `logs/<表>/<表>_YYYYMM.ndjson`。
> - **`DETACH` 再 `DROP`**：PG 的 `ALTER TABLE … DROP PARTITION` 没有 `IF EXISTS`（`42601`），
>   拆成 `DETACH PARTITION` + `DROP TABLE IF EXISTS` 才既幂等又可控。
> - **RLS 的两个方向都验过**（§6.3 × 本节，实测 PostgreSQL 17.11）：
>   1. 分区子表上的查询**不套父表策略**（父表 `FORCE` + fail-closed 策略时，读父表 0 行、
>      直接读子表 2 行）。保留期任务因此能跨租户归档——这是它唯一正确的读法，
>      也意味着表名/分区名绝不能来自请求：`assert_ident` + `PARTITIONED_TABLES` 白名单先校验再拼 DDL。
>   2. 反过来，往**父表** `INSERT` 会过 `WITH CHECK`：日志表一旦开 RLS，relay 不带
>      `app.tenant_id` 就写不进去（42501），事件反复重试后被放弃——审计静默停摆。
>      修复是 `SysLogStore::execute` 改走 `Db::begin_scoped`，租户从事件本身取
>      （`crates/system/tests/log_store.rs` 正向断言两租户各落一条且互不可见，
>      反向断言无上下文的插入必须被拒）。
> - **调度**：`register_fn` 只挂 handler，跑不跑看 `sys_job` 的行，所以 `seed.rs` 落一条
>   `SYSTEM` 组、每月 1 日 03:00 的「日志分区维护」（`concurrency=禁止并发`，
>   `do_nothing` 冲突——运维暂停过就不再强行恢复）。目标字符串在 seed 与组装根各有一份字面量，
>   `crates/all/tests/framework_jobs.rs` 断言两边一致；`crates/web/tests/jobs.rs` 从调度器装载入口
>   `list_schedulable` 反查这一行，并让它过同一套 `validate_cron`。启动时也会 `ensure_partitions` 一次。
> - **验证**：`migrations/tests/log_partitions.rs`（分区形态 + 往返）、
>   `crates/system/tests/log_retention.rs`（8 个月前的整月被归档后删除、当月不动、
>   dry-run 一行不少）、`crates/web/tests/jobs.rs`、`crates/system/tests/log_store.rs`。
> - **测试台的环境账**：§11.2 的「一个用例一个 schema」在分区表时代会明显放大
>   `DROP SCHEMA … CASCADE` 的共享锁数量（CASCADE 要为事务里每个对象取锁；日志分区把每个
>   schema 的对象数从 ~620 推到 ~750），10 个用例并行即撞 `53200 out of shared memory`。
>   已在 `docker-compose.yaml` 的 postgres 上设 `max_locks_per_transaction=256`，
>   这是 PG 对这类工作负载给的正解，不改生产形态也不缩减分区窗口。

> **§6.1 补记（2026-10-17）**：`sys_user_online` 从「文档里的一张表」变成真的表 + 真的写进去（§5.5 清单第 423 行）。
> 此前会话生命周期只活在 Redis 里：登录有 `sys_logininfor` 记账，**退出没有任何记账**——
> 进程重启后「谁在什么时候下线」查无对证。
>
> - **属主**：`auth`（会话事实的生产者）。迁移 `m20261017_000014_sys_user_online.rs`，
>   实体与写入器在 `crates/auth/src/online.rs`；§3.3 规则 6 由此成立——只有 `auth` 写这张表。
> - **形态**：`SnapshottedSessionStore` 装饰真正的 `SessionStore`（组装根与测试台都装它），
>   Redis 操作成功后才写快照。判定路径（中间件校验会话存在性、`max_sessions` 上限、踢出）
>   仍只看 Redis——本节明令「不参与任何踢出/上限判定」。
> - **不阻塞主流程**：快照写在 `tokio::spawn` 里，失败降级为 `warn`
>   （与 §7.2「审计写入不阻塞主流程」同一条理由；DB 抖动不该让登录变慢或失败）。
>   幂等靠主键：`session_id`（=`jti`）做 PK，`INSERT … ON CONFLICT DO UPDATE`。
> - **可重建**：`reconcile_with(store)` 以权威源为准——Redis 里还存在的会话 upsert 回「在线」，
>   库里挂着「在线」但权威源已没有的行收成「已退出」（进程被杀、TTL 自然过期这两条路径
>   不经过 `revoke`，只有对账能收口）。组装根启动时跑一次。已退出的行**不删**，历史价值就在这些行里。
> - **读接口**：`GET /api/v1/monitor/online-sessions`（`monitor:online:history`），
>   按 `user_name` / `ip` / `status` / `login_time` 区间过滤，走 `ScopedQuery` 的租户与数据范围
>   （与 `monitor/oper-logs` 同一套读法）。权限点已进种子（`sys_menu` 1093，挂在「在线用户」109 下），
>   受 `perm_coverage` 的两向对账约束。
> - **不进 §6.3 的 RLS 白名单**：快照写是会话生命周期上的系统侧写，写路径上没有请求级租户上下文；
>   给它开 RLS 只会让 write-behind 静默失败。租户隔离在**读**侧由 `Scope` 兜住，与 `outbox_event` 同一取舍。
> - **验证**：`crates/auth/src/online.rs` 单测断言 SQL 形状（幂等 upsert、`end_time IS NULL`
>   才标记退出、`last_seen=0` 的老会话回落到 `login_time` 而不是 1970）；
>   `crates/web/tests/online_users.rs::session_history_is_written_behind_and_reconcilable`
>   在真实 PG 上验完整链路：write-behind 落两行 → 历史接口读到且 `end_time` 为空 →
>   强退后该行变「已退出」且仍能被 `status=1` 查到 → 手工插入的幽灵行由 `reconcile_with` 收口。

> **§5.3 补记（2026-10-17）**：本节的「模糊查询用 PG 原生 `ILIKE`，高频模糊列建 `pg_trgm` GIN 索引」
> 两半都落地。此前所有搜索框走的是 SeaORM 的 `contains`，它生成 **`LIKE`**，于是两头都不合规：
> 大小写敏感（搜 `admin` 命中不了 `Admin`），而且**不转义通配符**——用户在搜索框里打一个 `%`
> 就变成 `%…%`，语义从「含 % 的行」变成「所有行」，扫描量跟着放大。
>
> - **单一出口**：`crates/web/src/search.rs` 给列加 `fuzzy()` = `ILIKE '%…%'` + 把输入里的
>   `\` `%` `_` 转义成字面量。13 个模块文件里 25 个列、36 处过滤点全部改走它，
>   没有第二份拼模式的写法（以后加搜索框也只能用它，否则 review 时一眼看得出来）。
> - **索引清单**：`m20261017_000015_trgm_indexes.rs` 的 `FUZZY_COLUMNS` 是「哪些列真的被模糊过滤」
>   的镜像（25 对），并在 `cicada_migrations` 根导出，让迁移与核对测试读同一份数据。
>   只给有过滤条件的列建索引：每个 GIN 都是写放大，无主不建。
>   分区表（`sys_oper_log` / `sys_logininfor`）在**父表**上建，PG 自动传播到所有分区，
>   之后维护任务新建的分区也自动带上（§7.2）。
> - **`search_path` 的坑（实测踩到）**：SeaORM 的 `set_schema_search_path(schema)` 生成
>   `SET search_path = "<schema>"`，**不带 `public`**。所以扩展必须 `WITH SCHEMA public`
>   固定落点（扩展是库级对象，一个库里只有一份，装在谁的首个 `search_path` 上纯属偶然），
>   而 opclass 要写全限定名 `public.gin_trgm_ops`——裸写会在 schema-per-test 的库里
>   报 `42704 operator class "gin_trgm_ops" does not exist`。
> - **`down()` 不删扩展**：同库还有别的 schema 在用这些索引，删扩展会让别人的迁移半途炸；
>   只删自己建的 25 个索引。
> - **验证**：`migrations/tests/trgm_indexes.rs` 三条用例——清单里每一对都真的建成
>   `USING gin` + `gin_trgm_ops`；60k 行规模下 `ILIKE '%kw%'` 谓词确实进 `Index Cond`
>   （小表上规划器永远选顺序扫描，那与索引可用性无关，所以关掉 seqscan 证明「可被服务」）；
>   `ILIKE` 大小写不敏感而转义后的 `%_` 是字面量。HTTP 侧由
>   `crates/web/tests/online_users.rs` 断言 `?user_name=ADMIN` 命中 1 条、`?user_name=%` 命中 0 条。

> **§7.8 补记（2026-10-16）**：`/metrics` 从占位文本（只有硬编码的 `cicada_up 1`）改成本节要求的真实导出，
> §4.1 的 `metrics-exporter-prometheus` 自此真正上线（`crates/infra/src/metrics.rs`，0.18.3 + `metrics` 0.24.6）。
>
> - **在事件点记录**：`http_requests_total{method,route,status}` 与
>   `http_request_duration_seconds{method,route}`（`layers::http_observability`，流水线第 2.5 层，
>   在 trace 之内、安全头之外，所以预检与短路响应也计入）；`app_errors_total{code}` 在
>   `Problem::new` 这一唯一失败出口计数；`job_runs_total{status=success|failed|skipped}` 在
>   `crates/worker/src/executor.rs`；`outbox_deliveries_total{kind}` 在发件箱 relay（§7.2）。
> - **在抓取时采样**：`db_pool_connections{state=total|idle|max|min}` 与
>   `cache_hit_ratio` / `cache_keyspace_*` / `cache_used_memory_bytes`——这两项只有被 scrape 时
>   才有意义，代价是一条 `INFO` 与两个池计数器（`Db::pool_live()`，sqlx 不回读 max/min，
>   上限由 `db.max_connections` 给出）。
> - **基数纪律**：`route` 标签不是原始路径，`normalized_route` 把纯数字、UUID、长十六进制段
>   压成 `{id}`；否则 `/api/v1/system/users/123` 会让时间序列随数据量线性膨胀。
> - **分桶与零值**：延迟桶固定为 1ms–5s（Prometheus 要求严格递增）；`job_runs_total` 与
>   `outbox_deliveries_total` 在 `install()` 里预置 0 值序列，避免第一次事件前面板显示「无数据」。
> - **链路（同日本节补完）**：`opentelemetry` / `opentelemetry_sdk` / `opentelemetry-otlp`
>   （OTLP/HTTP + protobuf，复用已有 reqwest，不引 tonic）与 `tracing-opentelemetry` 上线，
>   `crates/infra/src/telemetry.rs::init_tracing_with` 按 §7.9 的 `telemetry` 段决定是否导出：
>   `otlp_endpoint` 为空只出 stdout；配了就在同一 subscriber 上挂 OTLP layer
>   （`service.name` 资源属性、`sample_ratio` 走 `ParentBased(TraceIdRatioBased)`、批处理器，
>   `TracingGuard` drop 时 force_flush + shutdown，短 CLI 不丢尾批）。
> - **为什么自己开 `http.request` span**：`TraceLayer` 的请求 span 是 TRACE 级，
>   生产日志级别下它压根不存在，链路就没东西可导。第 2.5 层（`layers::http_observability`）
>   因此用 INFO 级 span，属性按 OTel 语义约定命名（`otel.kind=server`、`http.request.method`、
>   `http.route`、`url.path`、`http.response.status_code`，5xx 另标 `otel.status_code=ERROR`）。
> - **DB 子 span**：`Db::connect` 打开 `record_stmt_in_spans`，并在 filter 里补一条
>   `sea_orm=trace`（SeaORM 的语句 span 是 trace 级：`sea_orm::database` / `sea_orm::driver::*`）。
>   该 target 不产生额外事件，所以 stdout 一行不多；操作方自己写了 `sea_orm=` 时不覆盖。
> - **验证**：本地起一个假 collector（只记账 `POST /v1/traces` 的 protobuf 体）+
>   独立 schema 起服务，`RUST_LOG=cicada=info`（不给任何 trace 级开关）下抓到的批次里
>   确有 `http.request` 与其下的 66 个 `sea_orm::*` 子 span、`service.name` 资源属性，
>   且服务端 stdout 零新增噪声。

> **§9.1 补记（2026-10-17，会话指纹）**：「会话绑定 `(ip, user_agent)` 指纹」以前只是会话记录里
> 存了这两个字段，没有任何地方比较过。现在认证中间件每次都拿请求与 `session:{jti}` 比一次：
>
> - **`user_agent` 变了 → 撤销会话 + 401 `CICADA_AUTH_1011`**：语义是「去重新登录」，
>   不是「无权限」。撤销发生在权威源（Redis）上，所以未过期的 access_token 也随之失效——
>   令牌被复制到别的浏览器/设备这种场景，攻击者下一次请求就要重新过登录，而不是拿着令牌跑到过期。
> - **IP 变了只记一条 `info` 并放行**：正是本节警告的「移动网络下 IP 会变，不能做硬绑定」。
>   也不回写会话里的 IP：回写要拿剩余 TTL 重写整条记录，而存储侧没有「保持 TTL 更新」的原语；
>   何况 `sys_user_online` 记的登录 IP 是审计值，跟着连接漂反而没意义。
> - **缺信息不判**：任一侧 UA 或 IP 为空就当「不可比」放行。把「客户端没发 UA」当成攻击证据，
>   等于把 §9.1 反对的硬绑定换个形式又装了回来（不少非浏览器客户端就是不发 UA）。
> - **判定是纯函数**（`middleware::fingerprint`），三个分支各有单测；HTTP 侧
>   `crates/web/tests/auth_flow.rs::session_fingerprint_revokes_on_user_agent_change_only`
>   验完整链路：换 IP 放行 → 换 UA 得到 1011 → 换回原 UA 已经进不去（证明撤销真的落到了 Redis）。
> - **API Key 走的是另一条路**（`X-API-Key`，机器凭证），不参与 UA 指纹。

> **§9.1 补记（2026-10-17）**：口令规则从「写在文档里」变成真正拦得住东西的东西。
> 此前的样子：`auth.password.min_length` 配了但**没有任何读者**，重置密码那里硬编码
> `password.len() < 6`——比本节的 8 位松、不查大小写与数字，而且按字节计（中文口令会被算长）。
>
> - **规则与阈值同处**：`crates/infra/src/password_policy.rs` 的 `password_problems(pw, min_length)`
>   与 `password_problem_text(..)`。放 `infra` 而不是 `auth` 是被依赖图逼的：
>   `migrations`（`init_admin` / `tenant init` / `admin reset`）必须用同一份规则，
>   而数据层不许反向依赖认证域；阈值本来就配在 `config::PasswordConfig`，规则跟阈值分开住迟早会漏。
> - **判定口径**：长度按**字符**数（不是字节），且需同时含大写、小写、数字各至少一个，允许特殊字符。
>   违规项一次报全（`["缺大写字母", "缺数字"]`），不「一条一条挤牙膏」。
> - **四个入口都拦**：管理员建号（`POST /system/users`）、管理员重置
>   （`PUT /system/users/{id}/password`）、导入用初始密码（`sys.user.initPassword` 参与导入时先校验，
>   不合规则整批不执行——否则导入的是一批「落地就得改」的账号）、CLI 初始化与重置。
>   返回按 §7.1：422 + `errors:[{field:"password", message}]`，前端表单能贴回密码框。
> - **种子的初始密码换了**：`sys.user.initPassword` 从参考实现的 `123456` 改为 `Cicada@123`——
>   留着 `123456` 等于让默认配置直接撞自己定的规则。§9.1 的
>   `sys.account.chrtype = 3` 只是参数占位，实际以更严的本节为准。
> - **没做的**：自助改密接口不在 §6.1 / 附录 E 的承诺里，本次不凭空加；
>   `sys.account.initPasswordModify`（首次登录强制改密）仍是配置占位，它需要登录响应带标志位
>   与前端流程配合，属于 §10 的活。
> - **验证**：`crates/infra/src/password_policy.rs` 单测（含「中文按字符不按字节」与
>   `min_length` 生效边界）；`crates/web/tests/system_crud.rs::password_policy_is_enforced_and_config_driven`
>   在真实 HTTP 面上断言弱口令被 422 拦下、消息里两项违规都在，并把配置的阈值改成 12
>   后同一口令改判为「长度不足」——证明那条配置真的有人读。

> **§7.8 补记（2026-10-17，就绪探针的错误形状）**：`/readyz` 失败时原本回一个裸
> `{"status":"unavailable"}`——既不符合 §7.1「失败响应用 problem+json」，也没回答运维真正要问的
> 那句「是 PG 掉了还是 Redis 掉了」。现在按依赖返回附录 C 的业务码：PG →
> `CICADA_INTERNAL_9002`、Redis → `CICADA_INTERNAL_9003`，两个都坏时报 PG（更根本的那个）
> 但在 `detail` 里写全；`title` 与 `code` 由 `BizCode` 给出，状态码仍是 503 不变，
> 探针判定不受影响。映射是纯函数 `readiness_failure(db_ok, redis_ok)`，三个分支都有单测；
> HTTP 形状由 `crates/web/tests/health_probe.rs` 用一个「只有 `ping` 失败」的
> `SessionStore` 替身验证 content-type 与 `code`/`title`/`detail`。`/healthz` 保持不查依赖。

> **§16.1 补记（2026-10-16）**：`Manifest.menus` 过去是「有字段、无出口」——`Assembler::with()`
> 把清单转成 `ModuleRecord` 时直接丢掉了 menus，模块声明的菜单永远进不了 `sys_menu`；
> 本节的「perms / menus 交给该表属主注册的端口落库，属主缺席即失败」只做到了后半句的对账，
> 前半句没有实现。现在：
>
> - **端口**：`crates/ports/src/menu.rs` 的 `MenuRegistry`（`upsert` / `known_permissions`）+
>   `MenuSpec`（`module` / `name` / `kind` / `permission` / `path` / `parent` / `order`），
>   测试夹具 `InMemoryMenuRegistry` 满足 §3.3 规则 7 的双 adapter 要求。
> - **属主实现**：`cicada_system::DbMenuRegistry`。幂等键是「同权限标识，否则同名同类型」；
>   插入顺序固定为目录 → 菜单 → 按钮，父级才定位得到；`Button` 不给 `parent` 时按
>   `xxx:view` 约定挂到同模块的 `C` 菜单下，推断不出来返回 `MissingParent` 而**不是**悄悄挂根。
>   新行不写 `menu_id`，走序列（种子已 `reset_sequences`）。
> - **出口**：`Assembled::declared_menus()` 把 `MenuNode` 转成 `MenuSpec` 带出来；
>   `router_with_menus()` 返回 `(Router, Vec<MenuSpec>)`（`router_with` 是它的忽略菜单版），
>   因为落库是异步的、且必须发生在组装根。`AppBuilder` 默认用 `DbMenuRegistry`，
>   `with_menu_registry(...)` 可换 adapter（微服务形态把菜单归到独立服务），
>   upsert 失败即 `BootstrapError::Assemble`，不留半个可用服务。
> - **框架自带的菜单仍留在 `seed.rs`**：菜单种子要求固定 `menu_id`（附录 D 的 100/101/…），
>   自动分配的序列给不出同样的 id；声明式通道服务的是新增模块（`business:*` 一类）。
>   `crates/web/tests/module_contract.rs::declared_menus_are_carried_out_as_specs` 守声明侧，
>   `crates/system/tests/menu_registry.rs`（`--ignored`，真实 PG）守落库侧：插三次得 3 行、
>   重复 upsert 得 0 行、父级指向正确、孤儿按钮不写库。

> **M3/M4 补记（2026-10-16）**：把 §7 里最后几项「只有接口没有实现」的能力补齐，并交付 §10 剩下的两个界面。
>
> - **服务信息 API**（§7.8）：`GET /api/v1/monitor/server`（`crates/monitor/src/server.rs` 采集 +
>   `crates/web/src/server.rs` 出参），CPU/内存/Swap/负载/磁盘/进程与系统运行时长/版本一次采齐；
>   CPU 使用率是差值采样，两次刷新必须间隔 `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`（Linux 上 200 ms），
>   短于它 sysinfo 直接返回 0——回归测试见 `crates/monitor/tests/server_stat.rs`。
> - **数据脱敏**（§7.10）`#[mask]` 派生宏、**登录图形验证码**（§6.1/§9.1 失败后强制）、
>   **Idempotency-Key 防重复提交**（§7.10）、**服务端国际化**（§7.10，rust-i18n）、
>   **异步导出** `sys_export_task`（§7.5/附录 F）落地。
> - **前端**：`views/monitor/server`、`views/system/{tenant,package,apikey}` 四页；
>   API 凭证的签发响应在后端是 `#[serde(flatten)]`，线上 JSON 平铺，前端类型据此建模（不套 `key` 层）。
> - **仍递延**：主子表 `sub` 模板按 §7.7 在渲染阶段明确报错。
>   （这条名单原本还有「异步导出的完成通知」，已在 2026-10-17 交付，见下方「§7.5 补记」。）
> - **工程规范（§11.1）补做**：17 个 crate + `apps/cli` / `apps/server` 全部有 README，
>   按「职责 / 公开 API / 依赖 / 不做什么」四段写，其中「不做什么」承担边界声明（例如
>   `monitor` 不写「CPU 采样必须 ≥ 200 ms」这类硬约束就会被反复踩）；
>   CI 的 `rust` job 增加 `cargo doc --no-deps --workspace` + `RUSTDOCFLAGS=-D warnings` 一步，
>   顺带修掉 5 个 crate 的坏 intra-doc 链接（`kernel::scope` 指向私有子模块、`ports::job` 的
>   方括号误用、`worker::schedule` 停留在改名前的 `CronRunner`/`ApalisRunner`、
>   `codegen::templates` 与 `web::codegen` 指向私有项），并补齐 `rbac::PermissionSet`
>   缺文档注释的 7 个方法；本地跑通 `cargo doc --no-deps --workspace` 全绿。

发行节奏：M0–M4 先交付**大单体版**；微服务版在 M4 出口加一条「把 `auth` 拆成独立进程、服务间走 gRPC 跑通一次」，用同一套 crate 组合出微服务发行版（见 16.4）。客户端：本期随 M1 交付 v1 Vue 前端骨架、随 M3 交付代码生成器的 Vue 产物；React / Svelte / uni-app / Tauri 版本在 M5 之后按需增量。

> **M0/M3 补记（2026-10-15）**：补齐「模板化二次开发」这条此前只在文档里的路（§16.3）——
>
> - **门面 crate**（`crates/all`）：`cicada::bootstrap` 把 `apps/server` 原先手写的
>   ~100 行装配下沉成库（配置 → 连接池/缓存/端口 → 模块 → 中间件链 → 起服务），
>   `apps/server` 因此缩到 20 行，与使用者工程**共用同一条装配路径**，升级不会分叉；
>   同时提供分层别名（`cicada::web` / `cicada::kernel` / …）与 `prelude`。
>   feature 只覆盖真正可选的重型部件（`full` = `worker` + `migrations` + `codegen`）；
>   领域模块的裁剪靠**不注册模块**，不靠 feature——`AppState` 的形状决定了那些 crate
>   必须参与编译，与其做成假开关不如说清楚。
> - **组装缝**（`crates/web::router_with`）：内置模块 + 业务模块进同一个装配器，
>   业务模块同样受重名检测、权限点双向对账约束（`crates/web/tests/business_module.rs`）。
> - **模板工程**（`template/` + `cicada new`）：使用者工程只碰 `business/` `migrations/`
>   `templates/` `config/`，框架走依赖；`cicada new --cicada path=<工作区>` 支持本地开发，
>   渲染是失败关闭的（残留占位符即报错）。样例业务模块**就是**生成器产物，
>   由 `template_embeds_the_same_generated_module` 逐字节保证不漂移。
> - **迁移形态修正**：两个 `MigratorTrait` 实现共用一个 `seaql_migrations` 表会被 SeaORM
>   判为「版本对不上本地文件」而拒绝启动，故新增 `cicada_migrations::framework_migrations()`，
>   使用者工程把它拼在自己的迁移列表前面——**只有一个** `Migrator`。
> - **CI**（`.github/workflows/ci.yml`）：fmt → clippy(`-D warnings`) → 单元 → 集成（真实
>   PG/Redis）→ 迁移往返 → 模板工程渲染 + 编译 + 迁移。为此清掉了工作区里 4 处既有
>   clippy 告警（`-D warnings` 从「过不去」变成「过得去」）。依赖断言仍缺 `deny.toml`
>   与许可证裁决（§15 #9），暂不引入。

> **§8 / §7.10 补记（2026-10-17，OpenAPI 的第二个出口）**：接口路径在项目里有**两处**声明——
> handler 上的 `#[utoipa::path]` 和 `ApiDoc` 的 `paths(...)` 清单。只漏第二处没有任何编译期信号，
> 而且快照比对照样自洽（它比的是「生成物 vs 已提交文件」，两边同时缺一条就一致）。
> 实测漏了 13 条：附录 E 那批（角色启停 / 数据范围 / 用户分配、菜单排序与角色勾选树、
> 部门排序、公告已读名单与顶栏、字典按类型）与 §6.1 的 `/api/v1/monitor/online-sessions`。
> 补齐后 108 paths / 129 schemas / 158 operations。
>
> - **别名接口的形状**：`/system/depts/tree`、`/system/menus/tree` 原本直接复用
>   `list_depts` / `list_menus`，所以文档里压根没有它们——一个函数只能挂一条 `#[utoipa::path]`。
>   改成薄转调（`list_depts_tree` / `list_menus_tree`）：路由、权限点、响应一概不变，
>   只是多出一条能被登记的声明。utoipa 没有「同一操作挂两条路径」的原语，硬造不如把
>   这 6 行写清楚。
> - **门禁**：`every_declared_path_is_registered_in_the_document`
>   （`crates/web/tests/openapi_snapshot.rs`）扫 `crates/web/src` 下每条
>   `#[utoipa::path]` 的 `path = "…"`，逐条断言它出现在生成的文档里；
>   失败信息点名「文件 + 路径」，不是只报「快照不一致」。反向不用查——
>   删了 handler 会先在 `paths(...)` 的 import 上编译失败。
> - **为什么扫源码而不是枚举 Router**：枚举真实路由要 `AppState`（连库），
>   这条对账就只能挂 `--ignored`；而它需要的只是源码，于是留在毫秒级那档。
>   真要接 `Router` 得引 `utoipa-axum` 并改写全部 `Routes`，为一条对账不值。
> - **快照的阈值同步上调**（operations 140→150、schemas 110→120）：下限拦的是
>   「整段掉下去」，留着旧数字就等于没有。

> **§12 / §11.3 补记（2026-10-17，镜像这条路径真跑过一遍）**：`docker build` +
> 冒烟在本地完整走通：全新库 → 镜像里的 `cicada-cli migrate up / seed / init-admin`
> → 起服务 → `/readyz` 2 秒内就绪 → 容器健康检查转 `healthy` → 登录拿 EdDSA token
> → 本轮登记的 8 条接口全 200 → `/api-docs/openapi.json` 从镜像里给到 108 paths。
> 途中修掉三个「看着对、跑不了」的东西：
>
> - **`--entrypoint`**：CI 原来写 `cicada-server:ci cicada-cli migrate up`。镜像的
>   ENTRYPOINT 是 `cicada-server`，这条展开成 `cicada-server cicada-cli migrate up`——
>   server 不解析参数，于是这一步不迁移，而是起一个占住 8080 的服务。改成
>   `--entrypoint cicada-cli`，三步并成一个循环。
> - **基础镜像的补丁号**：`rust:1.98-alpine` 装的是 1.98.1，而 `rust-toolchain.toml`
>   钉 1.98.0，rustup 于是在构建中途去 `static.rust-lang.org` 下整套工具链；
>   内网/离线环境下报的是 `tls handshake eof`，完全看不出原因。钉成
>   `rust:1.98.0-alpine` 后构建不碰网络（`vendor/` 里的 swagger-ui zip 是同一动机）。
> - **指标名**：除 `cicada_up` 外，§7.8 的序列都不带前缀（`http_requests_total` /
>   `db_pool_connections` / `job_runs_total` / `outbox_deliveries_total` / `cache_*`），
>   前缀交给抓取端的 job 标签。CI 冒烟原来只 grep `^cicada_up`，等于什么都没验；
>   现在按真实名字断言 `^http_requests_total{`，并检查 openapi 路径数下限。
> - **端口不是 8080 的机器**：`docker-compose.yaml` 早就因此映射 8081（本机 8080 常被人占）。
>   冒烟用 `-p 18080:8080`，容器内仍听 8080，镜像里的 `HEALTHCHECK` 因此不用改——
>   服务端口目前只能由配置文件决定（env 只覆盖 `DATABASE_URL` / `REDIS_URL`），
>   把 HEALTHCHECK 做成跟着端口走需要先是条能被 env 覆盖的配置，那是另一件事。
> - **顺带纠正两处过时注释**：`docker-compose.yaml` 与 `infra::Config::load` 都写着
>   「镜像里 `config/default.yaml` 是烘进来的」，而 Dockerfile 明确**不**烘（烘了会让
>   镜像随开发者机器漂移，且文件一旦存在 env 就盖不住端口）。注释按现实改了，代码没动。

> **§11.1 / §3.3 补记（2026-10-17，依赖与分层的机器可检部分）**：§13 的 M0/M3 补记里
> 「依赖断言仍缺 `deny.toml` 与许可证裁决」这条清掉了——`deny.toml` 上线
> （advisories / bans 重复版本 / licenses 白名单 / sources 只允许 crates.io），
> CI 的 `rust` job 增加 `cargo deny check`、`cargo machete`、
> `python3 deploy/check_layering.py`（§3.3 的层图：L0→L4 谁能连谁，读 `Cargo.toml`，
> 秒级，不用编译整个 workspace）与单列的 OpenAPI 快照一步。
> `cargo machete` 按目录跑（`crates apps migrations`）而不是裸跑仓库根：
> `template/Cargo.toml` 是 cargo-generate 模板，里面 `{{cicada_dep}}` 不是合法 TOML，
> 裸跑会打一坨解析错误（退出码仍是 0，但那一行会让人以为门禁坏了）；
> 模板工程由自己的 job 渲染后 `cargo check`。许可证白名单里
> `MIT OR Apache-2.0` 这类双许可按第一个算，不需要逐个列组合。
> §15 #9 的另一半（cicada 自己用什么许可证）已在 `Cargo.toml` 写为
> `license = "MIT OR Apache-2.0"`，但那是代码里的事实，**不等于**项目所有者已裁决，
> ADR 仍待补。
>
> - **`rust` job 的 PostgreSQL 从 service 改成 `docker run` step**：§11.2 与 §7.2 都写了
>   测试台需要 `max_locks_per_transaction=256`（一个用例一个 schema、跑前 `DROP SCHEMA … CASCADE`，
>   分区表把单 schema 对象数推到 ~750，默认 64 会撞 `53200 out of shared memory`），
>   compose 里设了，CI 却没有地方设——service 的 `options` 是 `docker create` 的参数，
>   传不进服务器命令行。改成 step 后同一步顺带断言
>   `show max_locks_per_transaction` 必须是 256，这条环境前提自此由 CI 自己把关，
>   不再靠人记得去改 compose。`template` / `image` 两个 job 只建一次库、没有并行
>   CASCADE，继续用 service。
> - **§11.3 的编译加速取「按 crate 缓存」这一档**：`Swatinem/rust-cache@v2` +
>   `pnpm` 缓存。`sccache` 与 `cargo-nextest` 没上——它们省的是 runner 上的重复编译与
>   测试启动开销，代价是 CI 多两套工具各自的失效模式；当前套件（90 个测试二进制、
>   单元档 3 分钟内）没到那个瓶颈。这是刻意的不做，不是遗漏。
> - **CI YAML 结构**（改后）：4 个 job — `rust`（15→16 步，含起 PG）、`template`、
>   `frontend`、`image`，与 §11.3 那条流水线逐项对应。

> **§7.5 补记（2026-10-17，导出完成 → 通知中心）**：本节的「大表走 `sys_export_task` 异步任务，
> **完成后经通知中心给下载链接**」此前只做到前半句——产物落 `sys_file`、下载端点可用，
> 但没有任何东西去通知发起人，链接靠用户回列表页翻。现在 `run_task` 的成功分支会调
> `notify_export_done(...)`，走 `NotifyService::send`（§7.2 之外的第二个通知中心调用方）。
>
> - **模板进种子、渠道不进**：模板 `export.done`（zh-CN / en-US 两份，`channel_kind = email`）
>   是租户级数据，跟着 `seed()`（默认租户）与 `init_tenant_admin`（新建租户）一起写；
>   渠道带 SMTP 端点与凭据，按 §7.9「敏感值一律来自环境变量或密钥管理」的边界**不进种子**，
>   由运营在 `/api/v1/notify/channels` 建。链接写的是 API 路径而不是绝对 URL：
>   基座没有「对外可访问地址」这项配置，凭猜一个 host 反而更容易点不开。
> - **送不出去就只记日志，不改任务状态**：没有归属用户 / 创建者没填邮箱 / 租户没配启用渠道 /
>   投递失败——四种情况产物都已经在 `sys_file`，列表页与下载端点照常工作。
>   理由与 §7.2「审计写入不阻塞主流程」同一条：一条送不出去的邮件不该把成功的导出改成 failed。
> - **收件人查询不套数据范围**：只按 `tenant_id + user_id` 取发起者的邮箱。
>   这是「给动作的发起者回话」，不是业务取数——数据范围把发起者自己筛掉，
>   通知就永远发不出去，而这恰恰是最需要它的场景（能看全表的人才会导全表）。
> - **验证**：`crates/web/tests/async_export.rs` 新增两条。
>   `export_completion_goes_through_the_notify_center` 先跑「无渠道 → 任务 DONE 且零投递记录」，
>   再建渠道 + 填邮箱，断言落了一条 `sys_notify_record`、收件人是创建者、
>   `payload->>'task_id'` 对得上这次任务；`export_template_code_matches_the_seed` 对账
>   种子与生产者各持一份的模板码字面量（`web`(L4) 不得依赖 `migrations`，同 §7.2
>   框架任务目标串的处理）。
>
> **M2 出口条件补记（2026-10-17，「导出 10 万行内存稳定」改成量出来的数）**：这一格原本是
> 「⏳（无压测环境）」，但那句里能本地验证的部分其实有两件——**写得完**与**内存不随行数膨胀**，
> 两者只需要一台跑得动 PG 的机器。`crates/web/tests/export_scale.rs`（`--ignored`）用
> `generate_series` 落 10 万行后走同一条导出路径，每批回调里采一次 `/proc/self/statm`：
> **实测 6.9 秒、产物 2 177 572 字节、RSS 12 MiB → 15 MiB（增量 3 MiB）**。
> 断言三件事：产物 > 1 MiB 且是 zip 头（跑得起来）、RSS 增量 < 256 MiB（拦「把分批读库
> 改成整表 `all()`」这类退化——那会轻松越线，而 256 MiB 的余量又不至于在共享 CI 上误报）、
> `MAX_EXPORT_ROWS` 仍是 100_000（§7.5 的 10 万是承诺值不是参考值）。
> HTTP 层的 QPS / 延迟分位仍然要压测环境，M4 那一格照旧记 ⏳。

> **§8 补记（2026-10-17，排序白名单从契约变成行为）**：接口规范表里那行
> 「请求 `page` + `page_size` + **排序白名单 `sort_by` / `sort_order`**」此前没有任何实现——
> 所有列表的次序都写死在代码里，前端表格点列头不会换序。现在 `crates/web/src/sort.rs`
> 提供唯一的判定口（`resolve` 纯函数 + `SortSpec` 白名单常量），19 个列表接口逐个接线：
> 用户 / 角色 / 岗位 / 字典类型 / 字典数据 / 参数 / 公告 / API 凭证 / 租户 / 租户套餐 /
> 操作日志 / 登录日志 / 定时任务 / 文件 / 异步导出任务 / 会话历史 / 通知渠道 / 模板 / 记录。
>
> - **为什么必须是白名单**：§5.3 的规矩是不让客户端控制 SQL 标识符。自由文本的 `sort_by`
>   既是注入面，也是结构探测面（`column "totp_secret" does not exist` 就是把表结构送出去）。
>   对外名字与真实列名分两段存，落到 SQL 的只有常量那一份。
> - **不在名单里的名字 → 忽略，不是 422**：列表页多传一个字段最该发生的是「照常返回数据」，
>   而不是整页打不开。`sort_order` 只认字面量 `asc`，其余一律 `desc`
>   （拼错的顺序当成升序更反直觉）。
> - **接线顺序是唯一真正的坑**：`.sorted_by()` 若排在「以唯一主键开头」的默认排序之后，
>   显式排序永远只是平局裁决——传了等于没传，而编译与单元测试都看不见这件事
>   （`sort::resolve` 的单测只证明「名字被接受」，不证明「SQL 里它在前」）。
>   所以 19 处一律 `sorted_by` 在前、默认次序退化成同值行的收尾键（保证翻页不重不漏）。
>   变异验证：把 `crates/web/src/system/post.rs` 的两步换回原顺序，
>   `sort_whitelist_reorders_lists_and_rejects_unlisted_names` 立刻失败并抖出未按名字排序的响应体。
> - **两张表共用一个 `Select` 版扩展**（`sort_select.rs`）：`sys_dict_type` / `sys_config` /
>   `sys_notice` 是全局表，没有租户维度因此不实现 `DataScoped`、进不了 `.scoped()` 通道，
>   走的是裸 `Select<E>`。判定仍然是 `sort::resolve` 那一份，没有新增信任边界。
>   这也说明 §16.1 说的「深模块」不是「所有路径走同一个类型」——端口形态由数据决定。
> - **§8 的「弃用」那行仍未实现，是刻意的**：`Sunset` + `Deprecation` 需要有一个**真的被宣布
>   废弃**的接口才有落点。当前 95 条路径里没有一条处于弃用期，为假想需求先造一层
>   中间件与清单结构，正是 §16.1 反对的「浅接口」。第一条弃用出现时按这条顺序落地：
>   `Manifest` 里声明弃用元数据 → 中间件加两个头 → 生成器与 OpenAPI 标 `deprecated`。
>   （§16.3 的升级通道那句「废弃接口给一个版本的弃用期」是策略，不是当前缺口。）


> **§5.2 / §11.2 补记（2026-10-17，乐观锁从「一列」变成「一道闸门」）**：`version` 列一直建在库里
> （`sys_user` / `sys_role` / `sys_dept` / `sys_post`），但四个 update handler 都是无条件覆盖、
> `version` 从不递增，读接口也不回传它——§5.2 那句「并发编辑报明确冲突，而非静默覆盖」
> 当时只是表结构，没有任何行为。附录 C 的 `CICADA_SYS_3003` 也已经登记了，却没有任何代码会发出它。
>
> - **SeaORM 2.0.4 没有 `#[sea_orm(version)]`**（本节的原写法是照着想象中的 API 写的，已在 §5.2
>   就地改正）：源码里搜不到 opt-lock / `OptimisticLockError`，宏也不认 `version` 属性。
>   于是手写 CAS：`UPDATE … SET 各列, version = 期望 + 1 WHERE id = ? AND version = 期望`，
>   `rows_affected == 0` 就是冲突。写在 handler 里而不是抽象成通用 helper——四个实体的列不同，
>   泛型化要引入「列清单」这层新概念，而 §3.3 规则 7 明确反对为单一实现造端口。
> - **API 形状**：VO 多回一个 `version`（纯新增，存量客户端不受影响），
>   update 的 DTO 多一个 `version: Option<i32>`。**带了才检查，不带仍是后写覆盖**——
>   强制要求会让不带该字段的存量客户端整批 409，那属于 §2.2 不承诺的兼容破坏；
>   但即便不带，`version` 照样推进，客户端随时可以开始回传。
>   前端四张表的提交体都是显式构造的对象（不是把整行 spread 回去），因此这次改动对它零影响。
> - **从属数据不参与判定**：角色的菜单/部门关联、部门子树的 `ancestors` 重写是本角色/本节点
>   自身推导出来的结果，不该让一次并发关联变更把主行编辑判成冲突。
> - **§11.2 的「关键并发行为有专门测试」自此四项齐了**：会话踢出（`auth_flow`）、
>   乐观锁冲突（`system_crud::optimistic_lock_governs_every_versioned_entity`）、
>   分布式锁选主（`worker/tests/job_runtime`）、并发导入与并发投递（`m4_features` 等）。
>   新测试对四个实体逐一表驱动：建号版本必为 0 → 带对版本改成功且版本 +1 →
>   用同一个旧版本再改必 409 + `CICADA_SYS_3003` → 库里那一行没被旧请求写坏 →
>   不带版本仍可改但版本继续推进。变异验证：把 `Version.eq(expected)` 那道 filter 删掉，
>   测试立刻在第 660 行失败并抖出被静默覆盖的行——断言真的在守这件事。
> - **OpenAPI 快照**随之更新（8 个 schema 各多一个 `version` 字段），
>   `docs/openapi.json` 已重新落盘并由 `openapi_snapshot` 三个用例守住。

> **§7.9 补记（2026-10-19，五档配置优先级）**：§7.9 那张优先级链此前只落实了两档
> （读一个 YAML + 展开 `${VAR}`），现在是一条完整的链：**代码默认值 < `config/default.yaml`
> < `config/{profile}.yaml`（`CICADA_PROFILE` 选文件）< `config/local.yaml`（gitignore）
> < `CICADA_*` 环境变量**，实现在 `crates/infra/src/config.rs`。
>
> - **做法是「合并」不是「挑文件」**：把代码默认值序列化成树当底，逐层 YAML 深合并，
>   再叠环境变量，最后反序列化。数组与标量整体替换、对象递归——`cors.allowed_origins`
>   这种列表，运维要的语义从来不是「追加」。旧实现的 bug 正是「谁先存在用谁」：
>   `default.yaml` 一放下，`local.yaml` 与 profile 全成摆设。
> - **底树里的 `db.url` 被刻意抹成空串**：`DbConfig::default()` 那条是开发机地址，留着它，
>   「五档全都没配」就会静默连上本地库，而不是按 §7.9 报 `MissingDbUrl`。
>   校验只补 serde 不管的两类——越界与自相矛盾（`cors: "*" + allow_credentials` 是 §7.9 自己点名的），
>   报错消息带**字段路径**，因为启动失败时人在容器日志里。
> - **`CICADA_SERVER_PORT` 这类名字要能落到 `server.port`**：`_` 既是层级分隔符也是键名的一部分
>   （`max_upload_mb`），所以按「当前层能匹配上的最长键」贪心下降。写这条时踩到一个真 bug：
>   贪心循环里用的是 `map.get(&key)?`，最长匹配一失败就整条返回 `None`，
>   于是**所有两层以上的覆写都被静默忽略**（`CICADA_SERVER_PORT` 只留下一句 warn）。
>   改成 `continue` 退一格重试；测试两个方向都断言（合法名真的生效、非法名真的被报告），
>   把那条改回 `?` 立刻红。
> - **`DATABASE_URL` / `REDIS_URL` 单列一档**：它们比 `CICADA_DB_URL` 更早存在、也更要紧，
>   容器与 CI 都靠这两个名字。
> - **`Config` 及其全部子结构现在也实现 `Serialize`**：底树是「代码默认值序列化出来的」，
>   没有它就没有那第五档。
> - **仓库里两份样例文件被测试真加载了一遍**（`the_shipped_config_files_load_and_validate`）。
>   这条一加就发现 `default.yaml.example` 已经漂了：缺 `auth.password` 段。样例文件写错
>   要到第一次启动才暴露，那时最难查。
> - **顺带解掉一个 §12 的结**：容器的监听端口现在可以纯环境变量改
>   （`CICADA_SERVER_PORT`），不再需要烘配置文件；镜像的 HEALTHCHECK 也因此能跟着端口走。

> **§7.4 补记（2026-10-19，对象存储驱动与两条下载路径）**：§4.1 表里的 opendal 从
> `Cargo.toml` 的注释变成真依赖，§7.4 的「按配置切驱动」和「短时 URL」两条都落了地。
>
> - **驱动选择在组装根做一次**：`cicada_storage::build(&config.storage)` 按
>   `storage.driver` 返回 `Arc<dyn Storage>`，`sys_file.storage` 记的是**当时的驱动名**
>   （换驱动后存量对象仍在原桶，这个字段不能撒谎）。
>   0.59 起 opendal 把每个服务拆成独立 crate，所以引的是 `opendal` + `opendal-service-s3`
>   + `opendal-service-oss`；§4.1 写的 0.59.3 是插入号语义，`Cargo.lock` 实际解析到 0.59.5。
> - **`rustfs` 与 `s3` 共用同一个 S3 服务**——这正是选 opendal 的理由：S3 兼容协议不必为
>   每个厂商写一个驱动。两者差别只在两处：region 缺省时 rustfs 填 `auto`（自建网关不校验它），
>   以及**只有没给自定义端点时**（即真 AWS）才开虚拟主机风格；指向 rustfs/minio/R2 一律 path 风格，
>   这是那些服务能连通的前提。
> - **本地与内存驱动不绕 opendal**：它们只需要 `std::fs`，套一层 opendal 会把整条 HTTP
>   传输栈变成开发环境的运行时依赖。§7.4 要的「一套 API」是 `Storage` trait 本身——
>   业务代码（`crates/web/src/file.rs`）拿到的始终是 `Arc<dyn Storage>`，换驱动零改动。
>   三条路径共用同一份穿越校验（`../` 与绝对路径一律拒绝）与同一份错误语义
>   （缺对象 → `NotFound`，不是笼统的 `Io`）。
> - **`StorageConfig` 补齐对象存储四件套**（`bucket` / `endpoint` / `region` /
>   `access_key_id` + `access_key_secret: Secret<String>`），并在 `validate()` 里按驱动查：
>   rustfs 必须有端点（没有可推的默认地址），s3 至少给端点或 region 之一，三档都要桶名与凭证。
>   缺参在启动期就报字段路径，而不是等第一次上传拿到一个 403 被当成「存储挂了」。
>   `Config::load` 的分层机制意味着 `CICADA_STORAGE_ACCESS_KEY_SECRET` 直接可用（§7.9）。
> - **下载两条路径**（§7.4 那句括号）：`POST /system/files/{id}/download-url` 先按
>   权限点 + 数据范围鉴权，然后**问驱动能不能签名**——能（opendal `presign_read`）就回
>   桶端预签名 URL；不能（本地/内存）就签发 32 位一次性 token，由公开路由
>   `GET /api/v1/files/dl/{token}` 单次消费。凭证走 `DownloadGrantStore` 端口：
>   生产是 Redis（TTL 由 Redis 兜底，消费用「比对后删除」保证并发下只有一次成功，
>   §12 的「实例不持状态」），测试用进程内实现。
>   复核两件事才回字节：那一行没被软删、且 `path` 与签发时一致——否则链接就成了读取被替换内容的通道。
>   默认有效期 300 秒：够前端发一次下载、够用户点开一张头像，又短到「链接被复制进聊天」
>   不会变成长期通行证。
> - **§7.3 的账也顺手平了**：新增的下载凭证域（`cicada:dl:`）登记进 `CACHE_DOMAINS`；
>   同时发现第三方登录的 `oauth:state:` 前缀**一直在用却从没登记过**——按 §7.3 的规则
>   那是「不可观测也不可清理」的域，一并补上。域数量守卫（`redis_inspector` 里那条
>   「白名单域数量变化时请同步文档」）与 §7.3 正文的域清单同步更新。
> - **验证**：`crates/storage` 12 条单测——四档驱动都能从配置建出来且驱动名正确、
>   缺参逐条报错、opendal 这条实现路径（借内存服务）跑通读写删查 + 穿越拒绝 + `NotFound` 映射、
>   凭证「用一次即废 / 过期即失效 / 互不影响」；`m2_extra` 的端到端（真实 PG）覆盖
>   签发→匿名兑换拿到字节→重放 404→伪造 token 404→未鉴权签发 401，并断言链接里除 token
>   外不带任何内容。**没覆盖的**：对着真实桶签预签名 URL 这一支——opendal 的内存服务不实现
>   presign（返回 `Unsupported`，正是本地驱动走的那条分岔），要跑得等一个真实端点。
>   快照同步到 110 paths / 130 schemas。

> **§5.1 补记（2026-10-19，慢查询、优雅停机与连接池默认值）**：§5.1 那几条「约定」原本是散文，
> 现在三条都有实现和测试。
>
> - **慢查询阈值可配**：新增 `db.slow_query_ms`（默认 200，`0` 关闭）。这里有个反直觉的接法：
>   SeaORM 只在 `sqlx_logging` 为**真**时才应用慢语句设置，所以 `Db::connect` 写的是
>   `sqlx_logging(true)` + `sqlx_logging_level(Off)`——常规语句仍然一个字都不打
>   （§5.1「由 tracing 统一接管」没被破坏，语句照旧走 `record_stmt_in_spans` 进请求 span），
>   只有越过阈值才多一条 warn。sqlx 的日志只带语句文本与耗时，**参数值不会进日志**，
>   所以 §5.1 那句「不记录参数值」是被测出来的事实，不是许诺。
> - **优雅停机**：`server.graceful_shutdown_secs` 之前是一个「配了、校验了、但没人读」的字段。
>   现在 SIGINT 与 SIGTERM 都会触发 `with_graceful_shutdown`，排空**有上限**——到点仍有在途请求
>   就放手退出（否则一条卡住的长连接会让 `docker stop` 等满宽限期后被 SIGKILL，连退出码都不体面），
>   最后 `Db::close()` 等连接归还（§5.1 末条）。停机路径抽成
>   `serve_with_drain(listener, router, stop, grace)`：一来它能被测试（不必真发信号），
>   二来使用者的模板工程可以注入自己的停止来源。
> - **连接池默认值**：`max_connections = 0` 展开成 `CPU 核数 × 2`（§5.1 原文），代码默认值改为 `0`，
>   显式写数字仍然按数字（「PG 前置 pgbouncer 时下调」就是显式写数字的场景）。
>   `/metrics` 的 `db_pool_connections{state="max"}` 与 `Db::connect` 共用
>   `resolved_max_connections()`，否则监控看到的上限和真正生效的上限是两个数。
> - **顺带修掉镜像里的一处过时**：HEALTHCHECK 现在读 `${CICADA_SERVER_PORT:-8080}`。
>   §7.9 的环境变量覆写落地后，写死 8080 会把换了端口的部署一直报成 unhealthy。
> - **验证**：`crates/infra/tests/db.rs`（真实 PG，`--ignored`）四条——关池后 `ping` 必须失败
>   而不是悄悄重建；超阈值必有 warn 且带 SQL 摘要、日志里查不到参数哨兵值；
>   阈值放宽到 60s 时常规语句零输出。`crates/all/tests/graceful_shutdown.rs` 三条
>   ——信号完成即停、`grace` 到点强制退出（慢 handler 30s，断言 2s 内退出）、`grace=0` 不等。
>   看门狗那支代码删掉后两条断言立刻红，确认断言真的在守这件事。

> **§7.10 / §4.1 补记（2026-10-19，邮件渠道真正说 SMTP）**：选型表里「邮件 | lettre | SMTP」
> 这一行原本是空的——`lettre` 是 `Cargo.toml` 里的一句注释，email 渠道唯一的投递路径是
> 往 `config.endpoint` POST 一段 JSON。现在 `crates/notify/src/smtp.rs` 用 lettre 的
> async SMTP transport 把这条路补上。
>
> - **分岔只看运营填了哪个字段**：`email` + `host` → 直连 SMTP；只有 `endpoint` → 仍然走
>   HTTP 邮件网关（很多环境只有厂商的 HTTP 发信接口）。选择发生在 `RoutingSender`，
>   `NotifySender` 的语义与 `NotifyService` 的调用点一行没改。
> - **`ChannelConfig` 长出 SMTP 四件套**（`host` / `port` / `username` / `password` / `tls`），
>   `endpoint` 从必填变可选；「哪种渠道必须有什么」收进 `ChannelConfig::validate_for(kind)`，
>   接口的 422 与单测共用同一份规则。此前是 web 侧写死「`config.endpoint` 不能为空」，
>   照那个写法 SMTP 形状根本建不出来。
> - **三种配置形状在建渠道那一刻就被拦住**：远端 + `tls = none`（明文只允许回环 relay，
>   否则口令和正文一起在网络上裸奔）、`username` 与 `password` 只填一半、
>   `email` 既没 `host` 也没 `endpoint`。错误消息点名**字段名**——渠道配置是一坨 JSON，
>   不说字段名等于没说。
> - **口令支持 `${SMTP_PASSWORD}`**：复用 §7.9 的 `expand_placeholders`，投递那一刻才展开，
>   于是 `sys_notify_channel.config` 那行可以只存变量名。
> - **凭据不回显**：`ChannelVo.config` 里的 `token` / `password` 掩成 `***`，列表与详情走
>   同一份转换。有 `system:notify:list` 的人已经能改渠道，回显口令把「能改」放大成「能拿走」。
>   审计侧也核过：`sys_oper_log.oper_param` 记的是查询串而不是请求体，口令不会从审计里漏出去。
> - **端口与状态码**：`ssl` → 465、`starttls` → 587、`none` → 25；SMTP 没有 HTTP 状态码，
>   投递被接受时服务端回的是 `250`，`sys_notify_record` 就记 250，不借用一个 200。
> - **TLS 选型与门禁一致**：lettre 显式选 `aws-lc-rs`（与 reqwest 同一个 rustls provider，
>   避免出现两个默认加密后端），`cargo tree -i native-tls` 无匹配——§3.3「只用 rustls」
>   在新依赖上仍然成立。`deny.toml` 为此点名放行 `0BSD`（lettre 的 quoted-printable 编码器），
>   是逐条放行而不是关规则。
> - **验证**：`crates/notify/src/smtp.rs` 5 条——tls→端口映射、缺 host / 缺 from 的报错形状、
>   借 lettre 的 `AsyncStubTransport` 验真实组码（信封用的是纯地址、中文主题按 RFC 2047
>   编码成 `=?utf-8?b?…?=`、正文进报文、`payload` 不混进正文）、地址非法算 `Config` 而不是
>   `Transport`、渠道校验的边界；`m4_features.rs` 的
>   `smtp_email_channel_is_accepted_and_its_secret_is_masked` 走完整 HTTP 面（半配置 422 且
>   点名 `password`、完整 SMTP 201、列表里搜不到口令原文）。
> - **顺手修掉两处测试竞态**（都是本轮新增数据量暴露的，不是产品缺陷）：
>   `apalis.jobs` 是全测试台共享的一张表，而新建 schema 的自增 id 从 1 重新开始——上一轮
>   遗留的队列行会被这一轮的消费者取走，写出「同一个 job_id 两条日志」，把
>   `集群不重复执行` 那条断言变成掷硬币；现在 `setup()` 先按命名空间清空。
>   缓存观测那条只看 `SCAN` 的第一页，共享 Redis 上 authz 域攒够一百多个键后就会漏掉
>   自己刚写的键；改成跟着游标翻完，并逐页断言「不许出现域外键」——比原来的断言更强。

> **§7.3 补记（2026-10-19，缓存策略与四类缓存对象）**：这一节此前只有权限集合落了地，
> 其余三条防护（单飞 / TTL 抖动 / 空结果短 TTL）和另外四类缓存对象都还是散文；
> `refresh-cache` 清甚至是一个**没有写入者的域**——接口能调、返回 200、什么也不发生。
>
> - **策略层落在 `cicada-ports::kv`**（`KvCache` + `KeyValueStore`）而不是 Redis 句柄里：
>   单飞、抖动、`EMPTY_MARKER`、版本号这些是**行为**，与后面接的是 Redis 还是测试夹具无关。
>   放在端口这一侧的直接收益是全部可单测：`crates/ports/src/kv.rs` 里 5 条断言——
>   并发未命中只回源一次、空结果只值 30 秒、脏值不污染调用方、TTL 抖动不越 ±10%、
>   版本自增后旧键不可达。`cicada_infra::Cache` 只新增一个 `KeyValueStore` 实现，
>   把「Redis 抖动」翻译成「没命中」，策略层因此不需要错误分支（§7.3 的语义：缓存是纯优化）。
> - **键一律 `cicada:{域}:{租户|global}:{标识}:v{版本}`，且版本号键放在同一个域前缀之内**。
>   后者是这轮的一个真 bug 苗头：`/system/configs/refresh-cache` 做的是整域前缀清空，
>   如果版本号键在域外，清完域会把值清掉、版本却留在原地，下一次读又用旧版本号
>   命中残留——现在整域清空把值和版本一起清掉，回到干净起点。
> - **四类缓存对象按各自的真实读路径落地**：
>   ① **字典项**：`GET /system/dict/data/by-type/{type}`（全站最热的一次读，每个下拉框都要）
>   进缓存，六条字典写路径（类型与数据的增删改）各加一次 `Dict` 域版本自增。
>   ② **菜单树**：`GET /auth/routers` 按「角色集合」缓存，键用排序去重后的角色 id
>   （`roles_ident`）——不排序的话同一组角色换个顺序会在缓存里出现两份等价副本，命中率掉一半
>   而没人看得出为什么。菜单与角色的四条 + 六条写路径改为走
>   `invalidate_grant_caches`：权限集合与菜单树一起失效，避免只记得其一。
>   ③ **权限集合**：见下条的两级。④ **租户信息**：`rate_limit.tenant_cache_ttl_secs`
>   早就是为它配的（限流器里的进程内 TTL 缓存，每请求不查库），这一条本来就在，
>   没有再叠一层 Redis 缓存——同一份事实给两个身份只会让「改了多久生效」说不清。
>   ⑤ **参数**：`sys_config` 在本设计的接口清单里**没有按 key 取值的路径**（只有管理台的
>   分页/详情/导出），所以这轮不为「有缓存可写」去造一个接口。`Config` 域前缀与整域失效
>   的通道都是通的，真要加按 key 读时直接走 `cached_json` 即可。这是**记录下来的偏差**，
>   不是漏掉的一步。
> - **两级（§7.3 第一条）在权限集合上成形**：`AuthzCache` 前面加了一层 moka
>   （4096 条 / 5 秒），但**版本号仍然每次都读 Redis**。这一条是刻意的取舍：
>   如果连版本号也缓存在进程内，加一层 L1 就等于把「角色改了立刻失效」换成
>   「最多晚 N 秒」——那是安全属性换性能。现在 L1 只省掉取权限正文那一次往返与它的
>   反序列化，越权窗口为零，正是 §15 第 5 条里「版本号失效」那条路线在进程内的样子。
>   `invalidate_tenant` 顺手 `invalidate_all()` 清本实例的 L1（内存卫生，不是正确性依据）。
> - **新增的三个域都登记进了 `CACHE_DOMAINS`**（`dict` / `menu` / `tenant`，
>   连同原有的 `config`），守那条「白名单域数量变化时请同步文档」的计数断言同步到 15，
>   本节上面的域清单也一并更新。没登记的键空间在缓存监控里既看不见也清不掉（失败关闭）。
> - **验证**：`crates/system/src/cache.rs` 5 条（键与域前缀对得上、角色 ident 与顺序无关、
>   读一次就命中缓存、版本自增强制重新回源、脏值解析失败时清掉并回源）；
>   `crates/rbac` 4 条新增（L1 命中不再取正文、版本跳了 L1 立刻不可达、并发未命中只算一次、
>   单飞表用完即清）；`crates/web/tests/system_base.rs` 的
>   `dict_reads_are_cached_and_writes_invalidate_by_version` 端到端跑完整形状：
>   读一次 → **用裸 SQL 绕过接口删行** → 再读仍是老数据（证明读的确实是缓存而不是又查了库）
>   → 走接口写一条 → 立刻看到新数据、旧键仍在但不可达。
> - **§15 第 5 条仍然开着**，但已经把「版本号失效」这一路的实现铺好了：跨实例的
>   主动广播失效如果哪天要，只需要在 `bump_domain_version` 之后多做一次发布，
>   不必改动任何一个读路径。

> **§6.1 / §9.5 补记（2026-10-19，JWT 走 EdDSA 并支持 kid 轮换）**：§4.1 的「EdDSA(Ed25519) 优先，
> 支持 HS256 回退」、§6.1 的「支持 `kid` 多密钥轮换」与 §7.9 样例里的
> `auth.jwt: { alg, private_key, keys }` 此前都没有落地——配置里是一个平铺的 `auth.jwt_secret`，
> 代码里只有 HS256。现在 `auth.jwt` 是真的配置段，`TokenService` 是真的多密钥钥匙环。
>
> - **形状**：`{ alg: EdDSA|HS256, kid, private_key, public_key, keys: [{ kid, alg?, key }] }`。
>   EdDSA 必须同时给公钥——私钥不能验签，而 §16.2 的「把 auth 拆出去、别的服务本地验签」
>   要的就是那份公钥。缺公钥时构造 `TokenService` 直接失败（装配期 fail fast），
>   不留「签得出、验不了」这种要在第一个请求才暴露的部署。
> - **轮换语义**：签名只用当前 `kid`，验签按 JWT 头里的 `kid` + `alg` 在钥匙环里挑。
>   于是换密钥时旧 token 在自身有效期内仍可验（`keys` 里留着旧公钥），
>   把旧 kid 摘掉就是「轮换窗口结束」的显式动作，而不是重启时把所有人踢下线。
>   先 `decode_header` 再选密钥是刻意的：由头决定用哪套校验，避免「固定算法再解码」
>   给 alg 混淆攻击留门；HS256 的 token 拿着 EdDSA 的钥匙环必须验不过（有测试）。
> - **§9.5 的弱密钥拒绝挂在 profile 上**：`CICADA_PROFILE=prod` 时启动前额外检查
>   `enforce_production_secrets`——仍是仓库示例值、或 HS256 密钥短于 32 字符，直接拒绝起来。
>   为什么带条件：开发默认与容器冒烟跑的就是示例值，把它们一起拦掉只会让人绕过 profile
>   而不是去换密钥；而「形状对不对」（算法认不认识、kid 是否唯一、EdDSA 有没有公钥）
>   与「这台机器算不算生产」是两件事，分别留在 `validate()` 与这个函数里。
> - **验证**：`crates/auth/src/token.rs` 三条新测试——EdDSA 往返且头里带 kid、
>   轮换后新旧 kid 各自的表现（含「不再列出的 kid 当场失效」）、跨密钥/跨算法拒绝与
>   坏 PEM / 未知算法报 `Config` 而不是 `InvalidToken`；`crates/infra` 两条——
>   `auth.jwt` 解析与形状校验（EdDSA 缺公钥、kid 重复、算法不认识各拦一次）、
>   生产策略（示例值、短密钥、合格密钥放行）。

> **§7.2 补记（2026-10-19，UA、业务码与脱敏后的请求参数真正进审计）**：本节要求「自动记录：
> 操作人、租户、IP、UA、脱敏后的请求参数、耗时、结果、业务码」，其中 **UA、请求参数、业务码**
> 三项此前只落在 `sys_oper_log` 的文档里——表没有对应列，中间件也没采。现在三列都有，
> 而且是由审计层真正采到的。
>
> - **表**：`migrations/src/m20261019_000017_oper_log_context.rs` 加
>   `user_agent varchar(512)` / `biz_code varchar(40)` / `req_body varchar(4096)`，
>   外加 `biz_code` 的部分索引（按码捞失败记录是这张表最主要的读法）。字段沿
>   `OperLogEvent` → `outbox_event.payload` → `SysLogStore` 一路带到属主表，事件结构体上的
>   新字段一律 `#[serde(default)]`：发件箱里可能留着改版前的行，不能因为一次加列就让 relay
>   解不动而卡死。
> - **业务码从响应里取，而不是让 handler 报告**：`Problem::into_response` 顺手把码写进响应头
>   `x-cicada-biz-code`，审计中间件在响应回来时读它。之所以不在各处调用点手工传给 sink：
>   错误在 handler 内部成形，让每个写接口自己记得带一份码，等于把「漏填」变成常态。
>   成功响应没有这个头，因此 `biz_code` 只在失败行上有值——它描述的是这次失败。
> - **请求体采集有两条路，共同点是一个：都不改变 handler 收到的字节**（`crates/web/src/audit.rs`）。
>   声明了 `Content-Length` 且 ≤ 8 KB 的走缓冲——读进内存再把同一份字节交回请求，于是权限守卫
>   短路、handler 压根没碰体的那些被拒请求也留下了参数（审计最想看的正是这类尝试）；
>   没声明长度（chunked）或声明超限的走**旁路记录器**——与原流共用一个 `map`，每块原样转发、
>   顺手攒副本。分两条是因为 Body 是一次性的流：「读出来才发现超限」和「读到一半出错」都
>   没法把字节变回去，届时 handler 收到空体——为了记日志而改变请求结果，是审计层最不该犯的错。
>   副本超过 8 KB 就整段作废（不记半份看起来像完整 JSON 的东西），列宽 4096 个字符按 `char`
>   边界截断，绝不在多字节字符中间切断。
> - **脱敏按键名子串**：键名（大小写不敏感）含 `password` / `pwd` / `secret` / `token` / `otp` /
>   `totp` / `private_key` / `access_key` / `api_key` / `authorization` / `refresh` / `jwt` /
>   `signature` / `sign` 等片段的值改成 `***`，递归进嵌套对象与数组。宁可多遮一点也不漏：
>   审计的读者是「查谁改了什么」的人，不是需要看到口令的人。
> - **读侧**：`OperLogVo` 多出这三个可空字段（旧行为 `null`），列表与详情接口照旧；
>   按 `biz_code` 捞失败记录的能力先落在列与索引上。
> - **验证**：`crates/web/src/audit.rs` 四条单测——跨块 JSON 采得到且遮得住（嵌套与数组里的凭据
>   都变 `***`，转发字节与原文逐字节相等）、超限只作废副本、带长度声明的请求在 handler 一字节
>   都不读时参数仍在、非 JSON 体不接采集器但照常送达；
>   `crates/web/tests/monitor_logs.rs::audit_records_ua_masked_body_and_biz_code` 在真实 HTTP 面上
>   跑一遍（chunked 发体 → 一次成功、一次唯一约束冲突），断言两行都带 UA、成功行的 `req_body`
>   是脱敏后的 JSON 且整页搜不到口令原文、失败行 `biz_code = CICADA_SYS_3002` 而成功行为空。
>   OpenAPI 快照因此新增三个可空字段（`CICADA_OPENAPI_BLESS=1` 重刷，diff 只含这三处）。

> **§9.3 补记（2026-10-19，富文本入库前的白名单清洗）**：本节的「公式注入清洗」早已在
> `crates/web/src/import.rs` 落了地，但「富文本内容存储前用白名单清洗（`ammonia` 级别）」
> 此前只有一句要求——`sys_notice.notice_content` 是原样进库的，接口层没有任何清洗。
>
> - **落点**：`crates/system/src/rich_text.rs`（`sys_notice` 的属主是 `system`，清洗规则跟着
>   表走，而不是跟着某个 handler 走）。白名单只放排版与图文混排要用的标签
>   （段落/列表/标题/引用/代码/表格/`a`/`img`）与属性（`href`/`src`/`alt`/`title`/
>   `colspan`/`rowspan`/`start`/`width`/`height`）；`style`、`class`、`on*` 都不在列——
>   样式和脚本不是内容。协议只认 `http`/`https`/`mailto`：属性名合法不等于值合法，
>   `javascript:` 与 `data:` 是富文本 XSS 的主通道，必须单独一层。
> - **接法**：公告的创建与更新两条写路径都过 `sanitize_html`（漏一条就等于没做）。
>   读接口与 `sys_notice_read` 不涉及正文改写，因此不再处理——存的是什么就取什么，
>   「入库前清洗」的意义正是不依赖渲染端记得转义。
> - **依赖**：`ammonia 4.2.4` 带来 servo 系 HTML 解析器（`html5ever`/`cssparser`/`selectors`），
>   许可是 MPL-2.0，已按本仓「逐条写明理由」的做法加进 `deny.toml` 的许可证白名单
>   （文件级弱 copyleft：不改那些文件就没有开源义务）。TLS 仍是 rustls，没有引入 native-tls。
> - **验证**：`crates/system/src/rich_text.rs` 四条单测（脚本/内联样式/事件属性被去掉、
>   `javascript:` 与 `data:` URL 被去掉、排版标签与 http 链接保留、纯文本原样）；
>   `crates/web/tests/system_base.rs::notice_content_is_sanitized_before_storage` 从 HTTP 面写入
>   脏正文、再读回详情断言落库形状，创建与更新两条路都测。把 handler 里的 `sanitize_html`
>   换成原样写入，这条 e2e 会失败（已验证），证明它拦的是「漏接调用点」而不是函数本身。

> **§16.3 / §5.5 补记（2026-10-21，升级通道真的三份文档 + 授权服务器表的取舍）**：
> §16.3 机制 6 写的是「semver + CHANGELOG + 迁移指南」，模式 C 那行还额外要求
> 「按 CHANGELOG 手工合并……必须可行**且有文档**」。此前两者都没有：仓库里既没有
> `CHANGELOG.md`，也没有一份告诉使用者「版本变了你要做什么」的文档。现在补齐：
>
> - `CHANGELOG.md`（仓库根）：0.1.0 的 Breaking / Added / Changed / Fixed / 依赖与许可。
>   Breaking 的判定口径写在文件头——**不改代码、不改配置、不跑迁移就能升上去的不算破坏**。
>   每条都带使用者要做的那个动作，不是变更清单的复读。
> - `docs/UPGRADE.md`：升级四步（读 Breaking → 备份 + `cicada-cli migrate` → 配置键 → 门禁）、
>   迁移纪律（append-only、序号只增不补、只有一个 `Migrator` 与一张 `seaql_migrations`）、
>   `[patch]` 逃生口的用法与它的债、模式 C 的目录边界。**其中一条必须写清楚**：
>   `000003`（TOTP 两列）已并入 `000001`，老库里若还留着 `m20261009_000003_totp` 这一行，
>   SeaORM 会因「迁移列表里没有它」而拒绝跑 `up`，处理是删那**一行记录**而不是动数据
>   （列现在由 `000001` 建）。这类历史坑只有落在文档里才不会每次都靠口口相传。
> - `README.md` 的 CI 段与 `template/README.md` 的升级段跟着改准确：README 还写着
>   「依赖断言待许可证决策（§15 #9）后开启」，而 `cargo deny` / `cargo machete` /
>   分层检查 / OpenAPI 快照 / 镜像冒烟四个 job 早已在 `.github/workflows/ci.yml` 里跑。
> - **`sys_oauth_client` / `sys_oauth_token` 不建**（§5.5 的 P2 清单、附录 F）：
>   两张表描述的是「cicada 当授权服务器」（对外发 access/refresh token），而全文没有任何
>   对应的接口约定——§6.1 的 `/auth/*` 全是**登录**方向（cicada 当 OAuth 客户端，ADR 0007），
>   机器凭证走 `sys_api_key` + `X-API-Key`（ADR 0003）。§14 把「权限模型分裂」列为风险并
>   要求「只保留一套主选方案」，同时存在 API Key 与 OAuth 客户端两套对外凭证模型正是它。
>   真要对外发 token，按 M5 的纪律另立 ADR，一并定接口、同意授权与撤销语义；
>   M5 那行与 §5.5、附录 F 都改成了这个口径，不再让 `✅` 覆盖一个只做了一半的方向。

> **§9.4 补记（2026-10-21，写接口的引用归属校验真的拦住了跨租户挂载）**：本节要求
> 「所有按 ID 取数的方法强制带租户条件，并在返回前做归属校验」。读侧一直是带租户过滤的，
> 但**写侧的引用没人管**：`POST/PUT /system/users` 的 `role_ids`、`dept_id`，
> `PUT /system/roles/{id}` 的 `menu_ids`、`dept_ids`（自定义数据范围）都是调用方给的裸 id，
> 直接进 `sys_user_role` / `sys_role_menu` / `sys_role_dept`。后果不是当场提权——授权装载
> 确实按租户过滤，别的租户的角色解析不出权限——而是库里留下一条跨租户的授权引用：
> 那行以后被改归属、被回收再分配，权限就凭空出现。附录 C 的 `CICADA_RBAC_2002`
> （租户不匹配）因此从来没有出口。
>
> - **落点**：`crates/web/src/system/ownership.rs` 三个判定（`assert_roles` /
>   `assert_depts` / `assert_menus`），接在既有写路径的三个汇聚函数里
>   （`replace_user_roles`、`apply_role_depts`、`replace_role_menus`）——调用点分散在
>   创建、更新、批量导入与角色子接口共 8 处，校验放在汇聚点才不会漏。
>   `replace_user_roles` 多了一个 `tenant` 参数，就是为了把导入那条路也关进来。
> - **为什么是 403 而不是 404**：调用方确实提交了这么一行，答案不是「不存在」而是
>   「不是你的」；也不是 2001「缺权限标识」——那种情况的补救是去要权限，这里的补救是
>   改掉请求体里的 id。三种码对客户端是三件事，混成一个就没人知道该改哪。
>   `sys_menu` 是全局字典（无 `tenant_id`），因此那里退化成存在性校验：
>   挂一个不存在的菜单，等于授出一条永远解析不出权限点的引用。
> - **验证**：`crates/web/tests/m4_features.rs::cross_tenant_references_are_rejected`
>   直接往库里插 `tenant_id = 2` 的角色与部门，再从 HTTP 面提交：跨租户角色、跨租户部门、
>   角色的跨租户数据范围、悬空菜单引用四条都断言 403 + `CICADA_RBAC_2002`，
>   最后一条正例断言本租户的角色与部门照常能挂（拦住的是跨租户，不是所有引用）。
>   变异验证：把 `assert_roles` 那一行去掉，同一场测试立刻 201 并把 `role_ids:[2]`
>   写进租户 1 的用户（已复现后还原）。
> - **附录 C 另两码的诚实状态**：`CICADA_RBAC_2003`（数据范围兜底）**仍无出口**——
>   越权读在实现上是「按 ID 取数带租户/范围条件 → 查不到 → 404」，这是有意的：
>   用 403 回答「这行存在但不归你」等于给探测者一个存在性预言机。
>   `CICADA_VALIDATION_4003`（排序字段不在白名单）同样无出口，因为 §8 的接口规范表
>   明写「名单外的列名**忽略而不是报错**」——列表页多传一个字段最该发生的是照常返回数据。
>   两条都留在目录里（业务码一经发布不复用，见 §7.1），但不再假装它们有行为。

---

## 14. 风险与对策

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| SeaORM 动态查询表达力不足（复杂报表） | 开发受阻 | 明确「CRUD 用实体 API、复杂查询用 sea_query」，必要时用 `DatabaseConnection` 拿 sqlx 逃生；在 M0 就写一个复杂查询样例验证 |
| `sea-orm` 的 `rbac` feature 与 `sys_*` 表结构不兼容 | 方案返工 | 作为可选增强评估，不作为主选；半天 spike，结果写入 ADR |
| 代码生成器成本被低估 | 进度失控 | 排在 M3；先做单表闭环，再扩树表/主子表；模板与逻辑分离，允许项目覆写 |
| 编译时间随 crate 增多恶化 | 开发体验差 | 保持 crate 边界与 feature 最小化；sccache + nextest + 合理拆分；避免为「整洁」过度拆包 |
| 既有数据/前端无法直接沿用 | 迁移受阻 | 只沿用数据与权限标识的命名；迁移只从空库建立 schema，前端按 cicada 规范重写（2.2、10） |
| 权限模型分裂（自研 vs 第三方） | 维护成本翻倍 | 只保留一套主选方案；第三方组件必须能映射到主选模型才引入 |
| 多租户剪不断理还乱 | 后期返工 | M1 就把 `tenant_id` 贯穿到实体、缓存键与测试；宁可先做成单租户但结构预留 |
| 审计日志写入拖慢写接口 | 性能劣化 | 审计异步化 + 失败降级；分区表 + 定期归档 |

---

## 15. 待决策项

以下条目已有倾向性建议，但需要项目所有者确认后写入 ADR：

> 已决（2026-10-08）：**#2 数据库隔离形态**采用「单库单 schema + `tenant_id` 列」；
> **#4 主键策略**采用「系统表 `BIGINT IDENTITY` + 对外资源加 `uuid` 列」，一期不做多写多活、不引入分布式 ID。
> 其余条目仍待确认。

1. **多租户默认开关**：建议 `tenant.enabled = true` 但默认单租户（`tenant_id = 1`），结构完整预留。
2. **数据库隔离形态**：单库单 schema + `tenant_id`（**已决**）还是按租户 schema。
3. **PostgreSQL 生产形态**：自建（与二进制同机或独立机）还是托管云数据库（RDS）。
4. **主键策略**：系统表 `BIGINT IDENTITY` + 对外资源加 `uuid` 列（**已决**）；全局分布式 ID 生成器（雪花）仅在多写多活时再评估。
5. **缓存一致性边界**：Redis 与本地 moka 的失效传播采用「版本号失效」（建议）还是「主动广播失效」。
6. **MQ 引入时机**：outbox relay 先用进程内/Redis 队列（建议），还是直接引入 RabbitMQ。
7. **工作流**：是否需要在一期提供 Flowable/Camunda 的适配层，还是完全交给业务方。
8. **审计日志写入方式**：异步入库 + 失败降级（建议）还是与业务事务强一致。
9. **许可证**：cicada 采用 MIT / Apache-2.0 双许可（建议，与 Rust 生态惯例一致）还是闭源。
10. **模板工程形态**（见 16.3）：以 `cargo-generate` 模板作为主推上手方式（建议），还是只提供 crate 让使用者自己搭。
11. **outbox 启用范围**：建议仅审计、通知、租户配额这类跨模块副作用启用，其余模块保持简单直写。
    （2026-10-16 落地：审计已走 outbox，见 §7.2 补记与 ADR 0008；通知与配额仍是直写，等出现第二个
    跨模块副作用再迁——为单一实现造端口违反 §3.3 规则 7。）

---

## 16. 可扩展性、可拆分性与模板化二次开发

本章是对前面设计的自我审查，结论直接决定了 3.2、3.3、3.4、5.3、6.3、7.2、7.7 与 13 的修订。
使用的判据来自 deep module 的设计词汇：**模块**（有接口与实现的东西）、**接口**（调用方必须知道的一切，不只类型签名）、**缝**（接口所在的位置）、**adapter**（在缝上满足接口的具体物）、**深度**（少量接口换取大量行为）、**局部性**（改动与知识集中在一处）。

### 16.0 三问结论

| 问题 | 初版设计评级 | 缺口 | 修订位置 |
| --- | --- | --- | --- |
| 方便扩展吗 | 基本可用 | 无模块注册契约；三处接口偏浅；crate 拆得过碎 | 16.1、3.2、3.3、3.4、5.3、6.3、7.7 |
| 方便单体→微服务吗 | **不够** | 模块共享 `Db` 与事务、允许跨模块 JOIN；缺 port/adapter、outbox、数据范围的跨服务方案 | 16.2、3.3、7.2 |
| 方便模板二次开发吗 | **不够** | 缺 cargo-generate 模板、缺业务代码与框架的物理分离、缺单一入口 crate 与升级策略 | 16.3、3.2 |

> 已裁决（2026-10-08）：三问的最终结论见 [`design-it-twice/FINAL.md`](superpowers/design-it-twice/FINAL.md)。
> 模块注册契约采用「Manifest + Slot 逃生通道」（common-path 打底、吸收 flexible 的注册点一等化）；
> 数据范围接口采用 typestate 方案（取代 §6.3 的 `Scope::apply`）。

### 16.1 模块深度评估与三处深化

**已经是深模块的（保留）**：`kernel` 的错误与值对象、SeaORM 之上的 `Db`（连接/事务/软删/分页全部藏在一个句柄后）、opendal 之上的 `Storage`、代码生成器。这些的接口小而背后的行为多，符合深模块定义。

**三处偏浅的接口，已修订**：

1. **`RequestContext` 贯穿到持久层**（原 6.3）。这是个浅接口：调用方要学会 `request_id`、`trace_id`、`user_agent` 等一堆与取数无关的东西，DB 测试还得构造假请求。改为窄接口 `Scope { tenant_id, user_id, data_scope }`，在 service 边界由 `RequestContext` 派生。接口变小，测试面变窄，持久层也不再认识 HTTP。
2. **`DataScope::apply(q, dept_expr, owner_expr)`**（原 6.3）。要求每个调用点自行知道「哪列是部门、哪列是归属人」，schema 知识散落在所有 repository 里，改动会扩散——违反局部性。改为实体上声明 `DataScoped`，作用域条件只实现一次。
3. **crate 拆得过碎**（原 3.2）。`config` / `telemetry` / `db` / `cache` / `mq` 五个 crate，每个的接口几乎等于其实现，属于「只有一个 adapter 的假缝」。合并为 `infra` 一个 crate 的内部模块。真正配得上独立 crate 的是**存在两种 adapter** 的能力（存储：本地/S3；事件总线：进程内/MQ；目录：本地库/远程服务），它们进 `ports`。同时新增 `all` 门面 crate，让「不想研究 crate 划分」的使用者用 `cicada = { features = ["full"] }` 一把接入。

**新补的扩展契约（初版最大缺口）**：没有模块注册契约，第三个领域的作者只能去改 `apps/server`，这既不叫可扩展，也不叫模板友好。二选时对比过三种路线——把一个 trait 上列死若干注册点、把注册点做成运行期登记表（`Slot<T>`）、只产出一份「事实清单」——最终采用第三种（**常见路径优先**）：模块只交出一份 `Manifest`，装配策略全部搬进组装根。契约落在 `crates/web` 而非 `kernel`——它要引用 `axum::Router`、`sea_orm_migration`、`utoipa`，而 `kernel` 是 L0、不得引入这些依赖。

```rust
// crates/web/src/module.rs
/// 模块唯一必须实现的东西：一个方法，交出清单。
/// 取 `self: Box<Self>` 以保住 `dyn Module`（框架模块与业务模块进同一个 Vec）。
pub trait Module: Send + Sync + 'static {
    fn manifest(self: Box<Self>) -> Manifest;
}

/// 模块对框架的全部声明。所有字段可缺省，只有 name 必填。
pub struct Manifest {
    pub name: ModuleName,                       // 装配键：重名检测、依赖排序、报错指认
    pub owns: Vec<TableName>,                   // 本模块拥有的表，用于推导跨模块顺序与校验种子属主
    pub routes: RoutesFn,                       // fn(AppState) -> Router<AppState>，只含自己的路由
    pub migrations: Vec<Box<dyn MigrationTrait>>,
    pub perms: Vec<Permission>,                 // 权限点，与 #[require_perm] 双向对账
    pub menus: Vec<MenuNode>,                   // 菜单节点，交给菜单属主落库，不自己插 sys_menu
    pub seeds: Vec<SeedOp>,                     // 只允许写 owns 声明的表
    pub openapi: OpenApi,
    pub after: Vec<ModuleName>,                 // 非数据原因的附加偏序
}

/// 默认路径的糖：一个声明块（代码生成器产出）即生成一个 `Module` 实现。
#[macro_export]
macro_rules! declare_module { /* name / routes / owns / migrations / perms */ }
```

组装根只做一件事：收集各模块 `Manifest` 里的路由并合并（`app = app.merge(routes)`）；`AppBuilder::with_all_enabled()` 按 feature 展开框架模块，`with_business(business::register)` 接入使用者模块，`build()` 一次完成全部校验与装配、失败即不启动。这一个契约同时支撑三件事——模板工程挂业务模块、微服务里单独把某个模块打成二进制、按 feature 裁剪不需要的模块。

设计要点：

- **声明事实，不排顺序**：模块只说「我拥有哪些表、声明哪些权限点、有哪些路由」。迁移先于种子、鉴权中间件先于业务路由由组装根的固定相位保证；模块间顺序由「所有权/种子属主」推导出的边 ∪ `after` 声明决定，必须无环。
- **权限点双向对账**：`#[require_perm]` 在编译期把权限标识写进路由元数据，装配期取「路由声明的集合」与「`Manifest.perms` 的集合」求双向差集；路由声明了却没人 seed（漏注册）、seed 了却没人用（孤儿权限点）都直接失败。这补上了 9.4 承诺的启动期校验。
- **不写别人的表**：`perms` / `menus` 交给该表属主注册的 `MenuRegistry` 端口落库，属主缺席即失败；种子越界写别人的表即失败。
- **默认路径短、逃生通道显式**：默认路径是一个 `declare_module!` 声明块；条件注册、特殊顺序等走 `.with_if(...)`、`.after([...])` 逃生通道，能力不减，只是把显式性交还给作者。
- **不引入运行期登记**：没有采用「`Slot<T>` + 运行期相位」的模型——它把顺序藏进运行期调度、槽键靠字符串（拼错只能靠启动期检查兜底），其灵活性在本项目收益不足。

### 16.2 大单体版与微服务版（共享同一套代码）

cicada 交付**两个发行版本**：大单体版与微服务版。二者共享同一套 crate 与业务代码，差异只在组合根与传输 adapter——单体用 `LocalDeptDirectory` + 进程内事件，微服务版换成 `RemoteDeptDirectory` + gRPC/MQ（12 章）。所以下面的规则不是「以后拆分时再说」，而是**现在就要守住**，否则微服务版根本做不出来。

初版设计的致命处在于：L2 模块共享同一个 `Db`、同一个事务，并且允许 `sys_user ⋈ sys_dept` 这类跨模块 JOIN。这种代码在单体内跑得很好，一旦要拆就会腐烂——JOIN 变成分布式查询，事务变成分布式事务，事件变成远程调用。要让它「拆得开」，必须现在立四条硬规则：

1. **数据所有权**：一张表只有一个属主模块。别人要看，走端口。
2. **跨模块只经端口**：`ports` crate 定义 trait，模块之间只依赖 trait，不依赖对方的 entity。
3. **不跨模块开事务**：单体里也只允许「自己的表 + 自己的 outbox」在同一事务里；跨模块副作用通过事件异步达成。
4. **端口必须有两种 adapter**：单体用本地实现，拆分后换远程实现，**切换点只有组合根一行代码**。

```rust
// cicada-ports：同一个端口，两种 adapter
#[async_trait]
pub trait DeptDirectory: Send + Sync {
    async fn descendants(&self, dept: DeptId) -> Result<Vec<DeptId>>;
    async fn ancestors_path(&self, dept: DeptId) -> Result<String>;
}

pub struct LocalDeptDirectory { db: Db }                                  // 单体：直连本模块
pub struct RemoteDeptDirectory { http: HttpClient, cache: Cache }         // 微服务：HTTP + 缓存
```

对应 deep module 的依赖分类：部门目录属于「remote but owned」——网络另一头但有所有权。这类依赖的正解就是「在缝上定义端口，生产用 HTTP/gRPC adapter，测试用内存 adapter」，逻辑因此始终留在一个深模块里，即使部署上跨了网络。

**领域事件与发件箱**：定义 `DomainEvent` 与 `EventBus` 端口，adapter 有 `LocalBus`（进程内广播，单体用）与 `MqBus`（RabbitMQ 等）。凡是「写完自己的数据还要影响别人」的地方（审计、通知、租户配额），一律走 `outbox_event` 表 + relay 任务：在同一事务里写业务数据与事件，relay 负责投递。**这条修订直接改了 7.2**——初版让 `audit` 直接写 `sys_oper_log`（别人的表），拆分会立刻暴露耦合；改为 `audit` 写自己的 outbox，由 sink adapter 决定落地本地表还是发到队列。

**数据范围跨服务怎么办**（这是拆系统时最容易翻车的地方）：`DataScope` 被刻意设计成声明式、可序列化的对象（不含闭包、不含 SQL），因此它可以随 token 传递，或作为一次远程 `DeptDirectory` 查询的参数。典型组合是「token 里携带 scope 声明 + 每个服务本地缓存部门目录」，而不是每次查询都远程问一遍。

**最不痛苦的拆分顺序**：

1. `auth` 独立——JWT 公钥本地验签，拆分几乎零成本，也是同类微服务底座的选择。
2. `monitor` / `tool` 独立——以只读和自身数据为主，依赖少。
3. `audit` / `notify` 走 outbox 异步——本来就是事件驱动。
4. `system` 留到最后，因为被依赖最多；`user` / `dept` / `role` 三者再拆开是最后一步，且必须先把 `DeptDirectory` 与 token 携带 scope 落地。

**反面做法**：按表拆（`sys_user` 一个服务、`sys_role` 一个服务）。这会把一次 JOIN 变成 N+1 次远程调用，是最常见的微服务自残方式。要按聚合与所有权拆，而不是按表拆。同时承认代价：拆分会把「一次 JOIN」换成「一次远程调用 + 缓存失效策略」，只有在独立扩缩或独立发布真的有需求时才拆。

### 16.3 模板化二次开发

参考实现的主流用法是「fork 一份，删掉不要的，加自己的业务」。形态 B 在这个维度天然吃亏，必须靠三个机制把体验补回来。

**三种使用模式，都要能走通**：

| 模式 | 做法 | 升级方式 |
| --- | --- | --- |
| A. 开箱即用 | `cargo generate cicada/template my-app`，改配置 + 加业务模块 | 改 `Cargo.toml` 的版本号 |
| B. 深度定制 | 依赖 cicada crate，实现自己的 trait adapter，`business/` 写业务 | 同上，框架只升版本 |
| C. 完全接手 | fork 全仓，但只改 `business/`、`config/`、`templates/` | 按 CHANGELOG 手工合并（不推荐，但必须可行且有文档） |

**让二次开发便利的六个机制**：

1. `cargo-generate` 模板 + `cicada new`：生成的是**使用者自己的工程**，不是让你去改框架源码。
2. 业务代码与框架物理分离：使用者只碰 `business/`、`migrations/`、`templates/`、`config/`，框架 crate 通过 git tag 依赖。这一条是「方便二次开发」和「能升级」同时成立的前提。
3. 单一入口 crate：`cicada = { git = "...", tag = "v0.1.0", features = ["full"] }`，不想研究 crate 划分的人不必研究。
4. feature 开关 + 支持矩阵：模块可裁剪（不用的 `monitor`/`codegen` 不参与编译），但只公布并测试有限组合（如 `full`、`minimal`、`with-mq`），不承诺 2^n 全组合。
5. 覆盖点清单：让使用者「改行为而不改框架代码」。可覆盖项——配置、种子数据、代码生成模板、前端主题与组件、trait adapter（存储/事件/通知/鉴权策略）、模块增删。凡是必须改框架源码才能定制的行为，都算设计缺陷。
6. 升级通道：semver + CHANGELOG + 迁移指南；数据库迁移 append-only；废弃接口给一个版本的弃用期；`[patch]` 段落作为本地临时覆写的官方逃生口。

**明确禁止的反模式**：把业务代码写进框架 crate；修改框架的既有迁移文件；在框架里写「如果租户 X 就怎样」这类业务分支。

### 16.4 对里程碑的影响

| 阶段 | 新增工作 | 增量 |
| --- | --- | --- |
| M0 | `Module` 注册契约、`Scope`、`ports` crate、`all` 门面 crate、cargo-generate 模板骨架、`infra` 合并 | +3~5 天 |
| M1/M2 | `outbox_event` 表与 relay、`DeptDirectory` 端口与本地 adapter、审计改写为 sink | +2 天 |
| M3 | 代码生成器产出的落点改为使用者工程的 `business/` | 不变 |
| M4 | 出口条件加一条：**把 `auth` 拆成独立进程跑通一次**，验证 16.2 的规则不是纸上谈兵 | +3 天 |

---

## 附录 A：参考实现机制 → cicada 机制对照

| 参考实现 | cicada | 说明 |
| --- | --- | --- |
| Spring Boot + Spring MVC | axum + tokio | 无运行期容器，装配在 `apps/server` 组合根显式完成 |
| Shiro 过滤器链 | tower Layer 栈 | 认证、鉴权、限流、审计都是可组合 Layer |
| Shiro 单一账号密码登录 | 账号 + 多身份（账号名/手机/邮箱/微信）绑定 | 一人一账号、多登录身份，见 6.1 |
| `@RequiresPermissions` | `#[require_perm("...")]` 过程宏 | 编译期写入路由元数据，装配期与 `Manifest.perms` 双向对账（16.1） |
| AOP `DataScopeAspect` | 显式 `Scope`（数据范围）参数 + PG RLS 兜底 | Rust 无反射式切面，改为类型强制传递 |
| MyBatis + `${}` 动态 SQL | SeaORM + `sea_query` 绑定表达式 | 消灭字符串拼接，杜绝注入类漏洞 |
| Thymeleaf 服务端渲染 | 无（纯 API） | 仅保留 minijinja 用于代码生成与邮件模板 |
| Quartz | tokio-cron-scheduler（触发）+ apalis / apalis-sql（队列执行） | 调用目标白名单注册，根治反序列化 RCE |
| Druid 监控台 | Prometheus 指标 + OTel trace | 不做内嵌 HTML 控制台 |
| 代码生成（Java/Vue） | 代码生成（Rust/Vue/SQL/迁移） | 生成无魔法、开箱可编译的显式代码 |
| 多数据源 AOP 注解 | `Db` 注册表 + `ConnectionTrait` 选择 | 一期仅主库，二期再加 |
| `del_flag` 软删 | `deleted_at` + SeaORM 原生软删 | 语义更强，可区分删除时间与操作者 |
| 服务/缓存监控页 | `/metrics` + 专用只读接口 | 交给监控体系而不是内嵌页面 |

## 附录 B：术语表

| 术语 | 含义 |
| --- | --- |
| 权限点 | `模块:资源:操作` 形式的权限标识，如 `system:user:add` |
| 数据范围 | 角色可见的数据边界：全部/自定义/本部门/本部门及以下/仅本人 |
| 组装根 | `apps/server`，唯一负责构造依赖图与启动流程的地方 |
| RLS | PostgreSQL 行级安全策略，作为数据权限的数据库层兜底 |
| 兜底防线 | 应用层之外的独立校验层，用于防止应用层逻辑错误导致越权 |

---

## 附录 C：错误码目录

HTTP 状态码表达结果语义；业务码放在 RFC 9457 `problem+json` 的 `code` 扩展字段里（见 7.1）。`AppError` 到业务码的映射如下。命名规则：`CICADA_<域>_<四位>`，域段与 crate 对应，码一经发布不复用。

| 业务码 | HTTP | `AppError` 变体 | 何时返回 |
| --- | --- | --- | --- |
| `CICADA_AUTH_1001` | 401 | `Unauthorized` | 未携带令牌，或令牌无效/过期 |
| `CICADA_AUTH_1002` | 422 | `Validation` | 账号或密码错误（统一码，不区分身份是否存在，避免枚举） |
| `CICADA_AUTH_1003` | 423 | `Locked` | 账号因连续失败被临时锁定（对应 `maxRetryCount`） |
| `CICADA_AUTH_1004` | 403 | `Forbidden` | 账号已停用（`status = Disabled`） |
| `CICADA_AUTH_1005` | 422 | `Validation` | 图形验证码错误或过期 |
| `CICADA_AUTH_1006` | 422 | `Validation` | TOTP 二次验证失败 |
| `CICADA_AUTH_1007` | 401 | `Unauthorized` | refresh_token 无效或已被轮换（判定重放，撤销整条会话链） |
| `CICADA_AUTH_1008` | 409 | `Conflict` | 待绑定的身份已属于其他账号 |
| `CICADA_AUTH_1009` | 409 | `Conflict` | 解绑被拒：账号须保留至少一个可登录身份 |
| `CICADA_AUTH_1010` | 502 | `Upstream` | 微信授权码换取失败（上游不可用或凭证无效） |
| `CICADA_AUTH_1011` | 401 | `Unauthorized` | 会话指纹不符（`user_agent` 变了），需重新登录（见 9.1 补记） |
| `CICADA_RBAC_2001` | 403 | `Forbidden` | 缺少权限标识 |
| `CICADA_RBAC_2002` | 403 | `Forbidden` | 租户上下文缺失或不匹配；写接口提交了别的租户的引用（角色/部门）也用它（§9.4 补记） |
| `CICADA_RBAC_2003` | 403 | `Forbidden` | 目标数据在当前数据范围之外（命中兜底防线）。**一期无出口**：越权读表现为 404，用 403 回答「存在但不归你」等于交出存在性预言机（§9.4 补记） |
| `CICADA_SYS_3001` | 404 | `NotFound` | 资源不存在 |
| `CICADA_SYS_3002` | 409 | `Conflict` | 唯一约束冲突（账号名/手机/邮箱/权限标识/编码） |
| `CICADA_SYS_3003` | 409 | `OptimisticLock` | 乐观锁冲突（`version` 不匹配） |
| `CICADA_SYS_3004` | 409 | `Conflict` | 存在关联数据，拒绝删除（部门有下级、角色被引用等） |
| `CICADA_SYS_3005` | 422 | `Validation` | 树结构非法（父节点指向自身或后代） |
| `CICADA_VALIDATION_4001` | 422 | `Validation` | 字段级参数校验失败 |
| `CICADA_VALIDATION_4002` | 400 | `Validation` | 请求体超限、JSON 深度/数组长度超限 |
| `CICADA_VALIDATION_4003` | 422 | `Validation` | 排序字段不在白名单。**一期无出口**：§8 的接口规范表定了「名单外忽略而不是报错」，本码留着备用 |
| `CICADA_VALIDATION_4004` | 422 | `Validation` | 分页超过上限 |
| `CICADA_RATE_5001` | 429 | `RateLimited` | 触发限流 |
| `CICADA_INTERNAL_9001` | 500 | `Internal` | 未预期错误（对外只给可读 `msg`，细节只进日志） |
| `CICADA_INTERNAL_9002` | 503 | `Db` | 数据库不可用 |
| `CICADA_INTERNAL_9003` | 503 | `Db` | Redis 不可用 |
| `CICADA_INTERNAL_9004` | 500 | `Internal` | 存储/对象存储、邮件等外部依赖失败 |

---

## 附录 D：权限点目录（参考实现迁移基线，80 条）

参考实现 `sys_menu` 的 85 条记录中，80 条带权限标识（18 个 C 菜单 + 62 个 F 按钮）；其余 4 个 M 目录与 1 个外链菜单（外链）无权限标识。cicada 沿用这些标识（见 2.2 的命名策略）。类型列：`C`=菜单、`F`=按钮/接口。

**系统管理（目录）**

| 权限标识 | 类型 | 名称 |
| --- | --- | --- |
| `system:user:view` | C | 用户管理 |
| `system:user:list` | F | 用户查询 |
| `system:user:add` | F | 用户新增 |
| `system:user:edit` | F | 用户修改 |
| `system:user:remove` | F | 用户删除 |
| `system:user:export` | F | 用户导出 |
| `system:user:import` | F | 用户导入 |
| `system:user:resetPwd` | F | 重置密码 |
| `system:role:view` | C | 角色管理 |
| `system:role:list` | F | 角色查询 |
| `system:role:add` | F | 角色新增 |
| `system:role:edit` | F | 角色修改 |
| `system:role:remove` | F | 角色删除 |
| `system:role:export` | F | 角色导出 |
| `system:menu:view` | C | 菜单管理 |
| `system:menu:list` | F | 菜单查询 |
| `system:menu:add` | F | 菜单新增 |
| `system:menu:edit` | F | 菜单修改 |
| `system:menu:remove` | F | 菜单删除 |
| `system:dept:view` | C | 部门管理 |
| `system:dept:list` | F | 部门查询 |
| `system:dept:add` | F | 部门新增 |
| `system:dept:edit` | F | 部门修改 |
| `system:dept:remove` | F | 部门删除 |
| `system:post:view` | C | 岗位管理 |
| `system:post:list` | F | 岗位查询 |
| `system:post:add` | F | 岗位新增 |
| `system:post:edit` | F | 岗位修改 |
| `system:post:remove` | F | 岗位删除 |
| `system:post:export` | F | 岗位导出 |
| `system:dict:view` | C | 字典管理 |
| `system:dict:list` | F | 字典查询 |
| `system:dict:add` | F | 字典新增 |
| `system:dict:edit` | F | 字典修改 |
| `system:dict:remove` | F | 字典删除 |
| `system:dict:export` | F | 字典导出 |
| `system:config:view` | C | 参数设置 |
| `system:config:list` | F | 参数查询 |
| `system:config:add` | F | 参数新增 |
| `system:config:edit` | F | 参数修改 |
| `system:config:remove` | F | 参数删除 |
| `system:config:export` | F | 参数导出 |
| `system:notice:view` | C | 通知公告 |
| `system:notice:list` | F | 公告查询 |
| `system:notice:add` | F | 公告新增 |
| `system:notice:edit` | F | 公告修改 |
| `system:notice:remove` | F | 公告删除 |

**日志管理（系统管理下的子目录）**

| 权限标识 | 类型 | 名称 |
| --- | --- | --- |
| `monitor:operlog:view` | C | 操作日志 |
| `monitor:operlog:list` | F | 操作查询 |
| `monitor:operlog:remove` | F | 操作删除 |
| `monitor:operlog:detail` | F | 详细信息 |
| `monitor:operlog:export` | F | 日志导出 |
| `monitor:logininfor:view` | C | 登录日志 |
| `monitor:logininfor:list` | F | 登录查询 |
| `monitor:logininfor:remove` | F | 登录删除 |
| `monitor:logininfor:export` | F | 日志导出 |
| `monitor:logininfor:unlock` | F | 账户解锁 |

**系统监控（目录）**

| 权限标识 | 类型 | 名称 |
| --- | --- | --- |
| `monitor:online:view` | C | 在线用户 |
| `monitor:online:list` | F | 在线查询 |
| `monitor:online:forceLogout` | F | 单条强退 |
| `monitor:online:batchForceLogout` | F | 批量强退 |
| `monitor:job:view` | C | 定时任务 |
| `monitor:job:list` | F | 任务查询 |
| `monitor:job:add` | F | 任务新增 |
| `monitor:job:edit` | F | 任务修改 |
| `monitor:job:remove` | F | 任务删除 |
| `monitor:job:changeStatus` | F | 状态修改 |
| `monitor:job:detail` | F | 任务详细 |
| `monitor:job:export` | F | 任务导出 |
| `monitor:data:view` | C | 数据监控 |
| `monitor:server:view` | C | 服务监控 |
| `monitor:cache:view` | C | 缓存监控 |

**系统工具（目录）**

| 权限标识 | 类型 | 名称 |
| --- | --- | --- |
| `tool:build:view` | C | 表单构建 |
| `tool:gen:view` | C | 代码生成 |
| `tool:gen:list` | F | 生成查询 |
| `tool:gen:edit` | F | 生成修改 |
| `tool:gen:remove` | F | 生成删除 |
| `tool:gen:preview` | F | 预览代码 |
| `tool:gen:code` | F | 生成代码 |
| `tool:swagger:view` | C | 系统接口 |

计数核对：C 18 + F 62 = 80。任何新增业务模块的权限点都追加在自己的模块命名空间下（如 `business:product:add`），不与上表冲突。

cicada 已登记的扩展权限点（不计入上面 80 条）：

| 权限标识 | 类型 | 名称 |
| --- | --- | --- |
| `system:file:view` / `system:file:add` / `system:file:list` / `system:file:remove` | C / F | 文件存储（§7.4，参考实现无对应权限点） |
| `monitor:cache:list` | F | 缓存观测（§7.8） |
| `monitor:cache:clear` | F | 缓存按域清理（§7.8，破坏性，带审计） |
| `monitor:server:list` | F | 服务信息读取（§7.8；`monitor:server:view` 只管菜单可见性） |
| `monitor:data:list` | F | 数据监控只读统计（§7.8；`monitor:data:view` 只管菜单可见性） |
| `system:export:view` / `system:export:add` / `system:export:list` | C / F | 异步导出任务（§7.5） |
| `monitor:auditlog:view` / `monitor:auditlog:list` | C / F | 字段级变更留痕只读面（§7.2，P1） |
| `monitor:online:history` | F | 会话历史查询，读 `sys_user_online` 快照（§6.1 补记；`monitor:online:list` 仍是读 Redis 的实时在线列表） |
| `system:apikey:view` / `system:apikey:list` / `add` / `edit` / `remove` | C / F | 开放接口凭证（§7.10 / ADR 0003） |
| `system:tenant:view` / `list` / `add` / `edit` / `remove`、`system:package:view` / `list` / `add` / `edit` / `remove` | C / F | 平台管理与租户套餐（§6.4 / ADR 0004） |
| `notify:channel:*` / `notify:template:*` / `notify:record:list` / `notify:send` | C / F | 通知中心（§7.11 / ADR 0002） |

> **补记（2026-10-16）**：这张表此前只是「文档上登记了」——`migrations/src/seed.rs` 里没有对应的
> `sys_menu` 行，于是 `monitor:server:list`、`monitor:cache:*`、`system:file:*`、`system:export:*`
> 这 8 个权限点**只有 admin 的 `*:*:*` 能命中**，其它角色无论怎么勾角色树都拿不到。
> 现在种子已补齐（新增 C 菜单 `203 文件管理` / `204 导出任务` 与 8 个 F 按钮），
> 并且 `MENU_SEED` 提成了 `cicada-migrations` 的公开常量，`crates/web/tests/perm_coverage.rs`
> 加了双向对账：路由声明的权限点必须有 F 类菜单行（否则无法授予），
> 菜单里的按钮必须被某个模块清单声明（否则是死按钮）。§16.1 的权限点对账只覆盖
> 「路由 ↔ 模块清单」这一层，数据库那一层过去没人查，这就是漏口。

---

## 附录 E：系统模块接口 ↔ 权限标识对照

约定：参考实现里 `:view` 用于服务端页面渲染（`GET` 返回 HTML），cicada 是纯 API，因此 `:view` 退化为**菜单/路由可见性**标记——由 `GET /auth/routers` 下发，不再单独守卫 REST 接口；列表/详情等读取统一用 `:list`。写接口一律带权限点，未声明权限的写接口在装配期报错（9.4）。用户模块见正文 8，其余如下。

**角色（`system:role:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/system/roles` | `system:role:list` |
| GET | `/api/v1/system/roles/{id}` | `system:role:list` |
| POST | `/api/v1/system/roles` | `system:role:add` |
| PUT | `/api/v1/system/roles/{id}` | `system:role:edit` |
| PUT | `/api/v1/system/roles/{id}/status` | `system:role:edit` |
| PUT | `/api/v1/system/roles/{id}/data-scope` | `system:role:edit` |
| GET | `/api/v1/system/roles/{id}/users` | `system:role:list` |
| PUT | `/api/v1/system/roles/{id}/users` | `system:role:edit` |
| GET | `/api/v1/system/roles/export` | `system:role:export` |
| DELETE | `/api/v1/system/roles/{id}` | `system:role:remove` |

**菜单（`system:menu:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/system/menus/tree` | `system:menu:list` |
| GET | `/api/v1/system/menus/{id}` | `system:menu:list` |
| POST | `/api/v1/system/menus` | `system:menu:add` |
| PUT | `/api/v1/system/menus/{id}` | `system:menu:edit` |
| PUT | `/api/v1/system/menus/{id}/sort` | `system:menu:edit` |
| GET | `/api/v1/system/menus/role-tree/{roleId}` | `system:menu:list` |
| DELETE | `/api/v1/system/menus/{id}` | `system:menu:remove` |

**部门（`system:dept:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/system/depts/tree` | `system:dept:list` |
| POST | `/api/v1/system/depts` | `system:dept:add` |
| PUT | `/api/v1/system/depts/{id}` | `system:dept:edit` |
| PUT | `/api/v1/system/depts/{id}/sort` | `system:dept:edit` |
| DELETE | `/api/v1/system/depts/{id}` | `system:dept:remove` |

**岗位（`system:post:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/system/posts` | `system:post:list` |
| POST | `/api/v1/system/posts` | `system:post:add` |
| PUT | `/api/v1/system/posts/{id}` | `system:post:edit` |
| DELETE | `/api/v1/system/posts/{id}` | `system:post:remove` |
| GET | `/api/v1/system/posts/export` | `system:post:export` |

**字典（类型与数据共用 `system:dict:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/system/dict/types`、`/dict/data?dict_type=…` | `system:dict:list` |
| POST | `/api/v1/system/dict/types`、`/dict/data` | `system:dict:add` |
| PUT | `/api/v1/system/dict/types/{dict_id}`、`/dict/data/{dict_code}` | `system:dict:edit` |
| DELETE | `/api/v1/system/dict/types/{dict_id}`、`/dict/data/{dict_code}` | `system:dict:remove` |
| GET | `/api/v1/system/dict/types/export`、`/dict/data/export` | `system:dict:export` |

**参数（`system:config:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/system/configs` | `system:config:list` |
| POST | `/api/v1/system/configs` | `system:config:add` |
| PUT | `/api/v1/system/configs/{id}` | `system:config:edit` |
| DELETE | `/api/v1/system/configs/{id}` | `system:config:remove` |
| POST | `/api/v1/system/configs/refresh-cache` | `system:config:remove` |
| GET | `/api/v1/system/configs/export` | `system:config:export` |

**公告（`system:notice:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/system/notices` | `system:notice:list` |
| GET | `/api/v1/system/notices/{id}/readers` | `system:notice:list` |
| POST | `/api/v1/system/notices` | `system:notice:add` |
| PUT | `/api/v1/system/notices/{id}` | `system:notice:edit` |
| DELETE | `/api/v1/system/notices/{id}` | `system:notice:remove` |
| GET | `/api/v1/system/notices/top`、`POST …/{id}/read` | 仅需登录（个人已读） |

**操作日志（`monitor:operlog:*`，属主 `system`，只读视图）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/monitor/oper-logs` | `monitor:operlog:list` |
| GET | `/api/v1/monitor/oper-logs/{id}` | `monitor:operlog:detail` |
| DELETE | `/api/v1/monitor/oper-logs` | `monitor:operlog:remove` |
| GET | `/api/v1/monitor/oper-logs/export` | `monitor:operlog:export` |

**登录日志（`monitor:logininfor:*`，属主 `system`，只读视图）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/monitor/login-logs` | `monitor:logininfor:list` |
| DELETE | `/api/v1/monitor/login-logs` | `monitor:logininfor:remove` |
| PUT | `/api/v1/monitor/login-logs/unlock/{userName}` | `monitor:logininfor:unlock` |
| GET | `/api/v1/monitor/login-logs/export` | `monitor:logininfor:export` |

**在线用户（`monitor:online:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/monitor/online-users` | `monitor:online:list` |
| DELETE | `/api/v1/monitor/online-users/{sessionId}` | `monitor:online:forceLogout` |
| DELETE | `/api/v1/monitor/online-users` | `monitor:online:batchForceLogout` |

**定时任务（`monitor:job:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/monitor/jobs` | `monitor:job:list` |
| GET | `/api/v1/monitor/jobs/{id}` | `monitor:job:detail` |
| POST | `/api/v1/monitor/jobs` | `monitor:job:add` |
| PUT | `/api/v1/monitor/jobs/{id}` | `monitor:job:edit` |
| PUT | `/api/v1/monitor/jobs/{id}/status`、`/run` | `monitor:job:changeStatus` |
| DELETE | `/api/v1/monitor/jobs/{id}` | `monitor:job:remove` |
| GET | `/api/v1/monitor/jobs/export` | `monitor:job:export` |

**代码生成（`tool:gen:*`）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/tool/gen/tables` | `tool:gen:list` |
| GET | `/api/v1/tool/gen/tables/{id}` | `tool:gen:list` |
| GET | `/api/v1/tool/gen/db/tables` | `tool:gen:list` |
| POST | `/api/v1/tool/gen/import` | `tool:gen:list` |
| PUT | `/api/v1/tool/gen/tables/{id}` | `tool:gen:edit` |
| GET | `/api/v1/tool/gen/tables/{id}/preview` | `tool:gen:preview` |
| GET | `/api/v1/tool/gen/tables/{id}/download` | `tool:gen:code` |
| GET | `/api/v1/tool/gen/tables/download` | `tool:gen:code` |
| DELETE | `/api/v1/tool/gen/tables/{id}` | `tool:gen:remove` |

**缓存监控（`monitor:cache:*`，cicada 扩展命名空间）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/monitor/cache/overview` | `monitor:cache:list` |
| GET | `/api/v1/monitor/cache/domains` | `monitor:cache:list` |
| GET | `/api/v1/monitor/cache/keys?domain=…` | `monitor:cache:list` |
| GET | `/api/v1/monitor/cache/key?key=…` | `monitor:cache:list` |
| DELETE | `/api/v1/monitor/cache/keys?domain=…[&key=…]` | `monitor:cache:clear` |

**数据监控（`monitor:data:list`，cicada 扩展命名空间）**

| 方法 | 路径 | 权限标识 |
| --- | --- | --- |
| GET | `/api/v1/monitor/data` | `monitor:data:list` |

**监控只读页（`monitor:data:view` / `monitor:server:view` / `monitor:cache:view`）**：不再内嵌 HTML 控制台，改为 `/metrics` + 专用只读接口（7.8）；这三个 `:view` 权限点保留用于前端菜单可见性。缓存监控的 REST 守卫见上表——`monitor:cache:view` 只决定菜单是否可见，不授予任何接口权限。

> **补记（2026-10-17）**：本表的路径以**实现为准**重写了一遍，两类差异值得写下来，免得下次照这张表去找接口找错地方：
>
> - **字典是嵌套资源**：`/api/v1/system/dict/types` 与 `/api/v1/system/dict/data`（原表写的 `dict-types` / `dict-data`）。
>   代码里所有「子资源」都用斜线层级（`/roles/{id}/menus`、`/notices/{id}/readers`），字典没道理例外；
>   与其为迁就旧写法再开一套同义路由，不如把表对齐实现——两套路径并存会让权限点、审计与缓存键都出现两个身份。
> - **路径参数在实现里是用名的**：本表写 `{id}`，代码里是 `{role_id}` / `{menu_id}` / `{notice_id}`……
>   只影响 OpenAPI 文档里的参数名与排障可读性，不影响匹配；`normalized_route`（§7.8 的指标基数归一）
>   把两类都压成 `{id}`，所以两边不必强行统一。
>
> 同时补齐了此前「有承诺、无 handler」的 12 个接口：`PUT roles/{id}/status`、`PUT roles/{id}/data-scope`、
> `GET|PUT roles/{id}/users`、`GET menus/tree`、`GET menus/{id}`、`PUT menus/{id}/sort`、
> `GET menus/role-tree/{role_id}`、`GET depts/tree`、`PUT depts/{id}/sort`、
> `GET notices/{id}/readers`、`GET notices/top`。全部复用附录 D 已有的权限点（不新增），
> 验证见 `crates/web/tests/appendix_e.rs`（真实 PG）。两处取舍记在这里：
>
> - **`roles/{id}/users` 是一张带 `belonged` 标记的列表**，而不是参考实现的「已分配 / 未分配」两个端点：
>   两者要的其实是同一份数据 + 一个过滤条件，拆开会让搜索与翻页逻辑各写一遍。
>   `belonged=true|false` 过滤，省略看全部。
> - **`depts/{id}/sort` 只改同级顺序**：改父级是 `PUT /depts/{id}` 的职责——那里才有 `ancestors`
>   的级联重写。排序接口若也能移动，就会长出第二套会写错的树逻辑。
> - **`notices/top` 不挂权限点**（仅需登录）：它是「登录后有几条公告」的个人视图，带当前用户的 `read` 标记；
>   管理页的全量列表仍是 `system:notice:list`。
> - **内置 `admin` 角色不可停用**：`PUT roles/{id}/status` 对它返回 403——停用唯一管理员角色等于把自己锁在门外。
> - **角色用户分配整批拒绝**：请求里只要有一个用户不在当前租户可见范围内就全部不执行，
>   不做「能分多少分多少」，否则调用方以为这是一次事务，实际拿到的是半套权限。

---

## 附录 F：P1/P2 表结构

统一约定：主键 `bigint`，审计列 `create_by / create_time / update_by / update_time`，需要租户隔离的表带 `tenant_id`（6.4）。字段类型省略 `NOT NULL` 标注；`jsonb` 用于结构与多变的负载。

**`sys_file`（文件元数据，P1）**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `file_id` | bigint PK | |
| `tenant_id` | bigint | |
| `storage` | text | 驱动：`local` / `s3` / `oss` / `minio` |
| `path` | text | 不可猜测路径 `tenant/uuid`，对外不暴露 |
| `original_name` | text | 仅用于展示，不参与路径拼接 |
| `size` | bigint | 字节 |
| `mime` | text | 由 magic number 判定，不信扩展名 |
| `sha256` | text | 去重与完整性校验 |
| `biz_type` / `biz_id` | text / bigint | 业务归属，可空 |
| `deleted_at` | timestamptz | 逻辑删除 |

**`sys_audit_log`（字段级变更留痕，P1）**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `audit_id` | bigint PK | |
| `tenant_id` | bigint | |
| `table_name` | text | 被改动实体 |
| `record_id` | text | 主键值（字符串化，兼容复合键） |
| `action` | enum | `insert` / `update` / `delete` |
| `changed` | jsonb | 仅存变化字段：`{field: [old, new]}` |
| `oper_id` | bigint | 操作人 |
| `oper_time` | timestamptz | |
| `trace_id` | text | 关联链路 |

按月分区，保留期见 7.2。

**`sys_api_key`（开放接口凭证，P1）**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | bigint PK | |
| `tenant_id` | bigint | |
| `name` | text | 用途说明 |
| `key_prefix` | text | 明文前缀，仅用于展示与检索 |
| `secret_hash` | text | 只存哈希（Argon2id） |
| `scopes` | jsonb | 允许的权限标识/接口范围 |
| `status` | enum | `Active` / `Disabled` |
| `expires_at` | timestamptz | 可空 |
| `last_used_at` | timestamptz | 可空 |

**`sys_export_task`（异步导出，P1）**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | bigint PK | |
| `tenant_id` / `user_id` | bigint | |
| `module` | text | 导出模块标识 |
| `query` | jsonb | 条件快照（含数据范围，保证与请求一致） |
| `status` | enum | `Pending` / `Running` / `Done` / `Failed` |
| `file_id` | bigint | 完成后指向 `sys_file` |
| `total` / `processed` | bigint | 进度 |
| `error` | text | 失败原因（可空） |
| `finish_time` | timestamptz | 可空 |

**`outbox_event`（事务性发件箱，P1，见 16.2）**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | bigint PK | 单调递增，供 relay 顺序扫描 |
| `tenant_id` | bigint | |
| `aggregate` / `aggregate_id` | text / text | 事件源（如 `audit` / 记录 id） |
| `event_type` | text | 如 `audit.oper_log`、`notify.send` |
| `payload` | jsonb | 事件负载 |
| `created_at` | timestamptz | |
| `published_at` | timestamptz | relay 投递后写，可空 |
| `attempts` | int | 重试次数 |

索引：`(published_at, id)` 供 relay 扫描未投递事件。

**`sys_user_online`（在线会话快照，属主 `auth`，见 6.1）**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `session_id` | text PK | access token 的 `jti`；主键即幂等键，write-behind 重复投递只会 upsert |
| `tenant_id` / `user_id` / `user_name` | bigint / bigint / text | 会话归属 |
| `ip` / `user_agent` | text | 登录来源；浏览器/OS 不在这张表算 |
| `login_time` / `last_access_time` | timestamptz | 来自会话本体，不是投递时刻 |
| `status` | smallint | 0 在线 / 1 已退出（对齐参考实现同名列） |
| `end_time` | timestamptz | 退出时刻；仍在线为 `NULL` |

索引：`(tenant_id, login_time)`（历史翻页）、`(user_id)`（查某人的会话）。
它是**派生数据**：`OnlineSnapshot::reconcile_with` 以 Redis 为准对账（组装根启动时跑一次），
把「库里在线、权威源已没有」的行收成已退出；已退出的行不删——那正是这张表的用途。

**`sys_notify_channel` / `sys_notify_template` / `sys_notify_record`（通知中心，P2）**

| 表 | 关键列 |
| --- | --- |
| `sys_notify_channel` | `id`、`tenant_id`、`code`、`kind`（`sms`/`email`/`webhook`/`im`）、`config` jsonb、`enabled` |
| `sys_notify_template` | `id`、`tenant_id`、`code`、`channel_kind`、`locale`、`title`、`body`（minijinja）、`enabled` |
| `sys_notify_record` | `id`、`tenant_id`、`channel_id`、`template_code`、`recipient`（§ 原拟列名 `to` 是 SQL 关键字，落库改名）、`payload` jsonb、`status`、`error`、`sent_at` |

**`sys_oauth_client` / `sys_oauth_token`（开放授权，P2）**

| 表 | 关键列 |
| --- | --- |
| `sys_oauth_client` | `id`、`tenant_id`、`client_id`、`secret_hash`、`redirect_uris`、`scopes`、`grant_types`、`enabled` |
| `sys_oauth_token` | `id`、`client_id`、`user_id`、`access_hash`、`refresh_hash`、`scopes`、`expires_at`、`revoked_at` |

> **未建（2026-10-21）**：这两张表描述的是「cicada 自己当授权服务器」（给第三方应用发
> access/refresh token）。一期没有对应的接口约定（§6.1 的 `/auth/*` 全是**登录**方向，即
> cicada 当 OAuth **客户端**，落地见 ADR 0007），机器凭证走 `sys_api_key` + `X-API-Key`
> （ADR 0003）。同一件事留两套模型正是 §14「权限模型分裂」要点名的风险，因此不建表；
> 真要对外发 token 时按 M5 的纪律另立 ADR，一并定接口、同意授权与撤销语义。

**`sys_tenant_package`（租户套餐与限额，P2）**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | bigint PK | |
| `code` / `name` | text | 套餐标识与名称 |
| `user_limit` | int | 用户数上限（-1 = 不限） |
| `storage_limit_mb` | bigint | 存储上限 |
| `api_qps_limit` | int | 接口 QPS 上限 |
| `features` | jsonb | 功能开关集合 |

**`sys_user_identity`（登录身份绑定，见 6.1）**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `identity_id` | bigint PK | |
| `tenant_id` | bigint | 参与唯一键 |
| `user_id` | bigint | 指向 `sys_user` |
| `identity_type` | enum | `Username` / `Phone` / `Email` / `WeChat`（可扩展） |
| `identifier` | text | 账号名 / E.164 手机 / 小写邮箱 / `openid` 或 `unionid` |
| `provider` | text | 外部提供方标识（如 `wechat:mp`），本地身份为空 |
| `verified_at` | timestamptz | 验证时间，可空 |
| `is_primary` | bool | 该类型下的主身份，用于同步 `sys_user` 镜像列 |

唯一键 `(tenant_id, identity_type, identifier)`；索引 `(user_id, identity_type)`。

---

## 附录 G：数据迁移

迁移只从空库建立 cicada 自己的 schema；已有数据由使用者在业务侧自行处理。
