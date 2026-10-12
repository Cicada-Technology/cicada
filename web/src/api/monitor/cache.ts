/** 缓存监控（`monitor:cache:*`）。域（domain）是缓存键的命名前缀，由 infra 层定义。 */

import { request } from '@/utils/request';

export interface CacheInfoVo {
  version: string;
  role: string;
  uptime_secs: number;
  connected_clients: number;
  used_memory: number;
  used_memory_human: string;
  maxmemory: number;
  db_keys: number;
  hits: number;
  misses: number;
  expired_keys: number;
  evicted_keys: number;
  hit_rate: number;
}

export interface DomainStat {
  /** 域标识：清空/列举接口的 `domain` 入参就是它。 */
  name: string;
  prefix: string;
  description: string;
  /** 敏感域（如会话、令牌）在界面上需要额外警示。 */
  sensitive: boolean;
  keys: number;
  /** 键数达到统计上限，真实数量可能更多。 */
  truncated: boolean;
}

/** `/monitor/cache/domains` 返回的是带 `items` 的对象，不是裸数组。 */
export interface DomainList {
  items: DomainStat[];
}

export interface CacheOverview {
  info: CacheInfoVo;
  domains: DomainStat[];
  scanned_at: string;
}

export interface CacheKeyPage {
  domain: string;
  prefix: string;
  /** 游标分页：`SCAN` 的语义，不要把 cursor 当页码。 */
  cursor: number;
  has_more: boolean;
  keys: string[];
}

export interface CacheKeyMeta {
  key: string;
  kind: string;
  ttl_secs: number;
  domain: string;
}

export interface ClearResult {
  domain: string;
  /** 实际删除的键数。 */
  removed: number;
}

export function cacheOverview(): Promise<CacheOverview> {
  return request.get<CacheOverview>('/api/v1/monitor/cache/overview');
}

export function listCacheDomains(): Promise<DomainList> {
  return request.get<DomainList>('/api/v1/monitor/cache/domains');
}

export function listCacheKeys(domain: string, cursor = 0, count = 100): Promise<CacheKeyPage> {
  return request.get<CacheKeyPage>('/api/v1/monitor/cache/keys', {
    params: { domain, cursor, count },
  });
}

export function getCacheKeyMeta(key: string): Promise<CacheKeyMeta> {
  return request.get<CacheKeyMeta>('/api/v1/monitor/cache/key', { params: { key } });
}

/** 清空整个域，或域下的单个键（给出 `key` 时）。返回实际删除数。 */
export function clearCacheKeys(domain: string, key?: string): Promise<ClearResult> {
  return request.delete<ClearResult>('/api/v1/monitor/cache/keys', {
    params: key ? { domain, key } : { domain },
  });
}
