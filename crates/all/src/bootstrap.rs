//! 组装根骨架（/ 机制 1–2）。
//!
//! 使用者工程（`cicada new` 产出的 `my-app`）只写业务模块与配置，装配顺序、中间件链、
//! 基础设施连接与端口绑定都由这里完成；`apps/server` 用的是同一条路径，因此
//! 「模板工程能跑」与「框架自己能跑」不会分叉。
//!
//! ```ignore
//! let config = cicada::Config::load()?;
//! let _tracing = cicada::bootstrap::init_tracing_with("my_app=info", &config.telemetry)?;
//! let app = cicada::bootstrap::AppBuilder::new(config)
//!     .with_module(Box::new(business::demo::DemoModule))
//!     .with_job("my_app:cleanup", |_ctx| async { Ok(None) })
//!     .build()
//!     .await?;
//! app.serve().await?;
//! ```

use std::sync::Arc;
use std::time::Duration;

/// 框架自带任务「日志分区维护」的调用目标（`sys_job.invoke_target` 的取值）。
///
/// 种子行在 `cicada_migrations::seed`，handler 在这里注册——两头都写死同一个字符串会漂，
/// 所以 `crates/all/tests/framework_jobs.rs` 断言两边一致。
pub const JOB_LOGS_RETENTION: &str = "cicada:logs:retention";

/// 把文件存储适配成归档端口（归档到对象存储）。
///
/// 端口放在 `cicada-ports`、实现放在 `cicada-storage`，中间这座桥由组装根搭：
/// `system`（L2）不该为了写归档而去依赖 `storage`（L3）。
struct StorageArchive(Arc<dyn Storage>);

#[async_trait::async_trait]
impl ArchiveStore for StorageArchive {
    async fn put(&self, key: &str, body: &[u8]) -> Result<(), cicada_ports::ArchiveError> {
        self.0
            .put(key, body)
            .await
            .map_err(|e: StorageError| cicada_ports::ArchiveError::Storage(e.to_string()))
    }
}

use axum::Router;
use cicada_audit::{Outbox, OutboxAuditSink};
use cicada_auth::{
    CaptchaParams, CaptchaService, HttpOAuthClient, IdentityVerifier, OAuthService,
    RedisCaptchaStore, RedisLockoutStore, RedisOAuthStateStore, RedisSessionStore,
    RejectAllVerifier, SessionStore, SnapshottedSessionStore, StaticCodeVerifier, TokenService,
};
use cicada_infra::{Cache, Config, Db};
use cicada_ports::ArchiveStore;
use cicada_ports::{
    EventRouter, JobContext, JobError, JobHandler, JobRegistry, JobRuntime, MenuRegistry,
};
use cicada_rbac::{AuthzCache, RedisAuthzStore};
use cicada_storage::{DownloadGrantStore, RedisGrantStore, Storage, StorageError};
use cicada_system::{ApiKeyService, LocalDeptDirectory, TenantQuota};
use cicada_web::ratelimit::RateLimit;
use cicada_web::{Module, router_with_menus, state::AppState};

/// 组装失败的原因：都是启动期就能判定的硬错误（fail fast）。
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("配置加载失败：{0}")]
    Config(#[from] cicada_infra::ConfigError),
    #[error("数据库连接失败：{0}")]
    Db(#[from] cicada_infra::DbError),
    #[error("Redis 连接失败：{0}")]
    Cache(#[from] cicada_infra::CacheError),
    #[error("RLS 开启失败：{0}")]
    Rls(#[from] cicada_infra::rls::RlsError),
    #[error("定时任务运行时装配失败：{0}")]
    Job(#[from] JobError),
    #[error("文件存储驱动装配失败：{0}")]
    Storage(#[from] cicada_storage::StorageError),
    #[error("模块装配失败：{0}")]
    Assemble(String),
    #[error("监听地址绑定失败：{0}")]
    Bind(#[from] std::io::Error),
}

/// 组装根：收集使用者声明的事实（模块、任务处理器），再一次性建依赖。
pub struct AppBuilder {
    config: Config,
    modules: Vec<Box<dyn Module>>,
    registry: JobRegistry,
    /// 菜单表属主提供的注册端口；缺席时用 `cicada-system` 的实现（模块边界）。
    menu_registry: Option<Arc<dyn MenuRegistry>>,
}

impl AppBuilder {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            modules: Vec::new(),
            registry: JobRegistry::new(),
            menu_registry: None,
        }
    }

    /// 挂一个业务模块（模块只声明事实，顺序由装配器推导）。
    pub fn with_module(mut self, module: Box<dyn Module>) -> Self {
        self.modules.push(module);
        self
    }

    /// 批量挂模块。
    pub fn with_modules(mut self, modules: Vec<Box<dyn Module>>) -> Self {
        self.modules.extend(modules);
        self
    }

    /// 替换菜单注册端口（默认 `cicada_system::DbMenuRegistry`）。
    /// 微服务形态下把菜单归属到独立服务时，在这里换 adapter。
    pub fn with_menu_registry(mut self, registry: Arc<dyn MenuRegistry>) -> Self {
        self.menu_registry = Some(registry);
        self
    }

    /// 登记一个定时任务处理器。调用目标是显式 key，不做字符串反射。
    pub fn with_job<F, Fut>(mut self, key: &'static str, handler: F) -> Self
    where
        F: Fn(JobContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Option<String>, JobError>> + Send,
    {
        self.registry.register_fn(key, handler);
        self
    }

    /// 登记一个自定义任务处理器（对象形式）。
    pub fn with_job_handler(
        mut self,
        key: &'static str,
        handler: impl JobHandler + 'static,
    ) -> Self {
        self.registry.register(key, handler);
        self
    }

    /// 建依赖 → 挂模块 → 产出可直接 `serve` 的应用。
    pub async fn build(self) -> Result<App, BootstrapError> {
        let AppBuilder {
            config,
            modules,
            mut registry,
            menu_registry,
        } = self;

        // 指标 recorder：进程内装一次；同进程重复 build（测试）时忽略即可。
        if let Err(e) = cicada_infra::metrics::install() {
            tracing::debug!(error = %e, "指标 recorder 未重新安装");
        }

        let db = Db::connect(&config.db).await?;
        // RLS 兜底：默认关闭，按表开启；表名在启动期按白名单校验。
        if !config.rls.tables.is_empty() {
            cicada_infra::rls::enable(db.conn(), &config.rls.tables).await?;
            tracing::info!(tables = ?config.rls.tables, "RLS 兜底已开启");
        }

        let cache = Cache::connect(config.redis.url.expose_secret()).await?;
        // 在线会话快照：Redis 仍是唯一权威源，`sys_user_online` 只是
        // write-behind 出去的审计快照——判定路径（存在性 / 上限 / 踢出）一概不看它。
        let snapshot_store = Arc::new(SnapshottedSessionStore::new(
            Arc::new(RedisSessionStore::new(cache.clone())),
            db.clone(),
        ));
        let sessions: Arc<dyn SessionStore> = snapshot_store.clone();
        let lockout = Arc::new(RedisLockoutStore::new(cache.clone()));
        // 图形验证码：连续失败 required_after 次后登录强制校验。
        let captcha = Arc::new(CaptchaService::new(
            Arc::new(RedisCaptchaStore::new(cache.clone())),
            CaptchaParams {
                enabled: config.auth.captcha.enabled,
                length: config.auth.captcha.length,
                ttl_secs: config.auth.captcha.ttl_secs,
                required_after: config.auth.captcha.required_after,
            },
        ));
        // Idempotency-Key 防重复提交：执行位与结果重放都放 Redis。
        let idempotency = Arc::new(cicada_web::idempotency::IdempotencyService::new(Arc::new(
            cicada_web::idempotency::RedisIdempotencyStore::new(cache.clone()),
        )));
        let authz = Arc::new(AuthzCache::new(
            Arc::new(RedisAuthzStore::new(cache.clone())),
            config.auth.authz_cache_ttl_secs,
        ));
        // 认证设计：签名优先 EdDSA、支持 kid 多密钥轮换；密钥配错在这里就失败（fail fast），
        // 而不是等第一个登录请求拿到一个验不过的 token。
        let tokens =
            TokenService::from_jwt(&config.auth.jwt, config.auth.access_ttl_min as i64 * 60)
                .map_err(|e| BootstrapError::Assemble(e.to_string()))?;
        // 审计：生产者只写 outbox_event，落 system 的日志表由 relay 投递完成。
        let outbox = Outbox::new(db.clone());
        let audit = Arc::new(OutboxAuditSink::from_outbox(outbox.clone()));
        let events = Arc::new(EventRouter::new());
        events.register(Arc::new(cicada_system::SysLogStore::new(db.clone())));

        // 文件存储：按 `storage.driver` 选驱动——local / memory 原生，
        // rustfs / oss / s3 走 opendal。业务侧只拿 `dyn Storage`，换后端不改调用方。
        let storage: Arc<dyn Storage> = cicada_storage::build(&config.storage)?;
        // 本地驱动没有预签名能力，短时链接由服务端签发（文件存储）；凭证放 Redis，
        // 实例不持状态（部署），重启与扩容都不会让已签发的链接变宽。
        let grants: Arc<dyn DownloadGrantStore> = Arc::new(RedisGrantStore::new(cache.clone()));
        // 业务缓存走同一个 Redis 句柄；`Cache` 实现了 `KeyValueStore`，
        // 策略（单飞 / 抖动 / 空结果标记 / 版本号）在 `cicada-ports::KvCache` 那一侧。
        let sys_cache = cicada_ports::KvCache::new(Arc::new(cache.clone()));

        // 身份绑定验证码：未配置通道时失败关闭。
        let identity_verifier: Arc<dyn IdentityVerifier> = match &config.auth.identity_code {
            Some(code) => Arc::new(StaticCodeVerifier::new(code.clone())),
            None => Arc::new(RejectAllVerifier),
        };

        // 第三方登录：未配置 provider 时相关接口一律 404。
        let oauth = Arc::new(OAuthService::new(
            Arc::new(HttpOAuthClient::new()),
            Arc::new(RedisOAuthStateStore::new(cache.clone())),
            config.auth.oauth.providers.clone(),
            config.auth.oauth.state_ttl_secs,
            config.auth.oauth.ticket_ttl_secs,
        ));
        if !config.auth.oauth.providers.is_empty() {
            tracing::info!(
                providers = ?config.auth.oauth.providers.iter().map(|p| p.code.as_str()).collect::<Vec<_>>(),
                "第三方登录已启用"
            );
        }

        // 启动即对账在线快照：进程被杀、TTL 自然过期这两条路径不会经过 `revoke`，
        // 只有拿 Redis 的在线集合反查才能把库里挂着的「在线」收成「已退出」。
        match snapshot_store.snapshot().reconcile_with(&*sessions).await {
            Ok(report) if report.refreshed + report.closed > 0 => {
                tracing::info!(
                    refreshed = report.refreshed,
                    closed = report.closed,
                    "在线会话快照已按 Redis 对账"
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "在线会话快照对账失败（不影响服务）"),
        }

        // 日志分区生命周期：启动先补齐窗口，之后交给 JOB_LOGS_RETENTION。
        let retention = cicada_system::LogRetention::new(db.clone());
        let archiver: Arc<dyn ArchiveStore> = Arc::new(StorageArchive(storage.clone()));
        let audit_cfg = config.audit.clone();
        if let Err(e) = retention
            .ensure_partitions(
                audit_cfg.horizon_months.max(1) / 2,
                audit_cfg.horizon_months.max(1),
            )
            .await
        {
            tracing::warn!(error = %e, "启动时补分区失败，等维护任务重试");
        }
        registry.register_fn(JOB_LOGS_RETENTION, move |_ctx| {
            let retention = retention.clone();
            let archiver = archiver.clone();
            let cfg = audit_cfg.clone();
            async move {
                let report = retention
                    .run(
                        cfg.retention_days,
                        cfg.horizon_months,
                        cfg.archive,
                        &*archiver,
                    )
                    .await
                    .map_err(|e| JobError::Execution(e.to_string()))?;
                Ok(Some(format!(
                    "归档 {} 个分区，删除 {} 个，{} 字节，跳过 {} 个",
                    report.archived.len(),
                    report.dropped.len(),
                    report.bytes,
                    report.skipped
                )))
            }
        });

        let jobs = build_job_runtime(&config, db.clone(), cache.clone(), registry).await?;

        // 限流（第 7 层）：单 IP 令牌桶 + 租户套餐 QPS，配额在启动期由配置固定。
        let rate_limit = Arc::new(RateLimit::new(&config.rate_limit));

        let state = AppState {
            db: db.clone(),
            config: config.clone(),
            tokens,
            sessions,
            lockout,
            captcha,
            idempotency,
            storage,
            grants,
            sys_cache,
            dept_directory: Arc::new(LocalDeptDirectory::new(db.clone())),
            authz,
            audit,
            identity_verifier,
            oauth,
            notify: Arc::new(cicada_notify::NotifyService::new(
                db.clone(),
                // 技术选型：邮件渠道的选型是 lettre/SMTP；其余通道走 HTTP 网关。
                Arc::new(cicada_notify::RoutingSender::new()),
            )),
            api_keys: Arc::new(ApiKeyService::new(db.clone())),
            quota: Arc::new(TenantQuota::new(db.clone())),
            rate_limit,
            job_runtime: jobs.runtime,
            cache_inspector: Arc::new(cicada_monitor::RedisCacheInspector::new(cache)),
        };

        // 菜单声明落库（模块边界）：模块只声明，写 sys_menu 的始终是属主注册的端口。
        let (router, menus) = router_with_menus(state.clone(), modules);
        if !menus.is_empty() {
            let registry = menu_registry.unwrap_or_else(|| {
                Arc::new(cicada_system::DbMenuRegistry::new(db.clone())) as Arc<dyn MenuRegistry>
            });
            registry
                .upsert(&menus)
                .await
                .map_err(|e| BootstrapError::Assemble(format!("模块菜单落库失败：{e}")))?;
        }
        let mut tasks = jobs.tasks;
        if config.outbox.enabled {
            tasks.push(spawn_outbox_relay(outbox, events, config.outbox.clone()));
        } else {
            tracing::warn!("outbox.enabled = false：审计事件只攒在 outbox_event，不投递");
        }
        Ok(App {
            state,
            router,
            port: config.server.port,
            _job_tasks: tasks,
        })
    }
}

/// 任务运行时装配结果：运行时句柄 + 需要保活的后台任务。
struct JobParts {
    runtime: Arc<JobRuntime>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

/// 组装完成的应用：状态 + 路由 + 监听端口。
pub struct App {
    pub state: AppState,
    pub router: Router,
    port: u16,
    _job_tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl App {
    /// 配置里的监听端口。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 起服务：绑 `0.0.0.0:<port>`，挂 `ConnectInfo` 供限流层取对端 IP，
    /// 直到收到停止信号（Ctrl-C / SIGTERM）走完整优雅停机。
    pub async fn serve(self) -> Result<(), BootstrapError> {
        let addr = format!("0.0.0.0:{}", self.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!("cicada listening on http://{addr}");
        let grace = Duration::from_secs(self.state.config.server.graceful_shutdown_secs);
        serve_with_drain(listener, self.router, shutdown_signal(), grace).await?;
        // 数据层约定「关库前 `close().await` 等待连接归还」：不给池关闭的机会，退出时
        // 还挂着的连接由 PG 单方面断开，日志里会留一段和本进程无关的断连告警。
        if let Err(e) = self.state.db.close().await {
            tracing::warn!(error = %e, "关闭数据库连接池失败");
        }
        Ok(())
    }
}

/// 在给定监听器上服务到 `stop` 完成，并给在途请求最多 `grace` 的收尾时间。
///
/// 拆成独立函数有两个理由：停机路径要能被测试（不必真发信号），
/// 而使用者的模板工程可能想注入自己的停止来源（比如一个管理接口的 `/shutdown`）。
pub async fn serve_with_drain(
    listener: tokio::net::TcpListener,
    router: Router,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
    grace: Duration,
) -> Result<(), BootstrapError> {
    // 一个信号要让「开始排空」和「排空超时看门狗」两头都看见。用 watch 而不是
    // `Notify`：后者的 `notify_waiters()` 只叫醒已在等的读者，会丢唤醒。
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        stop.await;
        let _ = tx.send(true);
    });
    let serving = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(wait_until_stopped(rx.clone()));
    // `biased`：serve 已经排空返回时优先取它，别把正常退出报告成「超时强制」。
    tokio::select! {
        biased;
        res = serving => res?,
        _ = async {
            wait_until_stopped(rx).await;
            tokio::time::sleep(grace).await;
        } => {
            tracing::warn!(grace = ?grace, "优雅停机到点仍有在途请求，强制退出");
        }
    }
    Ok(())
}

/// 等停止标志被置起。先 `borrow_and_update` 再 `changed`，因此信号早于本 future
/// 开始等待也不会漏掉；发送端被丢弃同样视为「已停止」。
async fn wait_until_stopped(mut rx: tokio::sync::watch::Receiver<bool>) {
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// 停止信号：Ctrl-C 与 SIGTERM。
///
/// SIGTERM 是容器里的正路（`docker stop` 与 K8s 下线都发这个），只处理 Ctrl-C
/// 的服务在容器里等不到优雅停机，会被宽限期后的 SIGKILL 打断。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            // 装不上信号处理器（受限环境）时不该变成「永不退出」，交给 Ctrl-C 那一路。
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("收到停止信号，开始优雅停机");
}

/// 起审计发件箱的投递任务。
///
/// 循环体只做「排空一轮 + 睡到下一轮」；投递失败不会中断循环，毒事件由
/// `max_attempts` 兜住并打 error 日志。任务随进程存活，进程退出即结束。
fn spawn_outbox_relay(
    outbox: Outbox,
    events: Arc<EventRouter>,
    cfg: cicada_infra::OutboxConfig,
) -> tokio::task::JoinHandle<()> {
    tracing::info!(
        interval_ms = cfg.poll_interval_ms,
        batch = cfg.batch,
        handlers = ?events.registered(),
        "审计发件箱 relay 已启动"
    );
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(
            cfg.poll_interval_ms.max(50),
        ));
        loop {
            ticker.tick().await;
            match outbox
                .drain_once(&events, cfg.batch, cfg.max_attempts)
                .await
            {
                Ok(report) if report.delivered > 0 => {
                    tracing::debug!(
                        delivered = report.delivered,
                        failed = report.failed,
                        abandoned = report.abandoned,
                        "发件箱 relay 投递"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "发件箱 relay 本轮失败"),
            }
        }
    })
}

/// 定时任务运行时的装配（`worker` feature 打开时用真实运行时，否则退化为只接受人工触发）。
#[cfg(feature = "worker")]
async fn build_job_runtime(
    config: &Config,
    db: Db,
    cache: Cache,
    registry: JobRegistry,
) -> Result<JobParts, BootstrapError> {
    use cicada_ports::DistributedLock;
    use cicada_worker::{JobExecutor, JobRuntimeConfig, RedisDistributedLock, build_job_runtime};

    let job_cfg = JobRuntimeConfig::from_config(&config.jobs);
    let executor = Arc::new(JobExecutor::new(
        cicada_jobs::JobStore::new(db),
        Arc::new(registry),
        Arc::new(RedisDistributedLock::new(cache.clone())),
        job_cfg.instance_id.clone(),
        job_cfg.default_timeout_secs,
    ));
    let leader_lock: Arc<dyn DistributedLock> = Arc::new(RedisDistributedLock::new(cache));
    let handle = build_job_runtime(
        &job_cfg,
        executor,
        leader_lock,
        config.db.url.expose_secret(),
    )
    .await?;
    if config.jobs.enabled {
        tracing::info!(
            trigger = ?job_cfg.trigger,
            execution = ?job_cfg.execution,
            "定时任务运行时已启动"
        );
    }
    Ok(JobParts {
        runtime: handle.runtime,
        tasks: handle.tasks,
    })
}

/// 未启用 `worker` feature：只装配人工触发入口，后台 cron 与队列缺席。
#[cfg(not(feature = "worker"))]
async fn build_job_runtime(
    _config: &Config,
    _db: Db,
    _cache: Cache,
    _registry: JobRegistry,
) -> Result<JobParts, BootstrapError> {
    tracing::warn!("未启用 `worker` feature：定时任务只接受人工触发");
    Ok(JobParts {
        runtime: Arc::new(JobRuntime::new(Arc::new(cicada_ports::NoopRunSink), None)),
        tasks: Vec::new(),
    })
}
