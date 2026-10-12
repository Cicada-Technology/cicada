/**
 * 动态路由与侧栏菜单。
 *
 * 数据来源只有一个：`GET /auth/routers`（按当前用户的角色授权过滤后的菜单树）。
 * 后端已经做过可见性判定，前端不再二次过滤菜单——否则会出现「后端给了、前端藏了」
 * 这种最难排查的一类问题（`meta.perms` 只用于直接输入 URL 时的兜底校验）。
 */

import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import type { RouteRecordRaw } from 'vue-router';

import { getRouters } from '@/api/auth';
import { buildRoutesFromMenus, type SidebarMenu } from '@/router/dynamic';
import { notifyWarning } from '@/utils/feedback';

function countLeaves(items: SidebarMenu[]): number {
  return items.reduce(
    (total, item) => total + (item.children.length > 0 ? countLeaves(item.children) : 1),
    0,
  );
}

export const usePermissionStore = defineStore('permission', () => {
  const routes = ref<RouteRecordRaw[]>([]);
  const menus = ref<SidebarMenu[]>([]);
  const firstPath = ref<string | null>(null);
  /** 是否已经尝试过装载（失败也算装载过，避免守卫里反复重试把接口打爆）。 */
  const loaded = ref(false);
  /**
   * `router.addRoute` 返回的注销函数。
   *
   * 必须留着：换账号登录时若不撤掉上一个账号注册的路由，他会看到（并尝试打开）
   * 自己无权的页面——虽然接口会 403，但导航里出现别人的菜单本身就是错的。
   */
  const removers = ref<Array<() => void>>([]);

  const menuCount = computed(() => countLeaves(menus.value));

  /**
   * 拉取菜单树并生成路由。
   *
   * 不向外抛异常：菜单拉不到时给出空菜单 + 一条提示，用户仍能进概览页并手动刷新重试；
   * 把一次网络抖动变成「跳登录页」是很差的体验。
   */
  async function load(): Promise<void> {
    if (loaded.value) return;
    try {
      const tree = await getRouters();
      const generated = buildRoutesFromMenus(tree);
      routes.value = generated.routes;
      menus.value = generated.menus;
      firstPath.value = generated.firstPath;
    } catch {
      routes.value = [];
      menus.value = [];
      firstPath.value = null;
      notifyWarning('菜单加载失败，请刷新页面重试');
    } finally {
      loaded.value = true;
    }
  }

  function reset(): void {
    for (const remove of removers.value) {
      remove();
    }
    removers.value = [];
    routes.value = [];
    menus.value = [];
    firstPath.value = null;
    loaded.value = false;
  }

  function trackRemover(remove: () => void): void {
    removers.value = [...removers.value, remove];
  }

  return { routes, menus, firstPath, loaded, menuCount, load, reset, trackRemover };
});
