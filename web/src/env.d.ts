/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 浏览器标题与页面标题后缀。 */
  readonly VITE_APP_TITLE?: string;
  /** 后端源；留空表示与前端同源（生产由 Nginx 反代 /api 与 /auth）。 */
  readonly VITE_API_BASE_URL?: string;
  /** 仅开发期使用：vite dev server 把 /api、/auth 转发到这里。 */
  readonly VITE_DEV_PROXY_TARGET?: string;
  readonly VITE_DEV_PORT?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

declare global {
  /** 构建期注入的前端版本号（`vite.config.ts` 的 `define`，取自 package.json）。 */
  const __APP_VERSION__: string;
}

/**
 * 路由 `meta` 的形状：路由由后端菜单树动态生成，`meta.perms` 与后端权限标识
 * 一一对应。后端菜单树里没有的字段（如 `keepAlive`）由前端补默认值。
 */
import 'vue-router';

declare module 'vue-router' {
  interface RouteMeta {
    /** 免登录页面（登录页、错误页）：守卫据此放行。 */
    public?: boolean;
    /** 侧栏与面包屑显示名，来自后端菜单树的 `menu_name`。 */
    title?: string;
    /** 后端权限标识，与 `GET /auth/profile` 下发的 `permissions` 同名。 */
    perms?: string[];
    /** 后端菜单 id，便于把动态路由与菜单树对齐（刷新后重建路由用得上）。 */
    menuId?: number;
    /** Element Plus 图标名（后端 `icon` 字段存的是名字，如 `system`、`build`）。 */
    icon?: string;
    /** 外链菜单：不是内部路由，侧栏直接新窗口打开。 */
    external?: string;
    /** 是否缓存页面组件（配合 `<KeepAlive>`）。 */
    keepAlive?: boolean;
    /** 面包屑链路，由动态路由生成器填写。 */
    breadcrumb?: string[];
  }
}
