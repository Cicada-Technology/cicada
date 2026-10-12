//! 缓存观测 adapter 的真实 Redis 冒烟测试（`cargo test -p cicada-monitor -- --ignored`）。
//!
//! 默认忽略：需要本机 Redis。测试只使用**带随机后缀**的键，并且只删自己建的键，
//! 不会碰同一实例上的其他数据。

use cicada_infra::Cache;
use cicada_monitor::cache::{CACHE_DOMAINS, RedisCacheInspector, count_keys, domain_for_key};
use cicada_ports::{CacheError, CacheInspector};

fn url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379/0".to_string())
}

#[tokio::test]
#[ignore = "需要真实 Redis"]
async fn redis_adapter_observes_but_never_leaves_the_whitelist() {
    let cache = Cache::connect(&url()).await.unwrap();
    let inspector = RedisCacheInspector::new(cache.clone());

    let run = uuid::Uuid::new_v4().simple().to_string();
    let authz_key = format!("cicada:authz:1:1:v{run}");
    let lock_key = format!("pwd_lock:{run}");

    // 概览：真实 Redis 的 INFO/DBSIZE 能被解析。
    let info = inspector.info().await.unwrap();
    assert!(!info.version.is_empty(), "应解析出 redis_version");
    assert!(info.db_keys > 0, "当前库应至少有本次写入的键");
    assert!(info.hit_rate() >= 0.0 && info.hit_rate() <= 1.0);

    cache.set_ex(&authz_key, "{}", 300).await.unwrap();
    cache.set_ex(&lock_key, "1", 300).await.unwrap();

    // 分页扫描：跟着游标翻完整个域，而不是只看第一页——共享实例上 authz 域早就有
    // 上百个键时，第一页未必包含刚写的那把；断在「第一页」会把正确的分页行为测成竞态。
    let mut cursor = 0u64;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = inspector.scan("cicada:authz:*", cursor, 100).await.unwrap();
        pages += 1;
        assert!(
            page.keys.iter().all(|k| k.starts_with("cicada:authz:")),
            "扫描漏出了域外的键：{:?}",
            page.keys
                .iter()
                .filter(|k| !k.starts_with("cicada:authz:"))
                .collect::<Vec<_>>()
        );
        seen.extend(page.keys.iter().cloned());
        cursor = page.cursor;
        if page.done() {
            break;
        }
        assert!(pages < 200, "翻页没收束：游标一直不回 0");
    }
    assert!(
        seen.contains(&authz_key),
        "扫完整个域应能看到自己写的键（{pages} 页）"
    );

    // 单键元信息：类型与 TTL。
    let meta = inspector.key_meta(&authz_key).await.unwrap();
    assert_eq!(meta.kind, "string");
    assert!(meta.ttl_secs > 0 && meta.ttl_secs <= 300);
    assert_eq!(domain_for_key(&authz_key).unwrap().name, "authz");

    // 域内计数：自造的「肯定不存在」前缀应计到 0（走的是真实 SCAN 循环）。
    let probe = format!("cicada:authz:__none__{run}");
    assert_eq!(count_keys(&inspector, &probe, 10).await.unwrap().keys, 0);

    // 白名单之外一律拒绝：模式、键、前缀三条路径都要挡。
    assert!(matches!(
        inspector.scan("evil:*", 0, 10).await,
        Err(CacheError::OutOfNamespace(_))
    ));
    assert!(matches!(
        inspector.del("evil:1").await,
        Err(CacheError::OutOfNamespace(_))
    ));
    assert!(matches!(
        inspector.clear_prefix("cicada:", 10).await,
        Err(CacheError::OutOfNamespace(_))
    ));

    // 清理只删自己的键。
    assert!(inspector.del(&authz_key).await.unwrap());
    assert_eq!(inspector.key_meta(&authz_key).await.unwrap().ttl_secs, -2);
    assert!(inspector.del(&lock_key).await.unwrap());

    assert_eq!(CACHE_DOMAINS.len(), 15, "白名单域数量变化时请同步文档");
}
