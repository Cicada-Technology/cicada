//! 缓存对象与键布局：参数字典、字典项、菜单树、租户信息。
//!
//! 权限集合的缓存在 `cicada-rbac`（`AuthzCache`，两级），这一层负责缓存设计点名的另外四类。
//! 键一律是 `cicada:{域}:{租户}:{键}:v{版本}`，而**版本前缀必须与 `CACHE_DOMAINS`
//! 里登记的域前缀对得上**（缓存设计：不在白名单里的键空间既看不见也清不掉，失败关闭）。
//! 版本号键刻意放在同一个域前缀下面，于是「整域清空」把值和版本一起清掉——
//! 清了版本却留着旧值，等于让缓存回到读得到过期数据的状态。
//!
//! 类型不写在返回值上而是泛型：这些接口缓存的是各接入层自己的 VO（`DictDataVo` 等），
//! L2 不该为了缓存去认识 L4 的响应结构。

use std::future::Future;

use serde::Serialize;
use serde::de::DeserializeOwned;

use cicada_kernel::TenantId;
use cicada_ports::KvCache;

/// 业务缓存的默认 TTL（秒）。
///
/// 长 TTL 是安全的：真正的失效靠版本号自增，不靠过期。**TTL 在这里只是兜底**
/// ——万一有哪条写路径忘了 bump，最坏也就是这么长时间后自己纠正。
pub const DEFAULT_TTL_SECS: u64 = 600;

/// 四类缓存对象的域与版本号键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheDomain {
    /// `sys_dict_data` / `sys_dict_type`。
    Dict,
    /// `sys_config`（全局表，无租户列）。
    Config,
    /// `sys_menu` 路由树。
    Menu,
    /// `sys_tenant`。
    Tenant,
}

impl CacheDomain {
    /// 域前缀：与 `cicada_monitor::cache::CACHE_DOMAINS` 的登记项一一对应。
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Dict => "cicada:dict:",
            Self::Config => "cicada:config:",
            Self::Menu => "cicada:menu:",
            Self::Tenant => "cicada:tenant:",
        }
    }

    /// 全局对象（`sys_config` 无租户列）用一个固定的 `global` 槽位，
    /// 而不是让租户号出现在键里假装它是隔离的。
    const GLOBAL: &'static str = "global";
}

/// 版本号键：`cicada:{域}:ver:{租户|global}`，落在域前缀之内。
pub fn version_key(domain: CacheDomain, tenant: Option<TenantId>) -> String {
    let slot = tenant
        .map(|t| t.get().to_string())
        .unwrap_or_else(|| CacheDomain::GLOBAL.to_string());
    format!("{}ver:{slot}", domain.prefix())
}

/// 条目键：`cicada:{域}:{租户|global}:{标识}:v{版本}`（缓存键规范）。
pub fn entry_key(
    domain: CacheDomain,
    tenant: Option<TenantId>,
    ident: &str,
    version: i64,
) -> String {
    let slot = tenant
        .map(|t| t.get().to_string())
        .unwrap_or_else(|| CacheDomain::GLOBAL.to_string());
    format!("{}{slot}:{ident}:v{version}", domain.prefix())
}

/// 读一个 JSON 值（键已经算好）。`load` 返回 `None` 表示「源里确实没有」，
/// 这个结论会以短 TTL 记进缓存（缓存设计防穿透）。
///
/// 存储用 `read_through` 的字符串通道，序列化只发生在回源那一次；取回时要先验结构：
/// 灰度发布或结构变更后，旧值解析不出来必须**清掉再回源**，而不是让请求失败、
/// 也不是让这个键永久坏着。
pub async fn cached_json<T, F, Fut>(
    cache: &KvCache,
    domain: CacheDomain,
    tenant: Option<TenantId>,
    ident: &str,
    load: F,
) -> Option<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let version = cache.domain_version(&version_key(domain, tenant)).await;
    let key = entry_key(domain, tenant, ident, version);
    if let Some(raw) = cache
        .peek_raw(&key)
        .await
        .filter(|v| v != cicada_ports::EMPTY_MARKER)
    {
        match serde_json::from_str::<T>(&raw) {
            Ok(v) => return Some(v),
            Err(e) => {
                tracing::warn!(error = %e, key, "缓存值与当前结构不符，清掉并回源");
                cache.forget(&key).await;
            }
        }
    }
    let json = cache
        .read_through(&key, DEFAULT_TTL_SECS, move || async move {
            load().await.map(|v| serde_json::to_string(&v).ok())?
        })
        .await?;
    serde_json::from_str(&json).ok()
}

/// 整域失效（缓存设计「变更即整体失效」）：版本号 +1，旧键立刻不可达。
///
/// `sys_config` 是全局表，所以它的版本号也放在全局槽位。
pub async fn invalidate(cache: &KvCache, domain: CacheDomain, tenant: Option<TenantId>) -> i64 {
    cache
        .bump_domain_version(&version_key(domain, tenant))
        .await
}

/// 角色 → 菜单树的键标识：角色集合排序去重后拼接。
///
/// 排序是必须的——同一组角色换个顺序该得到同一个键，否则缓存里会出现两份等价副本，
/// 命中率掉一半而 nobody 知道为什么。
pub fn roles_ident(role_ids: &[i64]) -> String {
    let mut ids: Vec<i64> = role_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return "none".to_string();
    }
    ids.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cicada_kernel::TenantId;
    use serde::Deserialize;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Item {
        code: String,
    }

    #[test]
    fn keys_live_inside_their_registered_domain_prefix() {
        let tenant = Some(TenantId::new(7));
        for domain in [
            CacheDomain::Dict,
            CacheDomain::Config,
            CacheDomain::Menu,
            CacheDomain::Tenant,
        ] {
            let v = version_key(domain, tenant);
            let e = entry_key(domain, tenant, "sys_yes_no", 3);
            assert!(v.starts_with(domain.prefix()), "{v}");
            assert!(e.starts_with(domain.prefix()), "{e}");
            assert!(e.ends_with(":v3"), "{e}");
        }
        // 全局表用 `global` 槽位，而不是随便挑一个租户号。
        let global = version_key(CacheDomain::Config, None);
        assert_eq!(global, "cicada:config:ver:global");
    }

    /// 同一组角色换个顺序必须是同一个键。
    #[test]
    fn role_ident_is_order_independent() {
        assert_eq!(roles_ident(&[3, 1, 2]), roles_ident(&[1, 2, 3]));
        assert_eq!(roles_ident(&[1, 1, 2]), "1,2");
        assert_eq!(roles_ident(&[]), "none");
        assert_ne!(roles_ident(&[1, 2]), roles_ident(&[1, 2, 3]));
    }

    #[tokio::test]
    async fn cached_json_loads_once_and_serves_the_second_read() {
        let cache = KvCache::memory();
        let loads = Arc::new(AtomicUsize::new(0));
        let items = |loads: Arc<AtomicUsize>, cache: KvCache| async move {
            cached_json(
                &cache,
                CacheDomain::Dict,
                Some(TenantId::new(1)),
                "sys_yes_no",
                move || async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    Some(vec![Item {
                        code: "Y".to_string(),
                    }])
                },
            )
            .await
        };
        assert_eq!(
            items(loads.clone(), cache.clone()).await,
            Some(vec![Item {
                code: "Y".to_string()
            }])
        );
        assert_eq!(
            items(loads.clone(), cache.clone()).await,
            Some(vec![Item {
                code: "Y".to_string()
            }])
        );
        assert_eq!(loads.load(Ordering::SeqCst), 1, "第二次应当读缓存");
    }

    /// 版本号一自增，下一次读必须重新回源——这是「改字典立刻可见」的根据。
    #[tokio::test]
    async fn invalidate_forces_a_fresh_load() {
        let cache = KvCache::memory();
        let tenant = Some(TenantId::new(1));
        let loads = Arc::new(AtomicUsize::new(0));

        async fn read(
            cache: &KvCache,
            tenant: Option<TenantId>,
            loads: Arc<AtomicUsize>,
        ) -> Option<Vec<Item>> {
            cached_json(
                cache,
                CacheDomain::Dict,
                tenant,
                "sys_yes_no",
                move || async move {
                    let n = loads.fetch_add(1, Ordering::SeqCst);
                    Some(vec![Item {
                        code: format!("v{n}"),
                    }])
                },
            )
            .await
        }

        assert_eq!(
            read(&cache, tenant, loads.clone()).await,
            Some(vec![Item {
                code: "v0".to_string()
            }])
        );
        assert_eq!(
            read(&cache, tenant, loads.clone()).await,
            Some(vec![Item {
                code: "v0".to_string()
            }])
        );
        assert_eq!(loads.load(Ordering::SeqCst), 1, "第二次应当命中缓存");

        invalidate(&cache, CacheDomain::Dict, tenant).await;
        assert_eq!(
            read(&cache, tenant, loads.clone()).await,
            Some(vec![Item {
                code: "v1".to_string()
            }]),
            "失效后必须读到新回源的结果，而不是旧键"
        );
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    /// 脏值（结构对不上的历史缓存）不该让请求失败。
    #[tokio::test]
    async fn a_corrupt_cached_value_degrades_to_loading() {
        let cache = KvCache::memory();
        let tenant = Some(TenantId::new(1));
        let key = entry_key(CacheDomain::Dict, tenant, "broken", 0);
        cache
            .read_through(&key, 60, || async { Some("{ not json".to_string()) })
            .await;

        let loads = Arc::new(AtomicUsize::new(0));
        let flag = loads.clone();
        let got: Option<Vec<Item>> = cached_json(
            &cache,
            CacheDomain::Dict,
            tenant,
            "broken",
            move || async move {
                flag.fetch_add(1, Ordering::SeqCst);
                Some(vec![Item {
                    code: "recovered".to_string(),
                }])
            },
        )
        .await;
        assert_eq!(
            got,
            Some(vec![Item {
                code: "recovered".to_string()
            }])
        );
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }
}
