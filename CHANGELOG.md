# 更新日志

`cicada` 基座的公开契约变化都记在这里：**HTTP 接口与响应形状、配置键、数据库结构、
错误业务码、门禁命令**。格式参考 Keep a Changelog，版本遵循 semver。

判定口径（模板化机制的「升级通道」）：不改代码、不改配置、不跑迁移就能升上去的记在
`Added` / `Changed`；反之必须记在 `Breaking`，并写清使用者要做的那个动作。
数据库结构一律走迁移（append-only），不在这里教人改库。

## [未发布]

### Added（未发布）

- 数据监控：`GET /api/v1/monitor/data` 返回 PostgreSQL 库/表只读统计（库大小、连接池、
  会话数、行数、扫描次数、vacuum/analyze 时间），前端新增 `/monitor/data` 页面；
  接口权限点 `monitor:data:list`（迁移 000018 与种子补齐）。

### Fixed（未发布）

- 页面内容区恢复纵向滚动：body 锁滚动后，主内容区自己承担 `overflow-y: auto`，
  长表格与长表单不再被视口截断。
- 宽表支持鼠标横向滚动：横向滚动条常驻，Shift + 滚轮 / 触控板横向手势可直接滚动表格。
- 侧栏外链菜单刷新：迁移 000018 修正框架自带外链菜单的旧文案，前端对菜单缓存增加展示兜底。
- 操作日志查询失败时返回可读提示，不再只有笼统的「内部错误」。
- 新建用户表单禁止浏览器自动填充账号/昵称/密码；重置密码字段使用 `new-password`。
- 通知渠道按通道类型预填合法 `config` 模板，并在提交前校验 `endpoint` / `host`。
- 通知「测试发送」与发送记录补上模板编码、接收方、JSON 对象校验。
- 代码生成配置补上实体类名、功能名称、模块名、业务名的必填校验。
- 操作/登录日志「清空当前筛选结果」现在真的按筛选条件删除，不再误删整个租户日志。

## 0.1.0 - 2026-10-21

M0–M4 的完整基座：认证与权限、八个基础域、审计、导入导出、代码生成、定时任务、
多租户配额、通知中心、开放 API（API Key）、限流、可观测性。

### Breaking

- **错误响应改为 RFC 9457 `application/problem+json`**（统一响应）。这些情况的状态码变了：
  TOTP 码错误 `401 → 422`（`CICADA_AUTH_1006`）、重复绑定身份 `422 → 409`（`…_1008`）、
  缓存/会话存储不可用统一 `503 CICADA_INTERNAL_9003`、删除有子节点 `409 CICADA_SYS_3004`、
  树形成环 `422 CICADA_SYS_3005`、幂等冲突 `409 CICADA_SYS_3002`、
  字段校验 `422 CICADA_VALIDATION_4001` 且带 `errors: [{field,message}]`。
  未匹配路径的 404 与提取器（`Json`/`Query`/`Path`）的拒绝体也改成 problem+json；
  方法不匹配仍是空体 405（业务码目录未给 405 分配业务码）。
  动作：前端按 `code` / `errors[]` 判定，不要再拿状态码猜语义。
- **JWT 配置段换形状，默认算法改 EdDSA**（认证设计）：`auth.jwt_secret` 作废，改为
  `auth.jwt = { alg, kid, private_key, public_key, keys: [...] }`。EdDSA 必须同时给公钥，
  且 `CICADA_PROFILE=prod` 下仍是示例值或短 HS256 密钥会**拒绝启动**。
  动作：生成一对 Ed25519 密钥填进去；要轮换就把旧 kid 的公钥留在 `keys` 里。
- **配置加载改成五档叠加**（配置）：代码默认值 → `config/default.yaml` →
  `config/{profile}.yaml` → `config/local.yaml` → 环境变量（`CICADA_` 前缀，`__` 分层）。
  同名键的胜出方与以前不同，且启动期做形状校验（不认识算法、kid 重复、EdDSA 缺公钥都拦）。
- **分页上限**：`page_size` 超过 200 或为 0 → `422 CICADA_VALIDATION_4004`，
  不再静默按请求值去查库。动作：批量拉数据的前端与脚本改按 200 翻页，或走异步导出。
- **审计不再由 `audit` 直写 `sys_oper_log` / `sys_logininfor`**（审计设计，ADR 0008）：
  生产者只写 `outbox_event`（迁移 000011），由 relay 投递给属主表。
  动作：起 `cicada-worker`（或 `bootstrap` 的 relay 任务），否则日志只进发件箱、读接口看不到。
- **`sys_notice.notice_content` 入库前做富文本白名单清洗**（内容安全）：脚本、内联样式、
  事件属性、`javascript:` / `data:` 链接被去掉。**已有库里的旧内容不会被回改**，
  要清洗历史数据请自己走一遍更新接口或写一条使用者迁移。
- **`sys_oper_log` 多三列**（迁移 000017）：`user_agent` / `biz_code` / `req_body`，
  读接口的 `items[]` 相应多出三个可空字段（旧行为 `null`）。
- **日志表按月分区**（迁移 000013）：`sys_oper_log` / `sys_logininfor` 按 `oper_time` /
  `login_time` 分区，超过 `audit.retention_days`（默认 180）的分区归档到对象存储后删除。
  手工建过分区的环境需要先确认分区命名与 `deploy/` 的规则一致。
- **成功创建返回 `Location` 头**（统一响应）：`201` 带新建资源的 URI。只加头不坏体，
  但严格校验响应头的客户端要更新期望。

### Added

- 认证：登录/刷新/登出/改密、图形验证码（失败达阈值后强制）、TOTP 可选绑定、
  会话 `(ip, user_agent)` 指纹、在线会话（`sys_user_online`，迁移 000014）与历史查询。
- 权限：`sys_menu` 权限点与菜单树、角色/用户/部门/岗位/字典/参数/公告、
  五种数据范围（`Scope` 类型状态，编译期拦住未授权查询）、装配期权限点双向对账。
- 两级缓存：权限集合、菜单树、字典、参数、租户信息；版本号失效键
  `cicada:{域}:ver:{slot}`，防击穿单飞、防雪崩 TTL 抖动、防穿透空值短 TTL。
- 开放 API（ADR 0003）：`sys_api_key` + `X-API-Key`，`scopes` 精确匹配进权限集合。
- 通知中心（ADR 0002）：站内 / 邮件（lettre，真走 SMTP）/ Webhook 三通道 + 模板 + 投递记录。
- 文件存储：opendal 驱动按配置切（`storage.driver = local | rustfs | oss | s3`）、
  magic-number 校验、两条下载路径（对象存储 presign；本地走一次性令牌，
  `GET /api/v1/files/dl/{token}` 单次消费且免鉴权）。
- 异步导出：`sys_export_task`（迁移 000010）+ 完成通知；大表导出不进内存。
- 代码生成：单表 / 树表 / 主子表模板，生成物由 fixture crate 常态编译验证。
- 定时任务：apalis + PostgreSQL，在线增删改，集群不重复执行。
- 可观测性：真实 `/metrics`（HTTP 时延与 QPS、连接池、缓存命中、任务结果、
  按业务码的错误率）、OTLP trace 导出、慢查询日志（`db.slow_query_ms`）。
- 多租户（ADR 0001 / 0004）：`tenant_id` 贯穿、RLS 兜底开关、租户套餐与 QPS 限额。
- 限流（请求生命周期第 7 层、ADR 0005）：单 IP 令牌桶在最外层，租户 `api_qps_limit` 在鉴权后。
- 二开通道（模板化机制）：`cargo generate` 模板、`cicada new`、单一入口 crate `cicada`、
  `business/` 物理分离、`[patch]` 覆写逃生口。
- 门禁（部署）：`cargo fmt --check`、`clippy -D warnings`、单元 + `--ignored`
  真实库集成测试、`cargo doc -D warnings`（死链）、`cargo deny`、`cargo machete`、
  OpenAPI 快照、分层检查（`deploy/check_layering.py`）、容器镜像冒烟。

### Changed

- 优雅停机：`SIGTERM` / `SIGINT` 后停止接新连接、等在途请求跑完
  （`server.graceful_shutdown_secs`，默认 30），
  再关连接池；看门狗超时兜底，避免半开长连接把停机拖成长尾。
- 连接池默认值随核数派生（`db.max_connections: 0` = 自动），显式配置仍然优先。
- 口令策略真正生效：`auth.password.min_length`（默认 8）与复杂度校验，
  示例口令在 prod profile 下拒绝启动；失败锁定（`max_retry` / 锁定时长）与递增退避
  在同一策略里，跨实例计数走 Redis。
- 图形验证码（安全设计）：`auth.captcha` 达到失败阈值后强制。
- 模糊查询走 `ILIKE` + `pg_trgm` GIN 索引（分页与查询，迁移 000015）；可模糊检索的列清单
  由 `FUZZY_COLUMNS` 单点导出，索引与核对测试读同一份数据。

### Fixed（值得一提的）

- `CICADA_*` 环境变量对两层以上键（如 `CICADA_SERVER_PORT`）此前会被整段忽略。
- 审计的请求体采集改成与主流程共用同一个流：chunked 上传也采得到参数，
  而超限或读流出错时作废的是副本，不再可能让 handler 收到空体。
- 缓存监控的键扫描按游标翻页，不再只看第一页就判定「没有越界键」。

### 依赖与许可

新增 `opendal`（含 s3 / oss 服务）、`lettre`（rustls + aws-lc-rs，无 native-tls）、
`futures-util`（请求体旁路记录）、`ammonia`（富文本白名单）。
`ammonia` 的 HTML 解析器链（`html5ever` / `cssparser` / `selectors`）是 MPL-2.0，
已在 `deny.toml` 逐条写明放行理由；TLS 仍只有 rustls，`native-tls` / `openssl` 是硬禁。

### 升级指引

见 `docs/UPGRADE.md`：迁移怎么跑、`seaql_migrations` 里那条已作废的 `000003` 怎么处理、
以及上面每条 Breaking 对应的动作。
