//! 缓存域白名单与 Redis 观测 adapter（缓存 / 缓存监控）。
//!
//! 设计要点：**只观测 cicada 自己登记的键空间**。Redis 里同时放着会话与令牌索引，
//! 若提供「按任意键浏览/删除」，只读管理接口就变成了越权与凭据泄漏面（参考实现的
//! `/monitor/cache` 正是这样：读权限就能 `clearAll`）。这里用白名单把键空间切成
//! 若干**域**，接口只认域名，不认任意键；破坏性操作只到「整域失效」这一层，
//! 正好对应缓存的「版本号失效」——把域清空即可让全量缓存自然重建。

use async_trait::async_trait;

use cicada_infra::Cache;
use cicada_ports::{CacheError, CacheInfo, CacheInspector, KeyMeta, ScanPage};

/// 一次清域的单键上限：避免误操作把整个库扫空。
pub const CLEAR_MAX_KEYS: u64 = 10_000;

/// 概览里每个域最多统计的键数，超过即标记 `truncated`。
pub const COUNT_CAP: u64 = 1_000;

/// 参数缓存的域前缀；`/system/configs/refresh-cache` 按它整域失效。
pub const CONFIG_DOMAIN_PREFIX: &str = "cicada:config";

/// cicada 自有的一个 Redis 键空间。
#[derive(Clone, Copy, Debug)]
pub struct CacheDomain {
    /// 域标识（HTTP 面用它，不用前缀）。
    pub name: &'static str,
    /// 键前缀；域之间必须互不包含（`domain_for_key` 取最长匹配）。
    pub prefix: &'static str,
    pub description: &'static str,
    /// 含个人身份或会话信息，前端应额外提示。
    pub sensitive: bool,
}

/// 白名单：与 `cicada-auth` / `cicada-rbac` / `cicada-worker` 的键规范保持一致。
///
/// 新增缓存域时**必须**在这里登记，否则该域不可观测、也不可清理（失败关闭）。
pub const CACHE_DOMAINS: &[CacheDomain] = &[
    CacheDomain {
        name: "authz",
        prefix: "cicada:authz:",
        description: "权限集合缓存（键含角色版本号）",
        sensitive: false,
    },
    CacheDomain {
        name: "permver",
        prefix: "cicada:permver:",
        description: "角色权限版本号（自增即全量失效）",
        sensitive: false,
    },
    CacheDomain {
        name: "session",
        prefix: "session:",
        description: "在线会话（权威源，清理即强制下线）",
        sensitive: true,
    },
    CacheDomain {
        name: "user-sessions",
        prefix: "user_sessions:",
        description: "用户 → 会话索引集合",
        sensitive: false,
    },
    CacheDomain {
        name: "session-index",
        prefix: "session_index",
        description: "全局在线会话索引集合",
        sensitive: false,
    },
    CacheDomain {
        name: "lockout",
        prefix: "pwd_",
        description: "登录失败计数与账号锁定（清理即解锁）",
        sensitive: false,
    },
    CacheDomain {
        name: "captcha",
        prefix: "captcha_ans:",
        description: "登录图形验证码答案（一次性，清理即全部作废）",
        sensitive: true,
    },
    CacheDomain {
        name: "idempotency",
        prefix: "cicada:idem:",
        description: "Idempotency-Key 执行位与结果重放（清理即允许同键重试）",
        sensitive: true,
    },
    CacheDomain {
        name: "job",
        prefix: "cicada:job:",
        description: "定时任务选主租约与运行锁",
        sensitive: false,
    },
    CacheDomain {
        name: "oauth-state",
        prefix: "oauth:state:",
        description: "第三方登录授权 state（一次性，清理即全部作废）",
        sensitive: true,
    },
    CacheDomain {
        name: "download",
        prefix: "cicada:dl:",
        description: "文件一次性下载凭证（清理即让已签发链接立刻失效）",
        sensitive: true,
    },
    CacheDomain {
        name: "dict",
        prefix: "cicada:dict:",
        description: "字典项缓存（键含版本号；整域失效=版本号自增）",
        sensitive: false,
    },
    CacheDomain {
        name: "menu",
        prefix: "cicada:menu:",
        description: "角色菜单树缓存（键含版本号）",
        sensitive: false,
    },
    CacheDomain {
        name: "tenant",
        prefix: "cicada:tenant:",
        description: "租户信息缓存（键含版本号）",
        sensitive: false,
    },
    CacheDomain {
        name: "config",
        prefix: CONFIG_DOMAIN_PREFIX,
        description: "参数缓存（含版本号；/system/configs/refresh-cache 的目标）",
        sensitive: false,
    },
];

/// 按域名取域。
pub fn domain_for_name(name: &str) -> Option<&'static CacheDomain> {
    CACHE_DOMAINS.iter().find(|d| d.name == name)
}

/// 按键取所属域（最长前缀匹配）；不在白名单内返回 `None`。
pub fn domain_for_key(key: &str) -> Option<&'static CacheDomain> {
    CACHE_DOMAINS
        .iter()
        .filter(|d| key.starts_with(d.prefix))
        .max_by_key(|d| d.prefix.len())
}

/// 前缀必须落在某个域内部（等于域前缀，或是它更长的一段），不允许跨域。
fn prefix_within_domain(prefix: &str) -> bool {
    CACHE_DOMAINS.iter().any(|d| prefix.starts_with(d.prefix))
}

/// 扫描模式必须形如 `<域前缀>…*`。
fn pattern_within_domain(pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => prefix_within_domain(prefix),
        None => false,
    }
}

fn backend(e: impl std::fmt::Display) -> CacheError {
    CacheError::Backend(e.to_string())
}

/// 某个域当前的键数统计结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomainCount {
    /// 实际键数（`cap` 为硬上限，不会超过）。
    pub keys: u64,
    /// 是否因为达到 `cap` 而提前停止（真实键数可能更多）。
    pub truncated: bool,
}

/// 统计某前缀下的键数（`SCAN` 循环，只读）。`cap` 是硬上限。
pub async fn count_keys(
    inspector: &dyn CacheInspector,
    prefix: &str,
    cap: u64,
) -> Result<DomainCount, CacheError> {
    let pattern = format!("{prefix}*");
    let mut cursor = 0u64;
    let mut total = 0u64;
    loop {
        let page = inspector.scan(&pattern, cursor, 500).await?;
        total += page.keys.len() as u64;
        cursor = page.cursor;
        if cursor == 0 {
            let truncated = total > cap;
            return Ok(DomainCount {
                keys: total.min(cap),
                truncated,
            });
        }
        if total >= cap {
            return Ok(DomainCount {
                keys: cap,
                truncated: true,
            });
        }
    }
}

/// 生产 adapter：把 `cicada-infra::Cache` 的 Redis 原语映射成观测端口。
///
/// 白名单在这里**再校验一次**（纵深防御）：即便调用方绕过 HTTP 面的校验，
/// 也不可能通过这个 adapter 读到或删掉白名单之外的键。
#[derive(Clone)]
pub struct RedisCacheInspector {
    cache: Cache,
}

impl RedisCacheInspector {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }

    /// 直接拿到 Redis 句柄（组装根注入 `Cache` 时用）。
    pub fn cache(&self) -> &Cache {
        &self.cache
    }
}

#[async_trait]
impl CacheInspector for RedisCacheInspector {
    async fn info(&self) -> Result<CacheInfo, CacheError> {
        let raw = self.cache.info().await.map_err(backend)?;
        let num = |key: &str| {
            raw.get(key)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        };
        let text = |key: &str| raw.get(key).cloned().unwrap_or_default();
        Ok(CacheInfo {
            version: text("redis_version"),
            role: text("role"),
            uptime_secs: num("uptime_in_seconds"),
            connected_clients: num("connected_clients"),
            used_memory: num("used_memory"),
            used_memory_human: text("used_memory_human"),
            maxmemory: num("maxmemory"),
            db_keys: self.cache.dbsize().await.map_err(backend)?,
            hits: num("keyspace_hits"),
            misses: num("keyspace_misses"),
            expired_keys: num("expired_keys"),
            evicted_keys: num("evicted_keys"),
        })
    }

    async fn scan(&self, pattern: &str, cursor: u64, count: u64) -> Result<ScanPage, CacheError> {
        if !pattern_within_domain(pattern) {
            return Err(CacheError::OutOfNamespace(pattern.to_string()));
        }
        let (cursor, keys) = self
            .cache
            .scan(pattern, cursor, count)
            .await
            .map_err(backend)?;
        Ok(ScanPage { cursor, keys })
    }

    async fn key_meta(&self, key: &str) -> Result<KeyMeta, CacheError> {
        if domain_for_key(key).is_none() {
            return Err(CacheError::OutOfNamespace(key.to_string()));
        }
        let kind = self.cache.key_type(key).await.map_err(backend)?;
        let ttl_secs = if kind == "none" {
            -2
        } else {
            self.cache.ttl(key).await.map_err(backend)?
        };
        Ok(KeyMeta {
            key: key.to_string(),
            kind,
            ttl_secs,
        })
    }

    async fn del(&self, key: &str) -> Result<bool, CacheError> {
        if domain_for_key(key).is_none() {
            return Err(CacheError::OutOfNamespace(key.to_string()));
        }
        self.cache.del(key).await.map_err(backend)?;
        Ok(true)
    }

    async fn clear_prefix(&self, prefix: &str, max: u64) -> Result<u64, CacheError> {
        if !prefix_within_domain(prefix) {
            return Err(CacheError::OutOfNamespace(prefix.to_string()));
        }
        let pattern = format!("{prefix}*");
        let mut cursor = 0u64;
        let mut removed = 0u64;
        loop {
            let (next, keys) = self
                .cache
                .scan(&pattern, cursor, 200)
                .await
                .map_err(backend)?;
            for key in keys {
                if removed >= max {
                    return Ok(removed);
                }
                self.cache.del(&key).await.map_err(backend)?;
                removed += 1;
            }
            cursor = next;
            if cursor == 0 {
                return Ok(removed);
            }
        }
    }

    async fn dbsize(&self) -> Result<u64, CacheError> {
        self.cache.dbsize().await.map_err(backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_lookup_picks_longest_prefix() {
        assert_eq!(domain_for_key("cicada:authz:1:2:v3").unwrap().name, "authz");
        assert_eq!(domain_for_key("cicada:permver:1").unwrap().name, "permver");
        assert_eq!(domain_for_key("cicada:job:leader").unwrap().name, "job");
        assert_eq!(
            domain_for_key("user_sessions:7").unwrap().name,
            "user-sessions"
        );
        assert_eq!(domain_for_key("session:abc").unwrap().name, "session");
        assert_eq!(
            domain_for_key("session_index").unwrap().name,
            "session-index"
        );
        assert_eq!(domain_for_key("pwd_lock:admin").unwrap().name, "lockout");
        assert_eq!(
            domain_for_key("cicada:config:1:pwd:v2").unwrap().name,
            "config"
        );
    }

    #[test]
    fn unknown_keys_are_outside_the_namespace() {
        assert!(domain_for_key("some:other:app:key").is_none());
        assert!(domain_for_key("sessionx").is_none(), "前缀必须整段匹配");
        // `cicada:` 本身不是域——不允许整片清理。
        assert!(!prefix_within_domain("cicada:"));
        assert!(prefix_within_domain("cicada:config"));
        assert!(pattern_within_domain("cicada:authz:*"));
        assert!(!pattern_within_domain("cicada:authz:"), "必须是扫描模式");
        assert!(!pattern_within_domain("cicada:*"), "不允许跨域扫描");
    }

    #[tokio::test]
    async fn count_keys_walks_scan_pages() {
        let fixture = cicada_ports::InMemoryCacheInspector::new();
        for i in 0..7 {
            fixture.set(&format!("cicada:authz:1:{i}:v1"), "x", Some(60));
        }
        fixture.set("session:other", "y", Some(60));
        let n = count_keys(&fixture, "cicada:authz:", COUNT_CAP)
            .await
            .unwrap();
        assert_eq!(
            n,
            DomainCount {
                keys: 7,
                truncated: false
            }
        );
        let capped = count_keys(&fixture, "cicada:authz:", 3).await.unwrap();
        assert_eq!(
            capped,
            DomainCount {
                keys: 3,
                truncated: true
            },
            "cap 是硬上限"
        );
    }
}
