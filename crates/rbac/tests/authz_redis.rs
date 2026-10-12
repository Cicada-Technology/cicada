//! `RedisAuthzStore` 对真实 Redis 的读写 + 版本号整体失效测试。

use std::sync::Arc;

use cicada_infra::Cache;
use cicada_kernel::{DataScopeKind, Permission, RoleGrant, RoleId, TenantId, UserId};
use cicada_ports::InMemoryDeptDirectory;
use cicada_rbac::{AuthzCache, RedisAuthzStore, authorize};

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379/0".to_string())
}

async fn sample(tenant: TenantId) -> cicada_rbac::Authorization {
    let dir = InMemoryDeptDirectory::new();
    authorize(
        tenant,
        UserId::new(7),
        None,
        &[RoleGrant {
            role_id: RoleId::new(1),
            role_key: "ops".to_string(),
            data_scope: DataScopeKind::All,
            dept_ids: Vec::new(),
        }],
        [Permission::new("system:user:add")],
        &dir,
    )
    .await
}

#[tokio::test]
#[ignore = "需要真实 Redis"]
async fn redis_cache_roundtrip_then_version_bump_invalidates() {
    let cache = Cache::connect(&redis_url()).await.unwrap();
    cache.ping().await.unwrap();

    let tenant = TenantId::new(uuid::Uuid::new_v4().as_u128() as i64 & 0x7fff_ffff);
    let user = UserId::new(7);
    let authz = AuthzCache::new(Arc::new(RedisAuthzStore::new(cache)), 60);

    assert!(authz.get(tenant, user).await.is_none());
    authz.put(tenant, user, &sample(tenant).await).await;

    let hit = authz.get(tenant, user).await.expect("应命中 Redis 缓存");
    assert!(
        hit.permissions
            .contains(&Permission::new("system:user:add"))
    );
    assert_eq!(hit.role_keys, vec!["ops".to_string()]);

    // 角色版本号 +1 → 本租户缓存整体失效。
    authz.invalidate_tenant(tenant).await;
    assert!(authz.get(tenant, user).await.is_none());
}
