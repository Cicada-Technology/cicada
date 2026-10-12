//! 分层配置。敏感值用 `Secret<T>` 掩码，避免日志泄露。

use serde::{Deserialize, Serialize};

/// 敏感值新类型：`Debug` 输出掩码，只通过 `expose_secret()` 显式取用。
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Secret<T>(T);

impl<T: Default> Default for Secret<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T> Secret<T> {
    pub fn new(v: T) -> Self {
        Self(v)
    }

    pub fn expose_secret(&self) -> &T {
        &self.0
    }
}

impl<T> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DbConfig {
    pub url: Secret<String>,
    /// 池上限；`0` 表示 `CPU 核数 × 2`（数据层约定），由 `resolved_max_connections` 展开。
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_min_connections")]
    pub min_connections: u32,
    #[serde(default = "default_acquire_timeout")]
    pub acquire_timeout_secs: u64,
    #[serde(default = "default_schema")]
    pub schema: String,
    /// 慢查询阈值（毫秒）：超过即以 `warn` 记录 SQL 摘要与耗时。
    /// `0` 关闭这条日志。
    #[serde(default = "default_slow_query_ms")]
    pub slow_query_ms: u64,
}

/// 连接池上限的代码默认值：`0` 表示「按机器定」（默认 CPU 核数 * 2）。
///
/// 写死 20 是错的：同一条配置在 2 核的边节点和 32 核的库节点上是两种故障形状——
/// 前者会把请求排在池外，后者会把 PG 的连接数顶到 `max_connections` 之外。
fn default_max_connections() -> u32 {
    0
}

fn default_min_connections() -> u32 {
    0
}

fn default_acquire_timeout() -> u64 {
    8
}

fn default_slow_query_ms() -> u64 {
    200
}

fn default_schema() -> String {
    "public".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    pub port: u16,
    pub body_limit_mb: usize,
    pub graceful_shutdown_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            body_limit_mb: 20,
            graceful_shutdown_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RedisConfig {
    pub url: Secret<String>,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: Secret::new("redis://localhost:6379/0".to_string()),
        }
    }
}

/// RLS 兜底：默认关闭，按表开启。表名必须命中
/// `cicada_infra::rls::TENANT_TABLES` 白名单，启动期校验失败即 fail fast。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RlsConfig {
    pub tables: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TenantConfig {
    pub enabled: bool,
    pub default_id: i64,
}

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_id: 1,
        }
    }
}

/// 一条 OAuth2 / OIDC 登录提供方（「通用 OIDC 二期」、「微信扫码登录」）。
///
/// `kind = wechat` 走微信开放平台扫码（授权码 → `openid`/`unionid`）；
/// `kind = oidc` 走通用授权码流程（令牌端点 + UserInfo，`sub` 作为标识）。
/// 端点显式配置：不做事先的 `/.well-known/openid-configuration` 发现（见 ADR 0007 的代价）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OAuthProviderConfig {
    /// 路由里的提供方标识，如 `wechat` / `keycloak`；同时作为身份的 `provider` 列。
    pub code: String,
    /// `wechat` | `oidc`。
    pub kind: String,
    pub client_id: String,
    pub client_secret: Secret<String>,
    pub authorize_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    /// 回调地址（必须是提供方控制台登记的那个完整 URL）；留空退化为相对路径。
    #[serde(default)]
    pub redirect_uri: String,
    /// 空表示按 `kind` 取默认值（`snsapi_login` / `openid profile email`）。
    #[serde(default)]
    pub scopes: Vec<String>,
    /// 覆盖默认身份类型（`wechat` → `WeChatOpen`，`oidc` → `Oidc`）。
    #[serde(default)]
    pub identity_type: Option<String>,
}

/// 第三方登录。`providers` 为空即关闭：相关接口一律 404。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OAuthConfig {
    pub providers: Vec<OAuthProviderConfig>,
    /// 授权 `state` 有效期（秒），单次使用（CSRF 防护）。
    pub state_ttl_secs: u64,
    /// 未绑定身份时下发的「绑定票据」有效期（秒）。
    pub ticket_ttl_secs: u64,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            state_ttl_secs: 300,
            ticket_ttl_secs: 600,
        }
    }
}

/// 一把**只用于验签**的密钥（轮换期的旧密钥、或别的服务下发的公钥）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JwtKey {
    /// 与 JWT 头里的 `kid` 对应。
    pub kid: String,
    /// EdDSA 时是 PEM 公钥；HS256 时是共享密钥。留空表示与顶层 `private_key` 同值（HS256 的省事写法）。
    #[serde(default)]
    pub key: Secret<String>,
    /// 该密钥的算法；留空沿用顶层 `alg`。
    #[serde(default)]
    pub alg: String,
}

/// JWT 签名与验签配置（`auth.jwt`、「签名优先 EdDSA，支持 kid 多密钥轮换」）。
///
/// 为什么 EdDSA 要同时给公钥：非对称签名里私钥不能验签，而模块契约的「把 auth 拆出去、
/// 别的服务本地验签」需要的就是那份公钥。缺公钥时构造 `TokenService` 会直接失败——
/// 「签得出、验不了」的部署会在第一个请求上炸，不如在启动时炸。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct JwtConfig {
    /// `EdDSA`（Ed25519，技术选型的优先项）或 `HS256`（同密钥回退，开发默认）。
    pub alg: String,
    /// 当前签名密钥的 `kid`；写进 JWT 头，也用来挑验签密钥。
    pub kid: String,
    /// EdDSA：PKCS#8 PEM 私钥；HS256：共享密钥。配置：值应来自 `${JWT_PRIVATE_KEY}`。
    pub private_key: Secret<String>,
    /// EdDSA：SPKI PEM 公钥（验签用）。HS256 留空。
    pub public_key: Secret<String>,
    /// 额外仍参与验签的密钥：轮换期并存的旧 kid，或来自其他签发方的公钥。
    pub keys: Vec<JwtKey>,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            alg: "HS256".to_string(),
            kid: "1".to_string(),
            private_key: Secret::new(WEAK_JWT_SAMPLE.to_string()),
            public_key: Secret::new(String::new()),
            keys: Vec::new(),
        }
    }
}

/// 仓库里那份开发示例值。生产启动（`CICADA_PROFILE=prod`）见到它就拒绝起来。
pub const WEAK_JWT_SAMPLE: &str = "dev-secret-change-me";

/// 共享密钥的最小长度（「启动时校验长度与格式」）。
pub const MIN_JWT_SECRET_LEN: usize = 32;

/// 密码策略（`auth.password`）：失败锁定 + 最小长度。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PasswordConfig {
    /// 连续失败次数上限，`0` 不锁定（对应参考实现 `maxRetryCount`）。
    pub max_retry: u32,
    /// 锁定时长（分钟，对应参考实现 `lockTime`）。
    pub lock_minutes: u64,
    /// 密码最小长度。
    pub min_length: usize,
}

impl Default for PasswordConfig {
    fn default() -> Self {
        Self {
            max_retry: 5,
            lock_minutes: 10,
            min_length: 8,
        }
    }
}

/// 图形验证码策略（`auth.captcha`）：登录失败达到阈值后强制。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CaptchaConfig {
    /// 总开关，`false` 时完全不校验验证码。
    pub enabled: bool,
    /// 验证码字符数。
    pub length: u32,
    /// 答案有效期（秒）。
    pub ttl_secs: u64,
    /// 连续失败多少次后强制要求验证码；`0` 表示登录一律要。
    pub required_after: u32,
}

impl Default for CaptchaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            length: 4,
            ttl_secs: 120,
            required_after: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AuthConfig {
    /// JWT 签名/验签（认证设计 / 配置）。
    pub jwt: JwtConfig,
    pub access_ttl_min: u64,
    /// 刷新令牌有效期（天）。
    pub refresh_ttl_days: u64,
    /// 单账号最大会话数，`-1` 不限。
    pub max_sessions: i64,
    /// 授权缓存 TTL（秒）。角色变更时按版本号整体失效，TTL 兜底回收。
    pub authz_cache_ttl_secs: u64,
    /// 开发/测试用身份绑定验证码；未配置时绑定接口失败关闭（上线接真实短信/邮件通道）。
    pub identity_code: Option<String>,
    /// 密码策略（失败锁定 + 最小长度）。
    pub password: PasswordConfig,
    /// 图形验证码策略（失败后强制）。
    pub captcha: CaptchaConfig,
    /// 第三方登录（微信扫码 / 通用 OIDC）。
    pub oauth: OAuthConfig,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt: JwtConfig::default(),
            access_ttl_min: 15,
            refresh_ttl_days: 7,
            max_sessions: -1,
            authz_cache_ttl_secs: 300,
            identity_code: None,
            password: PasswordConfig::default(),
            captcha: CaptchaConfig::default(),
            oauth: OAuthConfig::default(),
        }
    }
}

/// 跨域（第 4 层 `cors`）：默认关闭（浏览器同源或走网关反向代理时不需要），
/// 需要放行独立域名的前端时再开。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CorsConfig {
    pub enabled: bool,
    /// 允许的源（`scheme://host[:port]`）；写 `*` 表示任意源（不能与 `allow_credentials` 同时用）。
    pub allowed_origins: Vec<String>,
    /// 是否允许携带凭证（Cookie）；置 true 时不能用 `*`。
    pub allow_credentials: bool,
    /// 预检结果缓存秒数。
    pub max_age_secs: u64,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_origins: Vec::new(),
            allow_credentials: false,
            max_age_secs: 3600,
        }
    }
}

/// 限流（第 7 层 `rate_limit`）。
///
/// 单 IP 维度用进程内令牌桶（`governor`）；租户维度取套餐的 `api_qps_limit`（多租户设计），
/// 需要身份，故在鉴权之后生效。两者都可在配置里关闭或调参。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RateLimitConfig {
    /// 是否启用限流层（关掉即完全放行，含租户配额）。
    pub enabled: bool,
    /// 单 IP 每秒配额；`0` 不限。
    pub per_ip_qps: u32,
    /// 单 IP 令牌桶容量（允许的瞬时突发，取 `max(per_ip_burst, per_ip_qps)`）。
    pub per_ip_burst: u32,
    /// 租户 `api_qps_limit` 的本地缓存 TTL（秒），避免每请求查库。
    pub tenant_cache_ttl_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            per_ip_qps: 200,
            per_ip_burst: 400,
            tenant_cache_ttl_secs: 60,
        }
    }
}

/// 文件存储（`storage`）。
///
/// `driver` 决定后面这些字段里哪些有意义：本地/内存只用 `root`，对象存储只用
/// 桶与凭证。校验（`Config::validate`）按驱动分开查，缺参在启动期就报错，
/// 而不是等第一次上传拿到一个 403。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct StorageConfig {
    /// 驱动：`local` / `rustfs` / `oss` / `s3`（本层默认 `local`）。
    pub driver: String,
    /// 本地驱动的根目录；对象存储驱动忽略。
    pub root: String,
    /// 上传大小上限（MB）。
    pub max_upload_mb: usize,
    /// 桶名（rustfs / oss / s3 必填）。
    pub bucket: String,
    /// 服务端点。rustfs 与自建 S3 网关必填（没有公网默认值可推）；
    /// `s3` 留空则走 AWS 的 region 解析。
    pub endpoint: String,
    /// 区域（S3 签名 v4 需要；rustfs 这类自建网关填 `auto`）。
    pub region: String,
    /// 访问密钥 ID（对象存储必填）。
    pub access_key_id: String,
    /// 访问密钥（「敏感值一律来自环境变量」——用 `CICADA_STORAGE_ACCESS_KEY_SECRET` 注入）。
    pub access_key_secret: Secret<String>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            driver: "local".to_string(),
            root: "./data/upload".to_string(),
            max_upload_mb: 20,
            bucket: String::new(),
            endpoint: String::new(),
            region: String::new(),
            access_key_id: String::new(),
            access_key_secret: Secret::new(String::new()),
        }
    }
}

/// 定时任务（`jobs`）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct JobConfig {
    /// 是否启用调度运行时（触发侧选主 + 执行侧消费者）。
    pub enabled: bool,
    /// 触发方式：`cron`（进程内 cron 生产，默认）/ `manual`（只接受人工或外部触发）。
    pub trigger: String,
    /// 执行方式：`inline`（触发即执行，默认）/ `apalis`（投递到 Postgres 队列，由消费者执行）。
    pub execution: String,
    /// 队列命名空间；`execution = apalis` 时作为 apalis 的 `job_type` 隔离不同应用。
    pub queue: String,
    /// 实例标识，缺省时进程启动生成；写入执行日志与选主 token。
    pub instance_id: Option<String>,
    /// 选主租约 TTL（秒），续期周期为其 1/3。
    pub leader_ttl_secs: u64,
    /// 未成为 leader 时的重试间隔（秒）。
    pub follower_retry_secs: u64,
    /// 任务未显式声明超时时的兜底值（秒），0 表示不限。
    pub default_timeout_secs: u64,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger: "cron".to_string(),
            execution: "inline".to_string(),
            queue: "cicada::jobs".to_string(),
            instance_id: None,
            leader_ttl_secs: 30,
            follower_retry_secs: 10,
            default_timeout_secs: 0,
        }
    }
}

/// 日志保留期与归档：日志表按月分区，到期先归档再删分区。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AuditConfig {
    /// 保留天数（默认 180）。按整月判定：只有**整月**都早于 `now - 天数` 才处理。
    pub retention_days: i64,
    /// 分区窗口往后预铺的月数；往前补的月数取它的一半（至少 1）。
    pub horizon_months: i32,
    /// 是否真的归档并删分区。关掉时只报告，不删——设计上反对「直接删除」。
    pub archive: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            retention_days: 180,
            horizon_months: 24,
            archive: true,
        }
    }
}

/// 可观测：日志恒走 stdout，链路按 OTLP 上报。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// OTLP collector 地址，如 `http://127.0.0.1:43180`；`None` 表示不导出链路。
    pub otlp_endpoint: Option<String>,
    /// 上报的服务名（资源属性 `service.name`）。
    pub service_name: String,
    /// 采样比例（0.0–1.0）。默认全采，高流量环境可降采。
    pub sample_ratio: f64,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: None,
            service_name: "cicada".to_string(),
            sample_ratio: 1.0,
        }
    }
}

/// 审计发件箱 relay：`audit` 写 `outbox_event`，relay 投给属主表。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OutboxConfig {
    /// 是否起后台投递任务（关=事件只攒不投，用于排障或外部 relay）。
    pub enabled: bool,
    /// 扫描间隔（毫秒）。
    pub poll_interval_ms: u64,
    /// 单轮最多取多少条事件。
    pub batch: u32,
    /// 同一事件的最大尝试次数，超过则盖章放弃并打 error 日志。
    pub max_attempts: i32,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_ms: 500,
            batch: 200,
            max_attempts: 5,
        }
    }
}

/// 代码生成器（`gen`）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GenConfig {
    /// 是否启用代码生成接口（生产可关）。
    pub enabled: bool,
    /// 生成代码默认落到的模块名（`module_name`），用户可在导入后逐个改写。
    pub module_name: String,
    /// 默认作者，写入生成文件的头部注释与 `gen_table.function_author`。
    pub author: String,
    /// 前端/接口路径前缀（`route_prefix` + `/{module}/{business}`）。
    pub route_prefix: String,
    /// 模板覆写目录：优先于内置模板（项目 `templates/` > 内置）。
    pub template_dir: Option<String>,
    /// 表名必须命中这些前缀之一才允许导入（空表示不限制前缀）。安全闸门。
    pub allowed_prefixes: Vec<String>,
    /// 生成命名时要去掉的表名前缀（按最长匹配）；只影响命名，不参与准入。
    pub strip_prefixes: Vec<String>,
    /// 追加的拒绝表名（框架表已内置拒绝，这里用于项目自己的黑名单）。
    pub deny_tables: Vec<String>,
    /// 单次导入的表数量上限（防止一次拖入整个库）。
    pub max_import_tables: usize,
}

impl Default for GenConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            module_name: "business".to_string(),
            author: "cicada".to_string(),
            route_prefix: "/api/v1".to_string(),
            template_dir: None,
            allowed_prefixes: Vec::new(),
            strip_prefixes: Vec::new(),
            deny_tables: Vec::new(),
            max_import_tables: 20,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub db: DbConfig,
    /// RLS 兜底开关（默认关，按表开启）。
    pub rls: RlsConfig,
    pub redis: RedisConfig,
    pub tenant: TenantConfig,
    pub auth: AuthConfig,
    /// 跨域（第 4 层）。
    pub cors: CorsConfig,
    /// 限流层（第 7 层）。
    pub rate_limit: RateLimitConfig,
    /// 可观测。
    pub telemetry: TelemetryConfig,
    /// 日志保留期与归档。
    pub audit: AuditConfig,
    pub storage: StorageConfig,
    pub jobs: JobConfig,
    /// 审计发件箱投递。
    pub outbox: OutboxConfig,
    /// 配置键仍是 `gen`（与 一致），字段名避开 Rust 2024 保留字。
    #[serde(rename = "gen")]
    pub codegen: GenConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            db: DbConfig {
                url: Secret::new("postgres://cicada:password@localhost:5432/cicada".to_string()),
                max_connections: default_max_connections(),
                min_connections: 0,
                acquire_timeout_secs: 8,
                schema: "public".to_string(),
                slow_query_ms: default_slow_query_ms(),
            },
            redis: RedisConfig::default(),
            rls: RlsConfig::default(),
            tenant: TenantConfig::default(),
            auth: AuthConfig::default(),
            cors: CorsConfig::default(),
            rate_limit: RateLimitConfig::default(),
            telemetry: TelemetryConfig::default(),
            audit: AuditConfig::default(),
            storage: StorageConfig::default(),
            jobs: JobConfig::default(),
            outbox: OutboxConfig::default(),
            codegen: GenConfig::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("解析配置失败：{0}")]
    Parse(String),
    #[error("读取配置失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("缺少数据库连接串（db.url 或 DATABASE_URL）")]
    MissingDbUrl,
    /// 配置「缺项、越界、URL 非法直接 fail fast」。消息里带**字段路径**，
    /// 因为启动失败时人在容器日志里，不在编辑器里。
    #[error("配置取值非法：{0}")]
    Invalid(String),
}

/// 环境变量前缀（配置优先级最高一档）。
pub const ENV_PREFIX: &str = "CICADA_";

/// 选 profile 的环境变量：`CICADA_PROFILE=prod` → 读 `config/prod.yaml`（配置第三档）。
pub const PROFILE_ENV: &str = "CICADA_PROFILE";

/// 配置的三档文件（优先级从低到高：default < {profile} < local）。
pub const DEFAULT_FILE: &str = "config/default.yaml";
pub const LOCAL_FILE: &str = "config/local.yaml";

/// 深合并：`over` 覆盖 `base`。对象递归合并，数组与标量整体替换
/// （`cors.allowed_origins` 这种列表，「追加」从来不是运维想要的语义）。
fn merge(base: &mut serde_json::Value, over: serde_json::Value) {
    match (base, over) {
        (serde_json::Value::Object(here), serde_json::Value::Object(there)) => {
            for (key, value) in there {
                match here.get_mut(&key) {
                    Some(child) if child.is_object() && value.is_object() => merge(child, value),
                    Some(child) => *child = value,
                    None => {
                        here.insert(key, value);
                    }
                }
            }
        }
        (here, value) => *here = value,
    }
}

/// 读一个 yaml 文件并展开 `${VAR}` 占位符；文件不存在返回 `Ok(None)`。
fn read_layer(
    path: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<serde_json::Value>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ConfigError::Io(e)),
    };
    let Some(text) = text else { return Ok(None) };
    let expanded = expand_placeholders(&text, env);
    let value: serde_json::Value =
        serde_yaml::from_str(&expanded).map_err(|e| ConfigError::Parse(format!("{path}: {e}")))?;
    // 空文件（或只有注释）在 YAML 里是 null，不能进合并。
    Ok(if value.is_null() { None } else { Some(value) })
}

/// 把 `CICADA_STORAGE_MAX_UPLOAD_MB` 这类名字解析成配置树里的键路径。
///
/// 难点是 `_` 既是层级分隔符也是键名的一部分（`max_upload_mb`）。所以按
/// 「当前层能匹配上的最长键」贪心下降：`STORAGE` + `MAX_UPLOAD_MB` 能走通，
/// 拆成 `MAX` + `UPLOAD_MB` 走不通，就不会被错认成别的键。
fn resolve_path(node: &serde_json::Value, tokens: &[&str]) -> Option<Vec<String>> {
    if tokens.is_empty() {
        return Some(Vec::new());
    }
    let map = node.as_object()?;
    for take in (1..=tokens.len()).rev() {
        let key = tokens[..take].join("_").to_lowercase();
        // 最长键匹配不上就退一格：`SERVER_PORT` 的整段 `server_port` 不是键名，
        // 直接返回 None 会让所有两层以上的覆写全军覆没。
        let Some(child) = map.get(&key) else { continue };
        if take == tokens.len() {
            return Some(vec![key]);
        }
        let tail = resolve_path(child, &tokens[take..])?;
        let mut path = vec![key];
        path.extend(tail);
        return Some(path);
    }
    None
}

/// 把 yaml 里的 `${NAME}` 占位符换成环境变量值（敏感值一律来自环境变量）。
///
/// 未设置的环境变量**原样留着**而不是报错：`config/default.yaml` 里那些开发用字面量
/// 不该因为谁没导出某个变量就起不来；真缺关键值时，连不上库会在启动阶段以
/// 依赖故障的形式暴露，比在这里炸一句「占位符没展开」更好定位。
pub fn expand_placeholders(text: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match env(name) {
                    Some(v) => out.push_str(&v),
                    None => out.push_str(&rest[start..start + 2 + end + 1]),
                }
                rest = &after[end + 1..];
            }
            // 只有 `${` 没有收尾：当普通文本处理。
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

impl Config {
    /// 单段 YAML 直接解析（不做分层与 env 覆写）：给测试与「已经有树」的调用方用。
    pub fn from_yaml_str(s: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(s).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// 配置的完整优先级：
    /// 环境变量 > `config/local.yaml`（gitignore）> `config/{profile}.yaml` >
    /// `config/default.yaml` > 代码默认值。
    ///
    /// 实现方式是「代码默认值当底 → 逐层 YAML 深合并 → 环境变量覆写 → 校验」，
    /// 而不是「读到哪个文件就用哪个」：后者会让 `default.yaml` 一旦存在就盖住
    /// `local.yaml` 与 profile，而配置排的是五档，不是三档互斥。
    /// 容器镜像刻意不烘 `default.yaml`（见 Dockerfile 末段），所以「一个文件都没有」
    /// 这条路径也走同一次合并——底就是代码默认值，开发与容器行为不分叉。
    pub fn load() -> Result<Self, ConfigError> {
        let env = |k: &str| std::env::var(k).ok();
        let mut paths = vec![DEFAULT_FILE.to_string()];
        let profile = env(PROFILE_ENV).filter(|p| !p.trim().is_empty());
        if let Some(profile) = &profile {
            paths.push(format!("config/{}.yaml", profile.trim()));
        }
        paths.push(LOCAL_FILE.to_string());
        let vars: Vec<(String, String)> = std::env::vars().collect();
        let cfg = Self::assemble(&paths, &vars, &env)?;
        // 「启动时校验长度与格式，拒绝弱密钥（如默认示例值）」：这条只在
        // `CICADA_PROFILE=prod` 时生效。开发环境与容器冒烟跑的就是示例值，
        // 把它们一起拦掉只会让人绕过 profile 而不是修密钥。
        if profile
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case("prod"))
        {
            cfg.enforce_production_secrets()?;
        }
        Ok(cfg)
    }

    /// 生产环境的密钥策略：示例值与过短的共享密钥一律拒绝启动。
    pub fn enforce_production_secrets(&self) -> Result<(), ConfigError> {
        let jwt = &self.auth.jwt;
        let secret = jwt.private_key.expose_secret().trim();
        if secret.eq_ignore_ascii_case(WEAK_JWT_SAMPLE) || secret.eq_ignore_ascii_case("change-me")
        {
            return Err(ConfigError::Invalid(
                "auth.jwt.private_key: 仍是仓库里的示例值，生产必须换成自己的密钥（EdDSA 或 ≥32 字符的 HS256 密钥）".to_string(),
            ));
        }
        if jwt.alg.trim().eq_ignore_ascii_case("HS256") && secret.len() < MIN_JWT_SECRET_LEN {
            return Err(ConfigError::Invalid(format!(
                "auth.jwt.private_key: HS256 密钥至少 {MIN_JWT_SECRET_LEN} 字符（当前 {}）；或者改用 EdDSA",
                secret.len()
            )));
        }
        Ok(())
    }

    /// 分层加载的实现体。`vars` 显式传入而不是就地读进程环境：
    /// `std::env::set_var` 在 edition 2024 里是 `unsafe`，测优先级不该要求测试改进程状态。
    fn assemble(
        paths: &[String],
        vars: &[(String, String)],
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let mut root =
            serde_json::to_value(Config::default()).expect("代码默认值必然可序列化成配置树");
        // 底树里的连接串刻意抹掉：`DbConfig::default()` 那条是开发机的地址，
        // 留着它，「三层都没配」就会静默连上本地库，而不是按配置的 fail fast 报错。
        root["db"]["url"] = serde_json::Value::String(String::new());
        for path in paths {
            if let Some(layer) = read_layer(path, env)? {
                merge(&mut root, layer);
            }
        }
        for name in env_overlay(&mut root, vars) {
            // 忽略而不是失败：别人 export 的 `CICADA_*`（如 CI 用的 CICADA_OPENAPI_BLESS）
            // 不该让服务起不来。但仍然说出来——环境变量写错键名是最难自查的一类错误。
            tracing::warn!(variable = %name, "CICADA_* 环境变量没有对应的配置项，已忽略");
        }
        let mut cfg: Config = serde_path_to_error::deserialize(root)
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        // 这两个具名变量比 `CICADA_DB_URL` 更早存在，也更要紧：连接串单独一档。
        if let Some(url) = env("DATABASE_URL") {
            cfg.db.url = Secret::new(url);
        }
        if let Some(url) = env("REDIS_URL") {
            cfg.redis.url = Secret::new(url);
        }
        if cfg.db.url.expose_secret().is_empty() {
            return Err(ConfigError::MissingDbUrl);
        }
        cfg.validate()?;
        Ok(cfg)
    }
}

/// 按目标当前的类型转换环境变量值。
///
/// 只认 bool / 整数 / 浮点 / 字符串四种：配置项没有结构体级的覆写需求，
/// 而「按 YAML 里已有的形状解释字符串」正是运维手改一个变量的心智模型。
fn coerce(target: &serde_json::Value, raw: &str) -> Option<serde_json::Value> {
    if target.is_boolean() {
        return match raw.trim() {
            "1" | "true" | "TRUE" | "yes" | "on" => Some(serde_json::Value::Bool(true)),
            "0" | "false" | "FALSE" | "no" | "off" => Some(serde_json::Value::Bool(false)),
            _ => None,
        };
    }
    if target.is_i64() || target.is_u64() {
        return raw.trim().parse::<i64>().ok().map(serde_json::Value::from);
    }
    if target.is_f64() {
        return raw.trim().parse::<f64>().ok().map(serde_json::Value::from);
    }
    if target.is_null() {
        // 默认值是 null 的项（如 `telemetry.otlp_endpoint`）：先看能不能当数，再当字符串。
        if let Ok(v) = raw.trim().parse::<i64>() {
            return Some(serde_json::Value::from(v));
        }
        return Some(serde_json::Value::String(raw.to_string()));
    }
    Some(serde_json::Value::String(raw.to_string()))
}

/// 用 `CICADA_*` 环境变量覆写配置树；返回被忽略的变量名（供调用方 warn）。
///
/// 拆成显式传入键值对而不是直接读进程环境：`std::env::set_var` 在 edition 2024 里是
/// `unsafe`，测优先级不该要求测试去改全局状态。
pub fn env_overlay(root: &mut serde_json::Value, vars: &[(String, String)]) -> Vec<String> {
    let mut ignored = Vec::new();
    for (name, value) in vars {
        let Some(rest) = name.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        // `CICADA_PROFILE` 只用来挑文件，配置树里没有对应字段。
        if rest.eq_ignore_ascii_case("PROFILE") {
            continue;
        }
        let tokens: Vec<&str> = rest.split('_').filter(|t| !t.is_empty()).collect();
        if tokens.is_empty() {
            continue;
        }
        // 先只读地找路径，再改写：两者同时要借用根节点。
        let Some(path) = resolve_path(root, &tokens) else {
            ignored.push(name.clone());
            continue;
        };
        if !set_at(root, &path, value) {
            ignored.push(name.clone());
        }
    }
    ignored
}

/// 沿 `path` 走到叶子并按目标当前类型落值；类型不匹配或路径断了都返回 false。
fn set_at(node: &mut serde_json::Value, path: &[String], raw: &str) -> bool {
    let Some((head, rest)) = path.split_first() else {
        return false;
    };
    let Some(child) = node.get_mut(head.as_str()) else {
        return false;
    };
    if rest.is_empty() {
        match coerce(child, raw) {
            Some(v) => {
                *child = v;
                true
            }
            None => false,
        }
    } else {
        set_at(child, rest, raw)
    }
}

impl Config {
    /// 配置「启动时做一次完整校验，错误信息给出具体字段路径」。
    ///
    /// 只管**越界**与**自相矛盾**两类：serde 已经管了缺项与类型。
    /// 自相矛盾的样本是配置自己点名的 `cors: "*" + allow_credentials`——
    /// 浏览器会拒收，但服务端启动时看起来完全正常，直到前端登录不上才发现。
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |field: &str, why: &str| ConfigError::Invalid(format!("{field}: {why}"));
        if self.server.port == 0 {
            return Err(bad("server.port", "不能为 0"));
        }
        if self.server.body_limit_mb == 0 {
            return Err(bad(
                "server.body_limit_mb",
                "不能为 0（等于拒绝一切带体请求）",
            ));
        }
        // `db.max_connections == 0` 是合法值：含义是「按 CPU 核数定」（数据层约定）。
        if self.db.acquire_timeout_secs == 0 {
            return Err(bad(
                "db.acquire_timeout_secs",
                "不能为 0（拿不到连接就立刻全失败）",
            ));
        }
        check_url(
            "db.url",
            self.db.url.expose_secret(),
            &["postgres", "postgresql"],
        )?;
        check_url(
            "redis.url",
            self.redis.url.expose_secret(),
            &["redis", "rediss"],
        )?;
        // JWT：先查形状（算法、必备字段、kid 唯一）。策略性的「弱密钥」判断在
        // `enforce_production_secrets`——那是「这台机器算不算生产」的问题，不是形状问题。
        let jwt = &self.auth.jwt;
        let alg = jwt.alg.trim().to_ascii_uppercase();
        if !matches!(alg.as_str(), "EDDSA" | "HS256") {
            return Err(bad(
                "auth.jwt.alg",
                &format!("只能是 EdDSA 或 HS256，收到 {:?}", jwt.alg),
            ));
        }
        if jwt.private_key.expose_secret().trim().is_empty() {
            return Err(bad(
                "auth.jwt.private_key",
                "不能为空（签名没有密钥就没有可验的东西）",
            ));
        }
        if jwt.kid.trim().is_empty() {
            return Err(bad("auth.jwt.kid", "不能为空：kid 是多密钥轮换的寻址键"));
        }
        if alg == "EDDSA" && jwt.public_key.expose_secret().trim().is_empty() {
            return Err(bad(
                "auth.jwt.public_key",
                "EdDSA 必须同时给出公钥：私钥不能用于验签（「公钥本地验签」也依赖它）",
            ));
        }
        {
            let mut seen = std::collections::HashSet::new();
            seen.insert(jwt.kid.trim().to_string());
            for key in &jwt.keys {
                if key.kid.trim().is_empty() {
                    return Err(bad("auth.jwt.keys[].kid", "不能为空"));
                }
                if !seen.insert(key.kid.trim().to_string()) {
                    return Err(bad(
                        "auth.jwt.keys",
                        &format!(
                            "kid {:?} 重复：同一个 kid 指向两把钥匙无法判定用哪把验签",
                            key.kid
                        ),
                    ));
                }
                let key_alg = if key.alg.trim().is_empty() {
                    alg.clone()
                } else {
                    key.alg.trim().to_ascii_uppercase()
                };
                if !matches!(key_alg.as_str(), "EDDSA" | "HS256") {
                    return Err(bad(
                        "auth.jwt.keys[].alg",
                        &format!("只能是 EdDSA 或 HS256，收到 {:?}", key.alg),
                    ));
                }
                if key.key.expose_secret().trim().is_empty() && key_alg == "HS256" {
                    return Err(bad("auth.jwt.keys[].key", "HS256 的验签密钥不能为空"));
                }
            }
        }
        if self.auth.access_ttl_min == 0 {
            return Err(bad(
                "auth.access_ttl_min",
                "不能为 0（issued 即过期，没人能调用）",
            ));
        }
        if self.auth.refresh_ttl_days == 0 {
            return Err(bad("auth.refresh_ttl_days", "不能为 0"));
        }
        if self.auth.password.min_length == 0 {
            return Err(bad("auth.password.min_length", "不能为 0"));
        }
        if self.cors.enabled
            && self.cors.allow_credentials
            && self.cors.allowed_origins.iter().any(|o| o == "*")
        {
            return Err(bad(
                "cors.allowed_origins",
                "通配来源与 allow_credentials 不能同时开（浏览器一律拒绝，且这是凭据外泄的形状）",
            ));
        }
        if !(0.0..=1.0).contains(&self.telemetry.sample_ratio) {
            return Err(bad(
                "telemetry.sample_ratio",
                &format!("必须在 0..=1，收到 {}", self.telemetry.sample_ratio),
            ));
        }
        if self.outbox.poll_interval_ms == 0
            || self.outbox.batch == 0
            || self.outbox.max_attempts == 0
        {
            return Err(bad(
                "outbox",
                "poll_interval_ms / batch / max_attempts 都必须 > 0",
            ));
        }
        if self.audit.retention_days == 0 || self.audit.horizon_months == 0 {
            return Err(bad("audit", "retention_days / horizon_months 都必须 > 0"));
        }
        if self.storage.max_upload_mb == 0 {
            return Err(bad("storage.max_upload_mb", "不能为 0"));
        }
        if !matches!(
            self.storage.driver.as_str(),
            "local" | "rustfs" | "oss" | "s3" | "memory"
        ) {
            return Err(bad(
                "storage.driver",
                &format!(
                    "未知驱动 {:?}（可用：local / rustfs / oss / s3 / memory）",
                    self.storage.driver
                ),
            ));
        }
        // 对象存储的必填项在这里查齐（配置「缺项 fail fast」）：驱动选错了通常是
        // 部署时少 export 了一个变量，而这类问题在第一次上传时才炸最难定位。
        if self.storage.needs_object_store() {
            if self.storage.bucket.trim().is_empty() {
                return Err(bad(
                    "storage.bucket",
                    &format!("驱动 {} 必须指定桶名", self.storage.driver),
                ));
            }
            if self.storage.access_key_id.trim().is_empty()
                || self
                    .storage
                    .access_key_secret
                    .expose_secret()
                    .trim()
                    .is_empty()
            {
                return Err(bad(
                    "storage.access_key_id",
                    &format!(
                        "驱动 {} 需要访问密钥（access_key_id + CICADA_STORAGE_ACCESS_KEY_SECRET）",
                        self.storage.driver
                    ),
                ));
            }
            // 端点能不能省，取决于有没有公网默认值可推：只有 AWS S3 能。
            let inferable = self.storage.driver == "s3" && !self.storage.region.trim().is_empty();
            if self.storage.endpoint.trim().is_empty() && !inferable {
                return Err(bad(
                    "storage.endpoint",
                    &format!(
                        "驱动 {} 必须指定端点{}（rustfs 是自建服务，没有可推的默认地址）",
                        self.storage.driver,
                        if self.storage.driver == "s3" {
                            "或 region"
                        } else {
                            ""
                        }
                    ),
                ));
            }
        } else if self.storage.driver == "local" && self.storage.root.trim().is_empty() {
            return Err(bad("storage.root", "本地驱动必须指定根目录"));
        }
        Ok(())
    }
}

impl DbConfig {
    /// 池上限的实际取值：`0` 展开成 `CPU 核数 × 2`。
    ///
    /// `Db::connect` 与 `/metrics` 的 `db_pool_connections{state="max"}` 都走这里，
    /// 不然监控看到的上限和真正生效的上限会是两个数。
    pub fn resolved_max_connections(&self) -> u32 {
        if self.max_connections > 0 {
            return self.max_connections;
        }
        std::thread::available_parallelism()
            .map(|n| (n.get() as u32) * 2)
            .unwrap_or(4)
    }
}

impl StorageConfig {
    /// 该驱动是否需要对象存储参数（桶 / 端点 / 凭证）。
    pub fn needs_object_store(&self) -> bool {
        matches!(self.driver.as_str(), "rustfs" | "oss" | "s3")
    }
}

/// URL 形状校验：scheme 必须在白名单里。只查 scheme 与「非空」，不查可达性
/// ——可达性是启动连接阶段的事，在这里查会让无网的 CI 与离线环境起不来。
fn check_url(field: &str, value: &str, schemes: &[&str]) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::Invalid(format!("{field}: 不能为空")));
    }
    let Some((scheme, _)) = value.split_once("://") else {
        return Err(ConfigError::Invalid(format!(
            "{field}: 不是 URL（缺 ://）：{value:?}"
        )));
    };
    if !schemes.iter().any(|s| s.eq_ignore_ascii_case(scheme)) {
        return Err(ConfigError::Invalid(format!(
            "{field}: scheme {scheme:?} 不在允许的 {:?} 里",
            schemes
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_section_parses_into_codegen_field() {
        let cfg = Config::from_yaml_str(
            "db: { url: \"postgres://u:p@localhost/db\" }\n\
             gen:\n  enabled: false\n  module_name: shop\n  strip_prefixes: [biz_]\n",
        )
        .expect("解析配置");
        assert!(!cfg.codegen.enabled);
        assert_eq!(cfg.codegen.module_name, "shop");
        assert_eq!(cfg.codegen.strip_prefixes, vec!["biz_".to_string()]);
        // 未写 gen 段时用默认值。
        let bare = Config::from_yaml_str("db: { url: \"postgres://u:p@localhost/db\" }").unwrap();
        assert!(bare.codegen.enabled);
    }

    #[test]
    fn secret_masks_debug() {
        let s = Secret::new("postgres://cicada:hunter2@localhost/cicada".to_string());
        let dbg = format!("{s:?}");
        assert!(dbg.contains("REDACTED"));
        assert!(!dbg.contains("hunter2"));
        assert_eq!(
            s.expose_secret(),
            "postgres://cicada:hunter2@localhost/cicada"
        );
    }

    #[test]
    fn placeholders_come_from_env_and_unknown_stay_put() {
        let env = |k: &str| match k {
            "DB_PASSWORD" => Some("s3cret".to_string()),
            _ => None,
        };
        let text = "url: \"postgres://cicada:${DB_PASSWORD}@localhost/cicada\"\nother: ${NOT_SET}";
        let out = expand_placeholders(text, &env);
        assert!(out.contains("cicada:s3cret@"), "{out}");
        assert!(out.contains("${NOT_SET}"), "未设置的变量要原样留着：{out}");
    }

    #[test]
    fn env_beats_the_config_file_layer() {
        // 覆盖发生在文件之后：容器里就是靠这条把连接串指到 compose 的网络里。
        let dir = std::env::temp_dir().join(format!("cicada-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("default.yaml");
        std::fs::write(
            &path,
            "db: { url: \"postgres://file-host:5432/filedb\", schema: public }\n\
             redis: { url: \"redis://file-host:6379/0\" }\n",
        )
        .unwrap();
        let cfg = Config::assemble(&[path.to_str().unwrap().to_string()], &[], &|k| match k {
            "DATABASE_URL" => Some("postgres://env-host:5432/envdb".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.db.url.expose_secret(), "postgres://env-host:5432/envdb");
        // 没被 env 覆盖的那条仍来自文件（且经过占位符展开这一层）。
        assert_eq!(cfg.redis.url.expose_secret(), "redis://file-host:6379/0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_falls_back_to_env_and_still_requires_db_url() {
        let err = Config::assemble(
            &["/nonexistent/cicada-default.yaml".to_string()],
            &[],
            &|_| None,
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::MissingDbUrl), "{err}");
        let cfg = Config::assemble(
            &["/nonexistent/cicada-default.yaml".to_string()],
            &[],
            &|k| (k == "DATABASE_URL").then(|| "postgres://h/db".to_string()),
        )
        .unwrap();
        assert_eq!(cfg.db.url.expose_secret(), "postgres://h/db");
    }

    #[test]
    fn parses_minimal_yaml_with_defaults() {
        let cfg = Config::from_yaml_str("db:\n  url: postgres://cicada:pw@localhost:5432/cicada\n")
            .unwrap();
        // 代码默认值是「按 CPU 核数定」（数据层约定），显式写了数字才按数字。
        assert_eq!(cfg.db.max_connections, 0);
        assert!(cfg.db.resolved_max_connections() >= 2);
        let pinned =
            Config::from_yaml_str("db: { url: \"postgres://u@h/db\", max_connections: 7 }")
                .unwrap();
        assert_eq!(pinned.db.resolved_max_connections(), 7);
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.tenant.default_id, 1);
    }

    /// 配置五档优先级的完整断言：代码默认值 < default.yaml < {profile}.yaml < local.yaml < env。
    ///
    /// 这一条盯的是曾经真实存在过的 bug：只看「哪个文件先存在」，于是 `default.yaml`
    /// 一放下，`local.yaml` 与 profile 就成了摆设——而配置排的是五档，不是三档互斥。
    #[test]
    fn five_layers_stack_in_the_documented_order() {
        let dir = std::env::temp_dir().join(format!("cicada-layers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let written = |name: &str, port: u16| {
            let path = dir.join(name);
            std::fs::write(
                &path,
                format!(
                    "server: {{ port: {port}, listen_addr: \"0.0.0.0\" }}\n\
                     db: {{ url: \"postgres://u:p@localhost:5432/cicada\" }}\n"
                ),
            )
            .unwrap();
            path.to_str().unwrap().to_string()
        };
        let default = written("default.yaml", 8081);
        let prod = written("prod.yaml", 8082);
        let local = written("local.yaml", 8083);
        let paths = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        let no_env = |_: &str| None;

        // 单层
        let only_default = Config::assemble(&paths(&[&default]), &[], &no_env).unwrap();
        assert_eq!(only_default.server.port, 8081);
        // 逐层叠加，后者赢
        let with_profile = Config::assemble(&paths(&[&default, &prod]), &[], &no_env).unwrap();
        assert_eq!(with_profile.server.port, 8082);
        let with_local =
            Config::assemble(&paths(&[&default, &prod, &local]), &[], &no_env).unwrap();
        assert_eq!(with_local.server.port, 8083);
        // 最后一档：环境变量盖住全部文件
        let with_env = Config::assemble(
            &paths(&[&default, &prod, &local]),
            &[("CICADA_SERVER_PORT".to_string(), "9999".to_string())],
            &no_env,
        )
        .unwrap();
        assert_eq!(with_env.server.port, 9999);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 仓库里那两份配置文件必须真能「加载 + 校验」通过。
    ///
    /// 这是分层加载器与样例文件之间唯一的连线：`default.yaml` 写错一个键名或漏一项必填，
    /// 只有启动时才暴露，而那时人在容器日志里、不在编辑器里。样例文件里的 `${VAR}`
    /// 占位符未展开也算通过（校验只看形状，可达性是连接阶段的事）。
    #[test]
    fn the_shipped_config_files_load_and_validate() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/");
        for name in ["default.yaml", "default.yaml.example"] {
            let cfg = Config::assemble(&[format!("{root}{name}")], &[], &|_| None)
                .unwrap_or_else(|e| panic!("{name} 加载失败：{e}"));
            assert_eq!(
                cfg.storage.driver, "local",
                "{name}：开发默认应当是本地驱动"
            );
            assert!(
                cfg.db.url.expose_secret().starts_with("postgres://"),
                "{name}：连接串形状不对"
            );
            assert!(cfg.server.port > 0, "{name}");
            // 配置的样例文件必须带 auth.jwt（开发默认 HS256 + 示例值；生产怎么换写在注释里）。
            assert_eq!(cfg.auth.jwt.alg, "HS256", "{name}");
            assert_eq!(cfg.auth.jwt.kid, "1", "{name}");
        }
    }

    /// 环境变量 → 配置树键名的映射规则。难点是 `_` 既是层级分隔符又是键名的一部分。
    #[test]
    fn env_names_map_to_paths_even_when_keys_contain_underscores() {
        let mut root = serde_json::to_value(Config::default()).unwrap();
        let ignored = env_overlay(
            &mut root,
            &[
                ("CICADA_SERVER_PORT".to_string(), "1234".to_string()),
                // storage.max_upload_mb：`MAX_UPLOAD_MB` 必须整体当键名，不能被拆成 MAX + ...
                ("CICADA_STORAGE_MAX_UPLOAD_MB".to_string(), "77".to_string()),
                (
                    "CICADA_AUTH_PASSWORD_MIN_LENGTH".to_string(),
                    "14".to_string(),
                ),
                (
                    "CICADA_TELEMETRY_OTLP_ENDPOINT".to_string(),
                    "http://otlp:4318".to_string(),
                ),
                ("CICADA_RATE_LIMIT_ENABLED".to_string(), "false".to_string()),
                // 三段键名（slow + query + ms）：贪心下降要能走通 `db` → `slow_query_ms`。
                ("CICADA_DB_SLOW_QUERY_MS".to_string(), "500".to_string()),
                // 不存在的项与类型不匹配的值都要被报告为「忽略」
                ("CICADA_NOT_A_SECTION".to_string(), "1".to_string()),
                (
                    "CICADA_SERVER_LISTEN_ADDR".to_string(),
                    "ignored".to_string(),
                ),
            ],
        );
        assert_eq!(root["server"]["port"], serde_json::json!(1234));
        assert_eq!(root["storage"]["max_upload_mb"], serde_json::json!(77));
        assert_eq!(
            root["auth"]["password"]["min_length"],
            serde_json::json!(14)
        );
        assert_eq!(
            root["telemetry"]["otlp_endpoint"],
            serde_json::json!("http://otlp:4318")
        );
        assert_eq!(root["rate_limit"]["enabled"], serde_json::json!(false));
        assert_eq!(root["db"]["slow_query_ms"], serde_json::json!(500));
        assert!(
            ignored.iter().any(|v| v == "CICADA_NOT_A_SECTION"),
            "未知项应当被报告：{ignored:?}"
        );
        assert!(
            ignored.iter().any(|v| v == "CICADA_SERVER_LISTEN_ADDR"),
            "`server` 命中了但叶子不存在，同样应当被报告：{ignored:?}"
        );
        // 合法的那条不应该出现在 ignored 里（`CICADA_SERVER_PORT` 等）。
        assert!(
            !ignored.iter().any(|v| v == "CICADA_SERVER_PORT"),
            "已知项不该被忽略：{ignored:?}"
        );
    }

    /// 认证设计 / 配置：`auth.jwt` 的形状与轮换配置。
    #[test]
    fn jwt_section_parses_and_validates_its_keyring() {
        let cfg = Config::from_yaml_str(
            "db: { url: \"postgres://u@h/db\" }\n\nauth:\n  jwt:\n    alg: EdDSA\n    kid: \"2026-10\"\n    private_key: \"-----BEGIN PRIVATE KEY-----\\nabc\\n-----END PRIVATE KEY-----\"\n    public_key: \"pubpem\"\n    keys:\n      - kid: \"2026-09\"\n        key: \"oldpub\"\n      - kid: \"2026-08\"\n        alg: HS256\n        key: \"a-shared-secret-of-some-length-here\"\n",
        )
        .expect("解析 auth.jwt");
        assert_eq!(cfg.auth.jwt.alg, "EdDSA");
        assert_eq!(cfg.auth.jwt.keys.len(), 2);
        cfg.validate().expect("EdDSA + 公钥 + 两个旧 kid");

        // EdDSA 不给公钥：验不了签的配置不该静默起来。
        let no_pub = Config::from_yaml_str(
            "db: { url: \"postgres://u@h/db\" }\nauth: { jwt: { alg: EdDSA, kid: \"1\", private_key: \"pem\" } }",
        )
        .unwrap();
        let err = no_pub.validate().unwrap_err().to_string();
        assert!(err.contains("auth.jwt.public_key"), "{err}");

        // kid 重复：同一个 kid 指向两把钥匙，验签时无法判定用哪把。
        let dup = Config::from_yaml_str(
            r#"db: { url: "postgres://u@h/db" }
auth:
  jwt:
    alg: HS256
    kid: "1"
    private_key: "s3cret-s3cret-s3cret-s3cret-1234"
    keys:
      - kid: "1"
        key: "other"
"#,
        );
        match dup {
            Ok(cfg) => {
                let err = cfg.validate().unwrap_err().to_string();
                assert!(err.contains("auth.jwt.keys"), "{err}");
            }
            Err(e) => panic!("配置应当能解析: {e}"),
        }

        // 未知算法直接拒。
        let bad_alg = Config::from_yaml_str(
            "db: { url: \"postgres://u@h/db\" }\nauth: { jwt: { alg: none, kid: \"1\", private_key: \"x\" } }",
        )
        .unwrap();
        let err = bad_alg.validate().unwrap_err().to_string();
        assert!(err.contains("auth.jwt.alg"), "{err}");
    }

    /// 「启动时…拒绝弱密钥（如默认示例值）」：这条只在 profile=prod 生效。
    #[test]
    fn production_startup_refuses_the_sample_and_short_secrets() {
        let mut cfg = Config::default();
        cfg.validate().expect("默认配置必须自检通过");
        // 默认值就是示例值：形状没问题，但当生产来用必须被拦。
        let err = cfg.enforce_production_secrets().unwrap_err().to_string();
        assert!(err.contains("示例值"), "{err}");

        cfg.auth.jwt.private_key = Secret::new("too-short".to_string());
        let err = cfg.enforce_production_secrets().unwrap_err().to_string();
        assert!(err.contains("32 字符"), "{err}");

        let long = "0123456789abcdef0123456789abcdef0123456789abcdef";
        cfg.auth.jwt.private_key = Secret::new(long.to_string());
        cfg.enforce_production_secrets()
            .expect("≥32 字符的 HS256 密钥算合格");

        // EdDSA 走的是 PEM 长度那一档：只要不是示例值、且给了公钥就通过形状校验。
        cfg.auth.jwt.alg = "EdDSA".to_string();
        cfg.auth.jwt.public_key =
            Secret::new("-----BEGIN PUBLIC KEY-----\nabc\n-----END PUBLIC KEY-----".to_string());
        cfg.validate().expect("EdDSA 形状齐全");
        cfg.enforce_production_secrets().expect("生产允许 EdDSA");
    }

    /// 校验只拦「越界」与「自相矛盾」，且消息里带字段路径。
    #[test]
    fn validate_names_the_offending_field() {
        let mut cfg = Config::default();
        cfg.db.url = Secret::new("postgres://u@h/db".to_string());
        cfg.validate().expect("默认配置必须自检通过");

        let mut cors = cfg.clone();
        cors.cors.enabled = true;
        cors.cors.allow_credentials = true;
        cors.cors.allowed_origins = vec!["*".to_string()];
        let err = cors.validate().unwrap_err().to_string();
        assert!(err.contains("cors.allowed_origins"), "{err}");

        let mut driver = cfg.clone();
        driver.storage.driver = "minio".to_string();
        let err = driver.validate().unwrap_err().to_string();
        assert!(err.contains("storage.driver"), "{err}");

        // 对象存储的必填项（文件存储的驱动切换不能把缺参带到第一次上传）。
        let mut s3 = cfg.clone();
        s3.storage.driver = "s3".to_string();
        let err = s3.validate().unwrap_err().to_string();
        assert!(err.contains("storage.bucket"), "{err}");
        s3.storage.bucket = "cicada".to_string();
        s3.storage.region = "ap-northeast-1".to_string();
        s3.storage.access_key_id = "key".to_string();
        s3.storage.access_key_secret = Secret::new("secret".to_string());
        s3.validate().expect("AWS S3 给 region + 桶 + 凭证就够");

        // rustfs 是自建服务：没有可推的默认端点，必须显式给。
        let mut rustfs = s3.clone();
        rustfs.storage.driver = "rustfs".to_string();
        let err = rustfs.validate().unwrap_err().to_string();
        assert!(err.contains("storage.endpoint"), "{err}");
        rustfs.storage.endpoint = "http://rustfs:9000".to_string();
        rustfs.validate().expect("补齐端点后应当通过");

        let mut url = cfg.clone();
        url.redis.url = Secret::new("memcached://localhost:11211".to_string());
        let err = url.validate().unwrap_err().to_string();
        assert!(err.contains("redis.url"), "{err}");

        let mut ratio = cfg.clone();
        ratio.telemetry.sample_ratio = 2.0;
        let err = ratio.validate().unwrap_err().to_string();
        assert!(err.contains("telemetry.sample_ratio"), "{err}");
    }
}
