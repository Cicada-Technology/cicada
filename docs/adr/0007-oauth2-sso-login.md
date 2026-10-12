# ADR 0007：第三方登录走「端口 + adapter」，未绑定只发短时效绑定票据

- 日期：2026-10-10
- 状态：已采纳

## 背景

设计文档 §6.1 列了 `POST /auth/login/wechat`（开放平台扫码，一期落地），§4.1 把「通用 OIDC」
放在二期（= M5），但此前只落地了身份模型（`sys_user_identity`）与自助绑定，
没有任何第三方登录入口。两类提供方的协议细节不同（微信是 `appid` + 两步 GET + HTTP 200 里塞
`errcode`；OIDC 是表单换令牌 + Bearer UserInfo），而编排逻辑（state 校验、找身份、发令牌/票据）
完全相同——正是设计文档 §3.3 第 7 条「缝」的典型场景。

## 结论

- **端口**：`cicada_auth::oauth::OAuthClient`（授权码 → `ExternalIdentity`）与
  `OAuthStateStore`（`state` 一次性登记/消费）。**两个 adapter 各一份**：
  - `HttpOAuthClient`（reqwest，生产：微信 / OIDC 两套协议）/ `RecordingOAuthClient`（测试）；
  - `RedisOAuthStateStore`（`SET EX` + `compare_and_delete` 的 Lua CAS，天然原子单次消费）/
    `InMemoryOAuthStateStore`（测试与单机开发）。
- **编排**：`OAuthService` 只依赖两个端口，因此整条链路（授权地址 → 回调换身份 → 已绑定发令牌 /
  未绑定发票据 → 票据绑定 → 再回调直接发令牌）可以在没有真实 IdP 的前提下测通。
- **接口**（都在 `cicada:http` 内置模块，不声明权限点）：
  - `GET /auth/login/{provider}/authorize-url` → `{ authorize_url, state }`（`state` 服务端生成）；
  - `POST /auth/login/{provider}/callback` → 已绑定 `{status:"bound", login:{...}}`；
    未绑定 `{status:"unbound", binding_ticket, external:{...}}`；
  - `POST /auth/identities/oauth`（需登录）→ 用票据把外部身份绑到当前账号。
- **不按邮箱自动合并**：外部身份没绑过就**不建号、不发令牌**，只发由 `TokenService` 签的
  短时效「绑定票据」（`purpose=bind`，claims 与访问令牌完全不同，因此两者不可互换；
  校验不留时钟宽限）。绑定必须由已登录用户主动发起，避免「提供方邮箱相同即接管已有账号」。
- **state 绑定提供方**：`state` 的值就是提供方 code，消费时做 CAS；重放/跨提供方/过期一律 401。
- **身份落库**：微信取 `unionid`（缺失回退 `openid`）、`identity_type = WeChatOpen`，
  `provider` 存配置里的 `code`；OIDC 用新增的 `identity_type = Oidc`，`identifier = {code}:{sub}`。
- 超限/错误映射：未配置的提供方 404，`state` 失效 401，上游失败 502（附录 C 的 502 `Upstream`）。

## 代价

- **端点显式配置**（`authorize_url` / `token_url` / `userinfo_url`），不做事先的
  `/.well-known/openid-configuration` 发现：少一次启动期/运行期网络依赖，代价是换 IdP 时要抄三个 URL。
  需要发现能力时在 `HttpOAuthClient` 内部加缓存即可，接口不变。
- OIDC 的 `identifier` 带 `{code}:` 前缀：`sys_user_identity` 的唯一索引是
  `(tenant_id, identity_type, identifier)`，不含 `provider`，加前缀是为了不同 IdP 的 `sub`
  撞车。要支持「同一 IdP 标识跨提供方合并」时，应改成把 `provider` 纳入唯一索引（需迁移）。
- 微信小程序登录（`code2session`）**未实现**：它不走授权码跳转，等小程序端落地时按同样方式
  加一个 adapter。`identity_type` 已预留 `WeChatMp`。
- 未做刷新上游令牌（我们不保存第三方 `access_token`/`refresh_token`），只在回调瞬间取一次身份；
  需要「代用户调用第三方 API」时再引入令牌存储（届时是新的 ADR）。
- 未采用设计文档 §4.1 快照里的 `oauth2` 5.0.x crate：它按 RFC 6749 建模并提供 TokenResponse 解析，
  但两家的差异（微信 `appid`/两步 GET/`errcode`、OIDC 的 UserInfo 形状）仍需自己写分支，
  而本场景只需要「授权地址 + 授权码换身份」两个动作，自研 adapter 少一层依赖与类型映射。
- 未做 OIDC 的 `nonce`/`id_token` 验签：我们只信任 TLS 保护的 UserInfo 端点。需要更强保证时
  应校验 `id_token` 的签名与 `nonce`。
