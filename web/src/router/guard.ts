/**
 * 导航守卫：认证、动态路由装载、`meta.perms` 兜底校验、标题。
 *
 * 顺序很重要——动态路由必须在首次解析目标路径之前装载完，否则刷新页面会先落到 404
 * 兜底路由；装载完成后用一次 `replace` 重放同一路径即可命中真实路由。
 */

import type { Router } from 'vue-router';

import { i18n } from '@/locales';
import { useAuthStore } from '@/stores/auth';
import { usePermissionStore } from '@/stores/permission';

import { HOME_PATH } from './routes';

export const LOGIN_PATH = '/login';
export const FORBIDDEN_PATH = '/403';

function titleOf(metaTitle: unknown): string {
  const appTitle = i18n.global.t('app.title');
  return typeof metaTitle === 'string' && metaTitle !== ''
    ? `${metaTitle} · ${appTitle}`
    : appTitle;
}

export function installGuards(router: Router): void {
  router.beforeEach(async (to) => {
    const auth = useAuthStore();
    const permission = usePermissionStore();

    if (!auth.isAuthenticated) {
      if (to.meta.public) return true;
      return { path: LOGIN_PATH, query: { redirect: to.fullPath } };
    }

    if (to.path === LOGIN_PATH) {
      return { path: permission.firstPath ?? HOME_PATH };
    }

    try {
      await auth.ensureProfile();
    } catch {
      // 令牌在别处被吊销（改密、管理员强退）时 profile 会 401；
      // 拦截器已经清过会话，这里只要把用户送回登录页。
      auth.reset();
      return { path: LOGIN_PATH, query: { redirect: to.fullPath } };
    }

    if (!permission.loaded) {
      await permission.load();
      for (const record of permission.routes) {
        permission.trackRemover(router.addRoute('root', record));
      }
      // 动态路由刚就绪：重放当前路径，让它有机会匹配到真实页面。
      //
      // 这里必须把 `path` 与 `query` 拆开写，不能图省事用 `{ path: to.fullPath }`：
      // vue-router 构造 fullPath 时只认 `rawLocation.query`，塞进 `path` 的查询串会被
      // `parseURL` 吃掉——症状是「带参数的链接一刷新参数就没了」，而且不报错。
      return { path: to.path, query: to.query, hash: to.hash, replace: true };
    }

    // 直接输入 URL 的兜底：没授权时不该靠 404 才发现，给一个说明清楚的 403。
    const required = to.meta.perms;
    if (required?.length && !auth.hasAnyPermission(required)) {
      return { path: FORBIDDEN_PATH };
    }

    return true;
  });

  router.afterEach((to) => {
    document.title = titleOf(to.meta.title);
  });
}
