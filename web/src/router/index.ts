/**
 * 路由实例。
 *
 * 根部固定一条 `name: 'root'` 的 Layout 记录（带 children 才会被 `addRoute` 当作父路由），
 * 动态菜单作为它的子路由注册，这样「外壳」只挂载一次，切页不会重建侧栏与顶栏。
 */

import { createRouter, createWebHistory } from 'vue-router';

import { installGuards } from './guard';
import { notFoundRoute, staticRoutes } from './routes';

function withRootName(routes: typeof staticRoutes): typeof staticRoutes {
  return routes.map((route) => (route.path === '/' ? { ...route, name: 'root' } : route));
}

export const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  // 兜底路由可以先声明：Vue Router 按路径特异性排序（静态段 > 命名参数 > 通配），
  // 因此 `/:pathMatch(.*)*` 永远排在动态菜单路由之后，不会把它们吃掉。
  routes: [...withRootName(staticRoutes), notFoundRoute],
  scrollBehavior: () => ({ top: 0 }),
});

installGuards(router);
