# cicada-auth

L3 能力：认证——身份解析、凭据校验、会话与令牌的生命周期。

## 职责

- `login`：登录 / 刷新 / 登出。**会话的权威在 Redis**（`session:{jti}`），
  `sys_user_online` 只是快照（见下面 `online`）。刷新令牌一次一换，旧令牌被复用即判定重放并撤销该用户
  整条会话链（`SessionPolicy` 决定 TTL 与轮换策略）。
- `token`：`TokenService` 签发/校验 JWT（access + refresh + 身份绑定票据 `BindingTicketClaims`）。
- `password`：Argon2id 写入；兼容参考实现搬迁来的 BCrypt 哈希，`needs_rehash` 用于登录成功后
  按需升级。密码哈希串自带参数，不另存 salt。
- `lockout`：连续失败计数与临时锁定（`LockoutPolicy` / `LockoutStore`）。
- `captcha`：图形验证码，连续失败 `required_after` 次后登录强制校验（安全设计）。
- `identity` + `totp`：手机/邮箱验证码与 TOTP 二次验证；未配置通道时 `RejectAllVerifier`
  **失败关闭**，不静默放行。
- `online`：`sys_user_online` 的属主。`SnapshottedSessionStore` 装饰真正的
  `SessionStore`，Redis 写成功后把会话生命周期 write-behind 到快照表——写在 `tokio::spawn` 里，
  失败只 `warn`，所以 DB 抖动不会让登录变慢或失败；主键用 `jti`，重复投递天然幂等。
  `OnlineSnapshot::reconcile_with` 以权威源对账（组装根启动跑一次）：进程被杀 / TTL 过期这两条
  路径不经过 `revoke`，只有对账能把「库里在线、Redis 已没有」的行收成「已退出」。
  退出行不删——历史就是这张表的用途。判定路径（会话存在性 / `max_sessions` / 踢出）一律不看它。
- `oauth`：微信开放平台与通用 OIDC 共用同一组端口（`OAuthClient` / `OAuthStateStore`，ADR 0007）；
  未配置 provider 时相关接口一律 404。

## 公开 API

`login` / `logout` / `refresh` / `LoginResult` / `RefreshResult` / `SessionPolicy` /
`canonical_identifier`、`TokenService` / `TokenClaims` / `BindingTicketClaims`、
`SessionStore` / `Session` / `RedisSessionStore` / `InMemorySessionStore` / `hash_refresh`、
`SnapshottedSessionStore` / `OnlineSnapshot` / `ReconcileReport` / `online::{Entity, Column, Model}`（`sys_user_online`）、
`hash_password` / `verify_password` / `needs_rehash`（`password::*`）、`LockoutStore` / `LockoutPolicy` / `LockState`（含 Redis 与内存实现）、
`CaptchaService` / `CaptchaParams` / `CaptchaStore`、
`Identity` / `IdentityType` / `IdentityVerifier` / `StaticCodeVerifier` / `RejectAllVerifier` /
`bind_identity` / `unbind_identity` / `list_identities`、`TotpSetup` / `generate_totp_setup` / `verify_totp`、
`OAuthService` / `OAuthClient` / `HttpOAuthClient` / `ExternalIdentity` / `ProviderKind` /
`authorize_url` / `redirect_uri`、`AuthError`。

## 依赖

`cicada-kernel`（Principal / SessionId 等值对象）/ `cicada-infra`（`Cache` 与 `Db`）/ `cicada-ports`
/ `cicada-macros`（快照实体的 `DataScoped`）、
`jsonwebtoken`、`argon2`、`bcrypt`、`totp-rs`、`captcha`、`reqwest`（rustls）、`sea-orm`、
`sha2`、`async-trait`、`tokio`（快照写的后台任务）。

## 不做什么

- **不算权限**：角色合并、权限点集合、数据范围都在 `cicada-rbac`；本 crate 只回答「你是谁」。
- 不出 HTTP 面：路由、请求提取与错误→状态码映射在 `cicada-web` 的 `auth` / `oauth` / `identity`。
- 不管 API 凭证（对外系统调用）：那是 `cicada-system::ApiKeyService`（ADR 0003）。
- 不做权限缓存：授权快照缓存在 `rbac::AuthzCache`。
- 生产不依赖内存实现：`InMemory*Store` 只服务测试，跨实例会话/踢出必须走 Redis。
