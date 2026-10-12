//! `AppState`：随请求流转的只读共享状态。

use std::sync::Arc;

use cicada_audit::AuditSink;
use cicada_auth::{
    CaptchaService, IdentityVerifier, LockoutStore, OAuthService, SessionStore, TokenService,
};
use cicada_infra::{Config, Db};
use cicada_notify::NotifyService;
use cicada_ports::{CacheInspector, DeptDirectory, JobRuntime, KvCache};
use cicada_rbac::AuthzCache;
use cicada_storage::{DownloadGrantStore, Storage};
use cicada_system::{ApiKeyService, TenantQuota};

use crate::ratelimit::RateLimit;

/// 组装根构造、handler 经 `State<AppState>` 抽取。`Db`/`Config` 都是廉价克隆
/// （内部 Arc），因此整体可直接 `Clone`；跨模块端口以 `Arc<dyn Trait>` 追加。
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Config,
    pub tokens: TokenService,
    pub sessions: Arc<dyn SessionStore>,
    /// 登录失败计数与账号锁定。
    pub lockout: Arc<dyn LockoutStore>,
    /// 登录图形验证码：连续失败后强制。
    pub captcha: Arc<CaptchaService>,
    /// `Idempotency-Key` 防重复提交：写接口按需开启。
    pub idempotency: Arc<crate::idempotency::IdempotencyService>,
    /// 文件存储驱动，按 `storage.driver` 选后端。
    pub storage: Arc<dyn Storage>,
    /// 一次性下载凭证：本地驱动签发短时链接用。
    pub grants: Arc<dyn DownloadGrantStore>,
    /// 业务缓存（参数字典 / 字典项 / 菜单树 / 租户信息）。
    /// 克隆廉价；测试夹具用 `KvCache::memory()`，不需要真 Redis。
    pub sys_cache: KvCache,
    pub dept_directory: Arc<dyn DeptDirectory>,
    /// 定时任务运行时：人工 `/run` 与定义变更后的触发侧对账入口。
    pub job_runtime: Arc<JobRuntime>,
    /// 缓存观测与按域失效：只读键空间视图，运维侧工具。
    pub cache_inspector: Arc<dyn CacheInspector>,
    /// 授权结果缓存，键含角色版本号。
    pub authz: Arc<AuthzCache>,
    /// 审计写入：操作日志 + 登录日志。
    pub audit: Arc<dyn AuditSink>,
    /// 身份绑定的归属校验（手机/邮箱验证码通道）。
    pub identity_verifier: Arc<dyn IdentityVerifier>,
    /// 第三方登录：授权地址、回调换身份、票据绑定。
    pub oauth: Arc<OAuthService>,
    /// 通知中心：渠道 + 模板 + 记录。
    pub notify: Arc<NotifyService>,
    /// 开放接口凭证：签发 / 校验 / 停用。
    pub api_keys: Arc<ApiKeyService>,
    /// 租户限额：席位与存储闸门。
    pub quota: Arc<TenantQuota>,
    /// 限流（第 7 层）：IP 维度 + 租户套餐 QPS。
    pub rate_limit: Arc<RateLimit>,
}
