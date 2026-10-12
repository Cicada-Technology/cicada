/**
 * 会话令牌的持久化与失效广播。
 *
 * 为什么不放进 pinia：`utils/request.ts` 需要在请求拦截器里同步读取令牌，
 * 若让它依赖 auth store，而 store 又依赖 request，就成了循环引用。
 * 于是令牌状态放在这个零依赖模块里，由 store 负责把它包装成响应式状态。
 */

import { readStorage, removeStorage, writeStorage, STORAGE_KEYS } from './storage';

export interface TokenPair {
  accessToken: string;
  refreshToken: string;
}

/**
 * 会话终止的原因。
 *
 * 区分它不是洁癖：`logout` 由界面自己跳登录页，收到「登录已过期」的提示会让人以为
 * 出了故障；只有 `expired`（令牌过期、被强退、改密）才需要提示，并且要把当前地址记进
 * `?redirect=`，用户重新登录后能回到原地。
 */
export type SessionEndReason = 'logout' | 'expired';

let accessToken = readStorage(STORAGE_KEYS.accessToken) ?? '';
let refreshToken = readStorage(STORAGE_KEYS.refreshToken) ?? '';

const clearedListeners = new Set<() => void>();

export function getAccessToken(): string {
  return accessToken;
}

export function getRefreshToken(): string {
  return refreshToken;
}

export function hasSession(): boolean {
  return accessToken !== '';
}

/**
 * 写入令牌对。
 *
 * 刷新接口会轮换刷新令牌（旧的立即作废），所以这里必须成对更新：
 * 只更新 access 会让下一次刷新用到已失效的 refresh，表现为「频繁被踢」。
 */
export function setTokens(pair: TokenPair): void {
  accessToken = pair.accessToken;
  refreshToken = pair.refreshToken;
  writeStorage(STORAGE_KEYS.accessToken, accessToken);
  writeStorage(STORAGE_KEYS.refreshToken, refreshToken);
}

export function clearSession(reason: SessionEndReason = 'logout'): void {
  accessToken = '';
  refreshToken = '';
  removeStorage(STORAGE_KEYS.accessToken);
  removeStorage(STORAGE_KEYS.refreshToken);
  // 只有「被动终止」才广播。注意这里**不要**加「有没有会话」的前置判断去过滤重复广播：
  // 订阅者（`main.ts` 的 `handleSessionLost`）本身可能就是被重复触发的（多标签页、
  // 并发 401），过滤会让后到的那次彻底没人收尾；幂等交给收尾函数自己做。
  // 反过来说，收尾函数**不能**再调 `clearSession`，否则就是回调递归（实测 RangeError）。
  if (reason !== 'expired') return;
  for (const listener of [...clearedListeners]) {
    listener();
  }
}

/**
 * 订阅「会话被动终止」（令牌过期 / 被强退 / 改密）；返回取消订阅函数。
 *
 * 主动登出不触发：那种情况下界面自己知道要跳登录页，不需要收尾回调。
 */
export function onSessionCleared(listener: () => void): () => void {
  clearedListeners.add(listener);
  return () => {
    clearedListeners.delete(listener);
  };
}

/** 当前内存里的令牌对（只读快照）。 */
export function peekTokens(): TokenPair {
  return { accessToken, refreshToken };
}
