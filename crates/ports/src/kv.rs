//! 通用键值缓存端口与缓存策略。
//!
//! 分成两层是刻意的：
//! - [`KeyValueStore`] 只是「字符串键值 + TTL + 自增」，生产实现是 Redis
//!   （`cicada_infra::Cache`），测试实现是这里的 [`InMemoryKeyValueStore`]；
//! - [`KvCache`] 是**策略**：单飞（防击穿）、TTL 抖动（防雪崩）、空结果短 TTL
//!   （防穿透）、版本号整域失效（缓存键里的 `v{version}`）。
//!
//! 策略放在端口这一侧而不是 Redis 实现里，理由是可测：没有 Redis 也能把
//! 「并发未命中只回源一次」「标记值不会被当成真值」这些行为断言出来。

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::OnceCell;

/// 键值存储的窄接口。所有方法都**不抛错**：缓存是纯优化，
/// 存储层抖动时调用方要的是「当没命中」，不是「请求失败」。
#[async_trait]
pub trait KeyValueStore: Send + Sync {
    async fn get(&self, key: &str) -> Option<String>;
    async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64);
    async fn incr(&self, key: &str) -> i64;
    async fn del(&self, key: &str);
}

/// 「源里确实没有」的标记值（缓存设计防穿透）。
///
/// 不写标记的话，一个不存在的键每被请求一次就打一次数据库——缓存对「不存在」
/// 反而是负收益。标记用不可能出现在业务 JSON 里的控制字符开头。
pub const EMPTY_MARKER: &str = "\u{0}cicada:empty";

/// 空结果的 TTL（缓存设计「对空结果做短 TTL 缓存」）：短到「刚建好的对象立刻可见」，
/// 又长到能挡住一阵探测。
pub const EMPTY_TTL_SECS: u64 = 30;

/// TTL 抖动比例（缓存设计防雪崩）。
pub const JITTER_RATIO: u64 = 10;

/// 把 TTL 打散成 `ttl × (1 ± 10%)`，避免同一批键（同一次版本自增后）在同一秒集体过期。
///
/// 熵源取系统时钟的纳秒位而不是引一个 `rand`：这里要的是「不要把过期时间排成齐射」，
/// 不是密码学随机。短 TTL（<10s）不抖——抖完没剩下多少有效期，等于自伤命中率。
pub fn jittered_ttl(ttl_secs: u64) -> u64 {
    if ttl_secs < 10 {
        return ttl_secs.max(1);
    }
    let spread = ttl_secs * JITTER_RATIO / 100;
    let clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    ttl_secs - spread + clock % (spread * 2 + 1)
}

/// 单飞表：`键 -> 那一次回源的共享结果`。
type FlightMap = Arc<Mutex<HashMap<String, Arc<OnceCell<Option<String>>>>>>;

/// 带策略的缓存句柄：包一个 [`KeyValueStore`]，加上单飞表。
///
/// 克隆廉价（内部 `Arc`），因此可以像 `Db` 一样随状态流转。
#[derive(Clone)]
pub struct KvCache {
    kv: Arc<dyn KeyValueStore>,
    /// 单飞表（缓存设计防击穿）：同一个键的并发未命中只回源一次。
    flight: FlightMap,
}

impl KvCache {
    pub fn new(kv: Arc<dyn KeyValueStore>) -> Self {
        Self {
            kv,
            flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 测试与单进程降级用的内存实现。
    pub fn memory() -> Self {
        Self::new(Arc::new(InMemoryKeyValueStore::new()))
    }

    /// 读透过缓存：命中即返回，未命中回源并写回。
    ///
    /// - **单飞**：同一个键的并发未命中只让第一个调用者真正回源，其余等同一个结果
    ///   （它们自己的 future 还没开始跑就被丢弃，不会重复查库）；
    /// - **TTL 抖动**：写回时打散过期时间（防雪崩）；
    /// - **空结果短 TTL**：`load` 返回 `None` 也写一个标记（防穿透），于是「不存在」
    ///   这个结论同样只值 30 秒，而不是永久挡住新数据。
    ///
    /// `load` 返回 `None` 的含义是「确认没有」；回源本身失败时调用方也应返回 `None`
    /// ——但要知道这会给这次失败上一次 30 秒的「不存在」税。
    pub async fn read_through<F, Fut>(&self, key: &str, ttl_secs: u64, load: F) -> Option<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Option<String>>,
    {
        if let Some(raw) = self.kv.get(key).await {
            return if raw == EMPTY_MARKER { None } else { Some(raw) };
        }
        let cell = {
            let mut map = self.flight.lock().expect("单飞表不该中毒");
            map.entry(key.to_string())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let value = cell
            .get_or_init(|| async {
                let loaded = load().await;
                let (payload, ttl) = match &loaded {
                    Some(v) => (v.clone(), jittered_ttl(ttl_secs)),
                    None => (EMPTY_MARKER.to_string(), EMPTY_TTL_SECS),
                };
                self.kv.set_ex(key, &payload, ttl).await;
                loaded
            })
            .await
            .clone();
        // 装载结束后摘掉这个 cell：留着的话，下一次失效后的首次未命中会直接读到
        // 上一次的结果，等于把单飞变成一层永不过期的缓存。
        let mut map = self.flight.lock().expect("单飞表不该中毒");
        if map.get(key).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
            map.remove(key);
        }
        value
    }

    /// 原样读一个键（含标记值），给需要「先验形状再用」的调用方用。
    ///
    /// 类型化缓存（`cicada-system` 的 `cached_json`）得先自己判断「这个键存的是不是
    /// 当前结构」，因为结构不符时要**清掉脏值再回源**——而 `read_through` 一旦命中
    /// 就不会把闭包留给我们重跑。
    pub async fn peek_raw(&self, key: &str) -> Option<String> {
        self.kv.get(key).await
    }

    /// 丢掉一个键：结构不符的脏值不该一直坏着。
    pub async fn forget(&self, key: &str) {
        self.kv.del(key).await;
    }

    /// 域版本号（缓存键里的 `v{version}`）：读不到按 0，绝不因为一次存储抖动
    /// 就把整域判成过期。
    pub async fn domain_version(&self, key: &str) -> i64 {
        self.kv
            .get(key)
            .await
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// 版本号自增：整域失效（缓存设计「变更即整体失效」）。
    ///
    /// 键里带着版本号，所以自增之后旧条目自然不可达，剩下的交给 TTL 回收——
    /// 不需要遍历删除，也就不需要「扫全库」这种破坏性操作。
    pub async fn bump_domain_version(&self, key: &str) -> i64 {
        self.kv.incr(key).await
    }
}

/// 内存实现：测试夹具与「没有 Redis 的单进程」（如 CLI 的一次性任务）。
#[derive(Default)]
pub struct InMemoryKeyValueStore {
    inner: Mutex<HashMap<String, Entry>>,
}

#[derive(Clone)]
struct Entry {
    value: String,
    expires_at: Option<std::time::Instant>,
}

impl InMemoryKeyValueStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前存活的键数（测试与观测用；顺带清掉已过期的条目）。
    pub fn live_keys(&self) -> usize {
        let mut map = self.inner.lock().unwrap();
        let now = std::time::Instant::now();
        map.retain(|_, e| e.expires_at.is_none_or(|t| t > now));
        map.len()
    }
}

#[async_trait]
impl KeyValueStore for InMemoryKeyValueStore {
    async fn get(&self, key: &str) -> Option<String> {
        let mut map = self.inner.lock().unwrap();
        let entry = map.get(key)?;
        if entry
            .expires_at
            .is_some_and(|t| t <= std::time::Instant::now())
        {
            map.remove(key);
            return None;
        }
        Some(entry.value.clone())
    }

    async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64) {
        let expires = (ttl_secs > 0)
            .then(|| std::time::Instant::now() + std::time::Duration::from_secs(ttl_secs));
        self.inner.lock().unwrap().insert(
            key.to_string(),
            Entry {
                value: value.to_string(),
                expires_at: expires,
            },
        );
    }

    async fn del(&self, key: &str) {
        self.inner.lock().unwrap().remove(key);
    }

    async fn incr(&self, key: &str) -> i64 {
        let mut map = self.inner.lock().unwrap();
        let current = map
            .get(key)
            .and_then(|e| e.value.parse::<i64>().ok())
            .unwrap_or(0);
        let next = current + 1;
        map.insert(
            key.to_string(),
            Entry {
                value: next.to_string(),
                expires_at: None,
            },
        );
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 抖动过的 TTL 仍然贴着原值：±10% 是打散，不是重新设计有效期。
    #[test]
    fn jittered_ttl_stays_inside_ten_percent() {
        for _ in 0..500 {
            let j = jittered_ttl(100);
            assert!((90..=110).contains(&j), "抖动跑偏：{j}");
        }
        assert_eq!(jittered_ttl(0), 1, "0 不该被抖成「永不过期」");
        assert_eq!(jittered_ttl(5), 5, "短 TTL 不抖");
        let seen: std::collections::HashSet<u64> = (0..500).map(|_| jittered_ttl(600)).collect();
        assert!(seen.len() > 1, "TTL 抖动没有分散效果");
    }

    /// 命中不再回源；空结果用标记挡住重复回源（缓存设计防穿透）。
    #[tokio::test]
    async fn read_through_loads_once_and_remembers_absence() {
        let cache = KvCache::memory();
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let key = "cicada:dict:1:sys_yes_no:v0";

        let first = {
            let loads = loads.clone();
            cache
                .read_through(key, 60, || async move {
                    loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Some("[]".to_string())
                })
                .await
        };
        assert_eq!(first.as_deref(), Some("[]"));
        for _ in 0..3 {
            let loads = loads.clone();
            let v = cache
                .read_through(key, 60, || async move {
                    loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Some("[]".to_string())
                })
                .await;
            assert_eq!(v.as_deref(), Some("[]"));
        }
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);

        // 「确实没有」也要被记住，但只记 30 秒——不是永久。
        let missing = "cicada:config:1:absent:v0";
        for _ in 0..2 {
            assert_eq!(
                cache.read_through(missing, 60, || async { None }).await,
                None,
                "空结果应当稳定地读成 None，而不是把标记漏给调用方"
            );
        }
        let raw = cache.kv.get(missing).await;
        assert_eq!(raw.as_deref(), Some(EMPTY_MARKER));
    }

    /// 单飞：并发未命中只回源一次，且所有等待者都拿到同一份结果。
    #[tokio::test]
    async fn concurrent_misses_load_the_key_once() {
        let cache = KvCache::memory();
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let key = "cicada:menu:1:2,3:v0";

        let one = |cache: KvCache, loads: Arc<std::sync::atomic::AtomicUsize>| async move {
            cache
                .read_through(key, 60, || async move {
                    loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    Some("[{\"menu_id\":9}]".to_string())
                })
                .await
        };
        let results = tokio::join!(
            one(cache.clone(), loads.clone()),
            one(cache.clone(), loads.clone()),
            one(cache.clone(), loads.clone()),
            one(cache.clone(), loads.clone()),
        );
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
        for r in [
            results.0.as_deref(),
            results.1.as_deref(),
            results.2.as_deref(),
            results.3.as_deref(),
        ] {
            assert_eq!(r, Some("[{\"menu_id\":9}]"), "等待者要拿到同一个结果");
        }
        assert!(
            cache.flight.lock().unwrap().get(key).is_none(),
            "装载完成后要把自己从单飞表里摘掉"
        );
    }

    /// 版本号：自增之后旧键不可达，这就是缓存的「整域失效」。
    #[tokio::test]
    async fn version_bump_makes_old_entries_unreachable() {
        let cache = KvCache::memory();
        let ver_key = "cicada:menu:ver:1";
        let entry = |v: i64| format!("cicada:menu:1:7:v{v}");
        cache.kv.set_ex(&entry(0), "old", 600).await;

        assert_eq!(cache.domain_version(ver_key).await, 0);
        assert_eq!(
            cache
                .read_through(&entry(0), 60, || async { None })
                .await
                .as_deref(),
            Some("old")
        );

        assert_eq!(cache.bump_domain_version(ver_key).await, 1);
        assert_eq!(cache.domain_version(ver_key).await, 1);
        // 新键位上什么都没有：这一次会回源。
        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = hit.clone();
        let fresh = cache
            .read_through(&entry(1), 60, move || async move {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Some("new".to_string())
            })
            .await;
        assert_eq!(fresh.as_deref(), Some("new"));
        assert!(
            hit.load(std::sync::atomic::Ordering::SeqCst),
            "版本跳了就该回源"
        );
    }

    #[tokio::test]
    async fn memory_store_honours_ttl() {
        let kv = InMemoryKeyValueStore::new();
        kv.set_ex("k", "v", 300).await;
        assert_eq!(kv.get("k").await.as_deref(), Some("v"));
        assert_eq!(kv.live_keys(), 1);
        kv.set_ex("gone", "v", 1).await;
        // 过期判定发生在读的时候，不需要后台线程。
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert_eq!(kv.get("gone").await, None);
        assert_eq!(kv.incr("n").await, 1);
        assert_eq!(kv.incr("n").await, 2);
    }
}
