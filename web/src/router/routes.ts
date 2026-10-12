/**
 * 静态路由：与后端菜单树无关的那几条（登录、概览、错误页、兜底）。
 *
 * 业务路由不在这里——它们由 `dynamic.ts` 从 `GET /auth/routers` 生成。
 */

import type { RouteRecordRaw } from 'vue-router';

import AppLayout from '@/layout/AppLayout.vue';

/** 概览页：不需要任何权限点，任何登录用户都能落地（菜单没授权时也有地方可看）。 */
export const HOME_PATH = '/dashboard';

export const staticRoutes: RouteRecordRaw[] = [
  {
    path: '/login',
    name: 'login',
    component: () => import('@/views/login/index.vue'),
    meta: { title: '登录', public: true },
  },
  {
    path: '/',
    component: AppLayout,
    redirect: HOME_PATH,
    children: [
      {
        path: 'dashboard',
        name: 'dashboard',
        component: () => import('@/views/dashboard/index.vue'),
        meta: { title: '概览', icon: 'HomeFilled', keepAlive: true },
      },
    ],
  },
  {
    path: '/403',
    name: 'forbidden',
    component: () => import('@/views/error/403.vue'),
    meta: { title: '无权限', public: true },
  },
];

/**
 * 兜底路由。路径特异性由 vue-router 自己排序（静态段 > 命名参数 > 通配），
 * 因此可以一开始就声明，动态菜单装载后依然优先命中（见 `router/index.ts`）。
 *
 * 它**不能**标 `public`：未登录时业务路由一条都还没装载，`/system/user` 这类真实内页
 * 也会先落到这里。若标了 public，守卫会放行，用户看到的是「页面不存在」而不是「请登录」
 * ——链接没错，错的是提示。留在守卫里兜售登录页，登录后 `redirect` 会自动回到原路径。
 */
export const notFoundRoute: RouteRecordRaw = {
  path: '/:pathMatch(.*)*',
  name: 'not-found',
  component: () => import('@/views/error/404.vue'),
  meta: { title: '页面不存在' },
};
