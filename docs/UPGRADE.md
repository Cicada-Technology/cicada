# 升级指南（§16.3 的「升级通道」）

这份文档只回答一个问题：**框架出了新版本，你怎么升上去**。
逐版本「改了什么、你要做什么」在 `CHANGELOG.md`；这里定的是动作与纪律。

## 三种模式各自的升级路径

| 模式 | 工程形态 | 升级动作 |
| --- | --- | --- |
| A. 开箱即用 | `cargo generate` 出来的自己的工程，框架走 git tag 依赖 | 改 `Cargo.toml` 里的 tag → `cargo run -p cicada-cli -- migrate` → 对着 CHANGELOG 的 Breaking 逐条处理 |
| B. 深度定制 | 同 A，另外自己实现端口 adapter、写 `business/` | 同 A；只在自己实现的那几个端口签名跟着变时才需要动代码 |
| C. 完全接手 | fork 全仓 | 按 CHANGELOG 手工合并。**不推荐**，但必须可行——所以框架的迁移文件不许改（见下），你的改动也只碰允许的目录 |

`[patch]` 段是本地临时覆写的官方逃生口：

```toml
[patch."https://github.com/<org>/cicada"]
cicada = { path = "../cicada-local-fix" }
```

代价是它是债：每次升级前先逐个摘掉 patch，确认上游已经修了再删。留着 patch 升版本，
下一次的「我改的到底生不生效」会变成一个谁也答不上来的问题。

## 升级的四步

1. **先看 Breaking，再看 Added。** CHANGELOG 的 Breaking 每条都写了使用者要做的那个动作；
   先把它做完再跑迁移，否则中途失败不知道是谁的锅。
2. **备份，然后跑迁移。**

   ```bash
   pg_dump …                                          # 结构 + 数据，按你的保留策略
   cargo run -p cicada-cli -- migrate                 # up（默认）
   cargo run -p cicada-cli -- migrate status          # 该应用的都应用了
   ```

   迁移是 append-only 的，`up` 只会往前；`migrate down` 只在开发环境用，
   生产回滚靠备份恢复，不靠反向迁移。
3. **配置跟着改。** 键名变化都在 CHANGELOG 里（0.1.0 的一次性清单：`auth.jwt_secret`
   → `auth.jwt`、口令策略在 `auth.password`、验证码在 `auth.captcha`、
   分区保留在 `audit.retention_days`、停机预算在 `server.graceful_shutdown_secs`）。
   五档优先级是 代码默认值 < `config/default.yaml` < `config/{profile}.yaml` <
   `config/local.yaml` < 环境变量（`CICADA_*`，`__` 分层），
   所以改哪一档要按「谁能覆盖谁」想清楚，不要在 `default.yaml` 里写生产值。
4. **跑门禁。** 与 CI 完全一致的命令集：

   ```bash
   cargo build --workspace                      # 类型层面先给出「接口变了」的清单
   cargo test --workspace
   cargo test --workspace --no-fail-fast -- --ignored   # 需要真实 PG + Redis
   ```

## 迁移纪律（为什么能只往前）

- **框架的迁移文件永不被修改，只被追加。** 你已经应用过的 `000005` 在下一个版本里还是
  那个 `000005`，否则 `seaql_migrations` 的历史与代码就对不上了。
- **序号只增不补。** 唯一的历史例外：`000003`（`sys_user` 的 TOTP 两列）已并入
  `000001` 的建表（一张表只建一次、一次建成最终形态），序号 `000003` 永久空着。
  **如果你的库里还留着 `m20261009_000003_totp` 这一行**，SeaORM 会因为「列表里没有它」
  而拒绝跑迁移（`Migration file ... is missing`）。那两列现在由 `000001` 负责建，
  所以删掉这一行记录就够了，不动数据：

  ```sql
  DELETE FROM cicada.seaql_migrations WHERE version = 'm20261009_000003_totp';
  ```

  （schema 不是 `cicada` 的话换掉前缀；`db.schema` 就是你的 search path。）
- **使用者工程的迁移拼在框架列表前面，只有一个 `Migrator`、一张 `seaql_migrations`。**

  ```rust
  impl MigratorTrait for Migrator {
      fn migrations() -> Vec<Box<dyn MigrationTrait>> {
          let mut all = cicada::migrations::framework_migrations();
          all.push(Box::new(m20261015_000001_goods::Migration));
          all
      }
  }
  ```

  两个 `Migrator` 共用一张迁移表是不行的：SeaORM 会把「对不上本列表的已应用版本」判为错误。

## 0.1.0 的几条 Breaking 具体怎么迁

- **错误响应（RFC 9457）**：成功体不变，失败体从 `{ "error": … }` 变成
  `application/problem+json`（`type` / `title` / `status` / `detail` / `instance` / `code`，
  字段级校验另有 `errors[]`）。状态码迁移表见 CHANGELOG。前端只要按 `code` 分支即可，
  `utils/api-error.ts` 已经是这么写的。
- **JWT 走 EdDSA**：`auth.jwt.alg: EdDSA` 时 `private_key` / `public_key` 都是 PEM
  （PKCS#8），验签只用配置里列出的密钥，`keys` 是轮换期内仍需验的旧公钥。产一对：

  ```bash
  openssl genpkey -algorithm ed25519 -out jwt-private.pem
  openssl pkey -in jwt-private.pem -pubout -out jwt-public.pem
  ```

  （`openssl` 在这里只是产密钥的运维工具；运行时的 TLS 仍然只有 rustls，
  `native-tls` / `openssl` 依赖在 `deny.toml` 里是硬禁。）
  私钥经环境变量注入：`CICADA_AUTH__JWT__PRIVATE_KEY`。想留在 HS256 就把 `alg` 写回
  `HS256` 并保证密钥不短于 32 字符——prod profile 下示例值与短密钥会拒绝启动。
- **审计落库走 outbox**：`audit` 只写 `outbox_event`，`sys_oper_log` / `sys_logininfor`
  由 relay 投递后写。只跑 web 不跑 worker 的话，日志接口会一直看不到新记录——
  这时先看发件箱积压，别怀疑写接口。
- **分页上限 200**：超限是 `422 CICADA_VALIDATION_4004`，不是静默截断。
  要大包就按 200 翻页，或者走异步导出（`sys_export_task`）。
- **富文本清洗不回改历史数据**：`sys_notice.notice_content` 的白名单只作用在写入路径。
  历史脏内容要么逐条走一次更新，要么写一条你自己的使用者迁移。

## 什么算「你的目录」，什么算框架的

模式 C 合并时按这条线判断：**只有 `business/`、`config/`、`templates/`、
`migrations/`（你自己的迁移文件）、前端业务视图是你可以改的。**
把业务代码写进框架 crate、改框架的既有迁移、在框架里写「如果租户 X 就怎样」——
这三件事在 §16.3 里被明确列为反模式，因为它们正是让升级变不可能的原因。
需要改框架行为时走覆盖点（配置、种子、模板、端口 adapter、模块增删）；
覆盖点不够就用 `[patch]` 顶上并回报上游，而不是直接改框架源码。
