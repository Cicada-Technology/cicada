//! 授权结果缓存：`user → roles → menus(perms) → 权限集合` 的落库读
//! 只在缓存未命中时发生。
//!
//! 缓存键含 `user_id` 与**角色版本号**：`cicada:authz:{tenant}:{user}:v{version}`，
//! 版本号存在 `cicada:permver:{tenant}`。角色 / 角色-菜单 / 用户-角色 / 菜单权限
//! 变更时调用 [`AuthzCache::invalidate_tenant`] 把版本号整体 +1，旧键随之失效
//! （并靠 TTL 自然回收），从而做到「角色或菜单变更时按版本号整体失效」。
//!
//! 缓存是纯优化：读写失败一律降级为直接查库，绝不让请求失败。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use cicada_infra::Cache;
use cicada_kernel::{TenantId, UserId};
use cicada_ports::jittered_ttl;

use crate::authorize::Authorization;

/// 授权缓存的存储缝：生产用 [`RedisAuthzStore`]，测试用 [`InMemoryAuthzStore`]。
#[async_trait]
pub trait AuthzStore: Send + Sync + 'static {
    async fn get(&self, key: &str) -> Option<String>;
    async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64);
    /// 读租户级角色版本号（缺失按 0）。
    async fn version(&self, tenant_id: i64) -> i64;
    /// 角色版本号自增，返回新值。
    async fn bump(&self, tenant_id: i64) -> i64;
}

/// Redis 实现。
#[derive(Clone)]
pub struct RedisAuthzStore {
    cache: Cache,
}

impl RedisAuthzStore {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl AuthzStore for RedisAuthzStore {
    async fn get(&self, key: &str) -> Option<String> {
        match self.cache.get(key).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, key, "读授权缓存失败，降级查库");
                None
            }
        }
    }

    async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64) {
        if let Err(e) = self.cache.set_ex(key, value, ttl_secs).await {
            tracing::warn!(error = %e, key, "写授权缓存失败");
        }
    }

    async fn version(&self, tenant_id: i64) -> i64 {
        match self.cache.get(&version_key(tenant_id)).await {
            Ok(Some(v)) => v.parse().unwrap_or(0),
            Ok(None) => 0,
            Err(e) => {
                tracing::warn!(error = %e, "读角色版本号失败，按 0 处理");
                0
            }
        }
    }

    async fn bump(&self, tenant_id: i64) -> i64 {
        match self.cache.incr(&version_key(tenant_id)).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "角色版本号自增失败");
                0
            }
        }
    }
}

/// 内存实现（测试 / 单进程降级）。
#[derive(Default)]
pub struct InMemoryAuthzStore {
    kv: Mutex<HashMap<String, String>>,
    versions: Mutex<HashMap<i64, i64>>,
}

impl InMemoryAuthzStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AuthzStore for InMemoryAuthzStore {
    async fn get(&self, key: &str) -> Option<String> {
        self.kv.lock().unwrap().get(key).cloned()
    }

    async fn set_ex(&self, key: &str, value: &str, _ttl_secs: u64) {
        self.kv
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
    }

    async fn version(&self, tenant_id: i64) -> i64 {
        *self.versions.lock().unwrap().get(&tenant_id).unwrap_or(&0)
    }

    async fn bump(&self, tenant_id: i64) -> i64 {
        let mut v = self.versions.lock().unwrap();
        let e = v.entry(tenant_id).or_insert(0);
        *e += 1;
        *e
    }
}

fn version_key(tenant_id: i64) -> String {
    format!("cicada:permver:{tenant_id}")
}

fn entry_key(tenant_id: i64, user_id: i64, version: i64) -> String {
    format!("cicada:authz:{tenant_id}:{user_id}:v{version}")
}

/// L1（进程内那一层）的容量与 TTL（缓存设计「短 TTL，防热点击穿」）。
///
/// 五秒够挡住对同一个用户的连续请求，又短到即使版本号机制哪天失灵，
/// 权限变更也只会晚五秒而不是晚到进程重启。
const LOCAL_CAPACITY: u64 = 4_096;
const LOCAL_TTL_SECS: u64 = 5;

/// 授权结果缓存。`Authorization` 可序列化，故直接存 JSON。
///
/// 两级：进程内 moka（短 TTL，防热点击穿）+ Redis（跨实例一致）。
/// 关键设计：**L1 的键里带着角色版本号，而版本号每次都从 Redis 读**。
/// 于是「变更即失效」仍然由 Redis 那一头保证（版本号一跳，L1 的键就再也对不上），
/// L1 只省掉「取权限集合正文」这一次往返与它的反序列化——加一级缓存没有换来
/// 任何越权窗口，这正是待确认项第 5 条里「版本号失效」那条路线在进程内的样子。
#[derive(Clone)]
pub struct AuthzCache {
    store: Arc<dyn AuthzStore>,
    ttl_secs: u64,
    local: moka::sync::Cache<String, Arc<Authorization>>,
    /// 单飞（缓存设计「防击穿用单飞加载」）：同一个「租户:用户」并发未命中时只算一次。
    flight: Arc<Mutex<HashMap<String, Arc<tokio::sync::OnceCell<Authorization>>>>>,
}

impl AuthzCache {
    pub fn new(store: Arc<dyn AuthzStore>, ttl_secs: u64) -> Self {
        let local = moka::sync::Cache::builder()
            .max_capacity(LOCAL_CAPACITY)
            .time_to_live(std::time::Duration::from_secs(LOCAL_TTL_SECS))
            .build();
        Self {
            store,
            ttl_secs,
            local,
            flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }

    /// 取缓存的授权结果（含数据范围、权限集合、超管标记、角色键）。
    pub async fn get(&self, tenant: TenantId, user_id: UserId) -> Option<Authorization> {
        let version = self.store.version(tenant.get()).await;
        let key = entry_key(tenant.get(), user_id.get(), version);
        if let Some(hit) = self.local.get(&key) {
            metrics::counter!(cicada_infra::metrics::CACHE_LOCAL_HITS, "domain" => "authz")
                .increment(1);
            return Some((*hit).clone());
        }
        metrics::counter!(cicada_infra::metrics::CACHE_LOCAL_MISSES, "domain" => "authz")
            .increment(1);
        let raw = self.store.get(&key).await?;
        match serde_json::from_str::<Authorization>(&raw) {
            Ok(a) => {
                self.local.insert(key, Arc::new(a.clone()));
                Some(a)
            }
            Err(e) => {
                tracing::warn!(error = %e, "授权缓存反序列化失败，降级查库");
                None
            }
        }
    }

    /// 写入授权结果。写入时重新读版本号，避免与并发失效竞争。
    pub async fn put(&self, tenant: TenantId, user_id: UserId, authz: &Authorization) {
        let Ok(raw) = serde_json::to_string(authz) else {
            return;
        };
        let version = self.store.version(tenant.get()).await;
        let key = entry_key(tenant.get(), user_id.get(), version);
        self.store
            .set_ex(&key, &raw, jittered_ttl(self.ttl_secs))
            .await;
        self.local.insert(key, Arc::new(authz.clone()));
    }

    /// 命中即返回；未命中则调用 `compute` 装载并回填。
    ///
    /// 未命中时同一个「租户:用户」的并发请求走单飞（缓存设计「防击穿用单飞加载」）：
    /// `OnceCell::get_or_init` 只让第一个调用者的装载 future 真正执行，其余等同一个结果
    /// ——它们自己的 future 还没开始跑就被丢弃，不会重复查库。落库失败会拿到空授权
    /// 而不是永远不返回，所以这条路上不存在「等不到值」的分支。
    pub async fn load_or_compute<F, Fut>(
        &self,
        tenant: TenantId,
        user_id: UserId,
        compute: F,
    ) -> Authorization
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Authorization>,
    {
        if let Some(a) = self.get(tenant, user_id).await {
            return a;
        }
        let flight_key = format!("{}:{}", tenant.get(), user_id.get());
        let cell = {
            let mut map = self.flight.lock().expect("单飞表不该中毒");
            map.entry(flight_key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };
        let loaded = cell
            .get_or_init(|| async {
                let authz = compute().await;
                self.put(tenant, user_id, &authz).await;
                authz
            })
            .await
            .clone();
        // 装载结束后摘掉这个 cell：留着的话，下一次权限变更后的首次未命中
        // 会直接拿到上一次的结果，等于把单飞变成了一层不过期的缓存。
        let mut map = self.flight.lock().expect("单飞表不该中毒");
        if map.get(&flight_key).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
            map.remove(&flight_key);
        }
        loaded
    }

    /// 角色版本号 +1：本租户内所有用户的授权缓存整体失效。
    ///
    /// 角色 / 角色-菜单 / 用户-角色 / 菜单权限变更后必须调用。
    pub async fn invalidate_tenant(&self, tenant: TenantId) {
        self.store.bump(tenant.get()).await;
        // 本实例的 L1 立刻清空：新键对不上是最终保证，但没必要让已经知道的旧值
        // 再占着内存与「一次误命中」的可能。
        self.local.invalidate_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorize::authorize;
    use cicada_kernel::RoleGrant;
    use cicada_ports::InMemoryDeptDirectory;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn sample_authz(super_admin: bool) -> Authorization {
        let dir = InMemoryDeptDirectory::new();
        authorize(
            TenantId::new(1),
            UserId::new(7),
            None,
            &[RoleGrant {
                role_id: cicada_kernel::RoleId::new(1),
                role_key: if super_admin { "admin" } else { "ops" }.to_string(),
                data_scope: cicada_kernel::DataScopeKind::All,
                dept_ids: Vec::new(),
            }],
            [cicada_kernel::Permission::new("system:user:add")],
            &dir,
        )
        .await
    }

    #[tokio::test]
    async fn hit_after_put() {
        let cache = AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60);
        let tenant = TenantId::new(1);
        let user = UserId::new(7);
        assert!(cache.get(tenant, user).await.is_none());

        let authz = sample_authz(true).await;
        cache.put(tenant, user, &authz).await;
        let got = cache.get(tenant, user).await.expect("应命中");
        assert!(got.is_super_admin);
        assert!(
            got.permissions
                .contains(&cicada_kernel::Permission::new("system:user:add"))
        );
    }

    #[tokio::test]
    async fn version_bump_invalidates_all_users() {
        let cache = AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60);
        let tenant = TenantId::new(1);
        let authz = sample_authz(false).await;
        for u in [7, 8] {
            cache.put(tenant, UserId::new(u), &authz).await;
        }
        assert!(cache.get(tenant, UserId::new(7)).await.is_some());

        cache.invalidate_tenant(tenant).await;
        assert!(cache.get(tenant, UserId::new(7)).await.is_none());
        assert!(cache.get(tenant, UserId::new(8)).await.is_none());
    }

    #[tokio::test]
    async fn load_or_compute_only_computes_once() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let cache = AuthzCache::new(Arc::new(InMemoryAuthzStore::new()), 60);
        let tenant = TenantId::new(1);
        let user = UserId::new(7);
        let calls = Arc::new(AtomicU32::new(0));
        for _ in 0..3 {
            let calls = calls.clone();
            cache
                .load_or_compute(tenant, user, || async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    sample_authz(true).await
                })
                .await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// 只数的存储缝：验证「L1 命中时真的没有再走 Redis」。
    #[derive(Default)]
    struct CountingStore {
        inner: InMemoryAuthzStore,
        payload_gets: AtomicUsize,
        version_reads: AtomicUsize,
    }

    impl CountingStore {
        fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait]
    impl AuthzStore for CountingStore {
        async fn get(&self, key: &str) -> Option<String> {
            self.payload_gets.fetch_add(1, Ordering::SeqCst);
            self.inner.get(key).await
        }
        async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64) {
            self.inner.set_ex(key, value, ttl_secs).await;
        }
        async fn version(&self, tenant_id: i64) -> i64 {
            self.version_reads.fetch_add(1, Ordering::SeqCst);
            self.inner.version(tenant_id).await
        }
        async fn bump(&self, tenant_id: i64) -> i64 {
            self.inner.bump(tenant_id).await
        }
    }

    /// 两级缓存：进程内那一层必须真的省掉后端的取正文往返。
    #[tokio::test]
    async fn the_local_layer_serves_repeats_without_touching_the_store() {
        let store = Arc::new(CountingStore::new());
        let cache = AuthzCache::new(store.clone(), 60);
        let (tenant, user) = (TenantId::new(1), UserId::new(7));
        cache.put(tenant, user, &sample_authz(true).await).await;
        let payload_gets = store.payload_gets.load(Ordering::SeqCst);

        for _ in 0..3 {
            assert!(
                cache
                    .get(tenant, user)
                    .await
                    .expect("应命中")
                    .is_super_admin
            );
        }
        assert_eq!(
            store.payload_gets.load(Ordering::SeqCst),
            payload_gets,
            "L1 命中时不该再取一次正文"
        );
        // 版本号每次仍然读后端：这是「变更即失效」的根据，缓存它就把安全属性换成性能了。
        assert!(store.version_reads.load(Ordering::SeqCst) >= 3);
    }

    /// 版本号一跳，L1 里那条旧值必须立刻不可达（权限模型的失效语义优先于命中率）。
    #[tokio::test]
    async fn a_version_bump_makes_the_local_layer_unreachable() {
        let store = Arc::new(InMemoryAuthzStore::new());
        let cache = AuthzCache::new(store, 60);
        let (tenant, user) = (TenantId::new(1), UserId::new(7));
        cache.put(tenant, user, &sample_authz(true).await).await;
        assert!(
            cache.get(tenant, user).await.is_some(),
            "先确认这一条是热的"
        );

        cache.invalidate_tenant(tenant).await;
        assert!(
            cache.get(tenant, user).await.is_none(),
            "版本号一跳之后，旧授权结果既不该从 Redis 也不该从 L1 被读到"
        );
    }

    /// 单飞（缓存设计防击穿）：并发未命中只允许一次装载，不是「各自查库」。
    ///
    /// 用 `join!` 而不是 8 个 `spawn` + 阻塞栅栏：栅栏会占住 runtime 的 worker 线程
    /// （4 个线程等 8 个到达者，直接死锁），而 `join!` 在同一任务里把 8 个 future
    /// 各poll到第一个 await 点，到达顺序确定、断言也就不靠睡眠赌运气。
    #[tokio::test]
    async fn concurrent_misses_compute_the_authorization_once() {
        let store = Arc::new(InMemoryAuthzStore::new());
        let cache = AuthzCache::new(store, 60);
        let (tenant, user) = (TenantId::new(1), UserId::new(4242));
        let calls = Arc::new(AtomicUsize::new(0));

        let one = |calls: Arc<AtomicUsize>, cache: AuthzCache| async move {
            cache
                .load_or_compute(tenant, user, || async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    // 装载慢一点，确保其余 7 个都停在同一个 cell 上等待。
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    sample_authz(true).await
                })
                .await
        };

        let results = tokio::join!(
            one(calls.clone(), cache.clone()),
            one(calls.clone(), cache.clone()),
            one(calls.clone(), cache.clone()),
            one(calls.clone(), cache.clone()),
            one(calls.clone(), cache.clone()),
            one(calls.clone(), cache.clone()),
            one(calls.clone(), cache.clone()),
            one(calls.clone(), cache.clone()),
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "8 个并发未命中应当只查一次库"
        );
        let (a0, a1, a2, a3, a4, a5, a6, a7) = results;
        for r in [a0, a1, a2, a3, a4, a5, a6, a7] {
            assert!(r.is_super_admin, "等待者也要拿到同一份授权结果");
        }

        // 单飞表必须被清空：留着它，下一次失效后的装载会直接读到上一次的旧结果。
        let leftover = cache
            .flight
            .lock()
            .expect("单飞表不该中毒")
            .get(&format!("{}:{}", tenant.get(), user.get()))
            .is_some();
        assert!(!leftover, "装载完成后要把自己从单飞表里摘掉");
    }
}
