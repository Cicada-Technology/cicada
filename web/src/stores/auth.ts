/**
 * 认证状态：令牌、当前用户、角色与权限集合。
 *
 * 令牌本身存在 `utils/session.ts`（拦截器要同步读），这里只是把它包成响应式状态，
 * 并缓存 `/auth/profile` 的结果给指令与页面复用。
 */

import { defineStore } from 'pinia';
import { computed, ref } from 'vue';

import * as authApi from '@/api/auth';
import type { LoginRequest, ProfileResult } from '@/api/auth';
import {
  clearSession,
  peekTokens,
  setTokens,
  type SessionEndReason,
  type TokenPair,
} from '@/utils/session';

/** 超级管理员的角色标识：后端凭它短路所有权限判定（`cicada_rbac::ADMIN_ROLE_KEY`）。 */
export const SUPER_ADMIN_ROLE = 'admin';

export const useAuthStore = defineStore('auth', () => {
  const tokens = ref<TokenPair>(peekTokens());
  const profile = ref<ProfileResult | null>(null);
  /** 是否已经从后端拉到过 profile；路由守卫用它决定要不要先加载。 */
  const profileLoaded = ref(false);

  const isAuthenticated = computed(() => tokens.value.accessToken !== '');
  const userId = computed(() => profile.value?.user_id ?? null);
  const userName = computed(() => profile.value?.user_name ?? '');
  const tenantId = computed(() => profile.value?.tenant_id ?? null);
  const roles = computed(() => profile.value?.roles ?? []);
  const permissions = computed(() => profile.value?.permissions ?? []);
  const isSuperAdmin = computed(() => roles.value.includes(SUPER_ADMIN_ROLE));

  const permissionSet = computed(() => new Set(permissions.value));

  function hasPermission(perm: string): boolean {
    if (perm === '') return true;
    // 管理员短路与服务端一致：后端 `is_admin` 时不再逐点比对。
    return isSuperAdmin.value || permissionSet.value.has(perm);
  }

  function hasAnyPermission(perms: string[]): boolean {
    return perms.length === 0 || perms.some((perm) => hasPermission(perm));
  }

  function hasRole(role: string): boolean {
    return roles.value.includes(role);
  }

  function acceptTokens(next: TokenPair): void {
    setTokens(next);
    tokens.value = next;
  }

  async function signIn(payload: LoginRequest): Promise<void> {
    const result = await authApi.login(payload);
    acceptTokens({
      accessToken: result.access_token,
      refreshToken: result.refresh_token,
    });
  }

  async function loadProfile(): Promise<ProfileResult> {
    const result = await authApi.getProfile();
    profile.value = result;
    profileLoaded.value = true;
    return result;
  }

  /** 只加载一次；路由守卫生效期间不会重复打接口。 */
  async function ensureProfile(): Promise<ProfileResult> {
    if (profile.value && profileLoaded.value) return profile.value;
    return loadProfile();
  }

  /**
   * 只把内存状态对齐到「已无会话」，不碰令牌、不广播。
   *
   * 会话失效的收尾（`main.ts::handleSessionLost`）必须用这个而不是 [`reset`]：
   * 令牌早就被 `clearSession('expired')` 清掉了，收尾里再清一次会重新广播回自己，
   * 形成回调递归（真实踩过 `RangeError: Maximum call stack size exceeded`）。
   */
  function forget(): void {
    tokens.value = { accessToken: '', refreshToken: '' };
    profile.value = null;
    profileLoaded.value = false;
  }

  /** 结束会话：清令牌 + 对齐内存状态。`reason` 决定是否向应用层广播（见 `utils/session`）。 */
  function reset(reason: SessionEndReason = 'logout'): void {
    clearSession(reason);
    forget();
  }

  async function signOut(): Promise<void> {
    try {
      await authApi.logout();
    } catch {
      // 登出接口失败（令牌已失效）不应该把用户卡在登录态，本地照样清干净。
    } finally {
      reset();
    }
  }

  return {
    tokens,
    profile,
    profileLoaded,
    isAuthenticated,
    userId,
    userName,
    tenantId,
    roles,
    permissions,
    isSuperAdmin,
    hasPermission,
    hasAnyPermission,
    hasRole,
    acceptTokens,
    signIn,
    loadProfile,
    ensureProfile,
    signOut,
    reset,
    forget,
  };
});
