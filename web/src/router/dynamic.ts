/**
 * 菜单树 → 路由与侧栏：路由由后端菜单树动态生成，`meta.perms` 与后端权限标识
 * 一一对应。
 *
 * 两个刻意的设计决定：
 *
 * 1. **路由是平的**（全部挂在唯一一个 Layout 下），侧栏是嵌套的。后台页面之间没有层级语义，
 *    扁平路由让面包屑、`keep-alive`、按名跳转都简单；层级只影响导航呈现，因此由菜单树
 *    自己承担。
 * 2. **`component` 缺失时按 `path` 推导视图**。种子数据里的菜单 `component` 是空串
 *    （见 `migrations/src/seed.rs::seed_menus`），而代码生成器产出的菜单给的是
 *    `module/business/index`。两种都要能用，所以推导规则要能同时吃下 `/system/user`
 *    与 `business/goods/index`。
 */

import type { RouteRecordRaw, RouteRecordSingleView } from 'vue-router';

import type { MenuNode } from '@/api/auth';

/** 侧栏视觉分组：同一顶级模块下的图标共用一套色调。 */
export type SidebarTone = 'system' | 'monitor' | 'tool' | 'platform' | 'notify' | 'neutral';

/** 侧栏节点：目录保留层级，叶子是可导航项。 */
export interface SidebarMenu {
  id: number;
  title: string;
  icon?: string;
  tone: SidebarTone;
  /** 内部路由路径，或（外链菜单）完整 URL。 */
  path: string;
  /** 外链：点击后新窗口打开，不参与路由。 */
  external: boolean;
  perms: string[];
  children: SidebarMenu[];
}

export interface GeneratedRoutes {
  routes: RouteRecordRaw[];
  menus: SidebarMenu[];
  /** 第一个可导航菜单的路径；概览之外的回退目标。 */
  firstPath: string | null;
}

/** 所有页面组件，编译期由 Vite 收成一张表。 */
const viewModules = import.meta.glob('@/views/**/*.vue');

/**
 * 把 glob 的键裁成 `views/` 之后的部分（`system/user/index.vue`）。
 *
 * 不能直接用 glob 的原始键：Vite 的键是**解析后的路径**（开发/构建下都形如
 * `/src/views/system/user/index.vue`），而菜单里的 `component`/`path` 是
 * `system/user/index`、`/system/user` 这种写法。两边各自归一化到同一形状再查，
 * 才不会因为前缀差异静默查不到——那种情况会让每个页面都掉进兜底视图。
 */
function viewKey(path: string): string {
  return path.replace(/^.*?\/?views\//, '');
}

const viewIndex = new Map<string, RouteRecordRaw['component']>(
  Object.entries(viewModules).map(([key, loader]) => [
    viewKey(key),
    loader as RouteRecordRaw['component'],
  ]),
);

const PLACEHOLDER_VIEW = 'placeholder/index.vue';

function normalizeComponentKey(raw: string): string {
  let key = raw.trim();
  if (key === '') return '';
  key = key.replace(/^@?\/?views\//, '');
  key = key.replace(/^\/+/, '');
  key = key.replace(/\.vue$/, '');
  // 目录形式（`system/user`）补 `index`；已经是文件名（`system/user/index`）则原样。
  if (!key.endsWith('/index') && !key.includes('.')) {
    key = `${key}/index`;
  }
  return `${key}.vue`;
}

function isExternalPath(path: string): boolean {
  return /^https?:\/\//i.test(path);
}

function joinPath(parent: string, segment: string): string {
  if (!segment) return parent;
  if (segment.startsWith('/')) return segment;
  const base = parent === '/' ? '' : parent.replace(/\/+$/, '');
  return `${base}/${segment}`;
}

function resolveView(node: MenuNode): string {
  const declared = node.component?.trim() ?? '';
  if (declared !== '') return normalizeComponentKey(declared);
  const path = (node.path ?? '').replace(/\/+$/, '');
  return normalizeComponentKey(path === '/' ? '' : path);
}

function routeName(path: string): string {
  const slug = path
    .replace(/^\/+/, '')
    .replace(/[^a-zA-Z0-9]+/g, '-')
    .replace(/-+$/, '');
  return slug === '' ? 'index' : `page-${slug}`;
}

function fallbackView(): RouteRecordRaw['component'] {
  // 视图缺失（菜单已配、界面未做）时不要 404：落在一个说明页上，
  // 用户能看懂「接口就绪、界面待做」，也不会以为链接坏了（ux-admin-portal 的取向）。
  const placeholder = viewIndex.get(PLACEHOLDER_VIEW);
  return (placeholder ?? (() => import('@/views/error/404.vue'))) as RouteRecordRaw['component'];
}

/**
 * 一个菜单节点贡献的路径。
 *
 * - `C` 菜单：`path` 可能是绝对（`/system/user`，种子数据）或相对（`goods`，生成器产物）。
 * - `M` 目录：`#`（种子数据的惯例）或空串表示不贡献路径段，其它值贡献一段。
 * - 外链：原样使用，不参与拼接。
 */
function ownPathOf(node: MenuNode, parentPath: string): { path: string; external: boolean } {
  const raw = node.path?.trim() ?? '';
  if (isExternalPath(raw)) return { path: raw, external: true };
  const isDirectory = node.menu_type !== 'C';
  if (isDirectory && (raw === '' || raw === '#')) return { path: parentPath, external: false };
  return { path: joinPath(parentPath, raw), external: false };
}

/**
 * 种子菜单的图标（Lucide 名称）。
 *
 * 后端 `sys_menu.icon` 允许为空；空值时按稳定的 `menu_id` 取这里的内置映射，
 * 自定义菜单再按路径前缀与名称关键词兜底。图标名统一由 `AppIcon` 解析。
 */
const MENU_ICONS: Record<number, string> = {
  1: 'Settings2',
  2: 'Activity',
  3: 'Wrench',
  4: 'Globe',
  5: 'Layers',
  6: 'Bell',
  100: 'Users',
  101: 'ShieldCheck',
  102: 'ListTree',
  103: 'Building2',
  104: 'Briefcase',
  105: 'BookOpen',
  106: 'Settings',
  107: 'Bell',
  108: 'ScrollText',
  109: 'UserCheck',
  110: 'CalendarClock',
  111: 'ChartColumn',
  112: 'Server',
  113: 'Database',
  114: 'LayoutTemplate',
  115: 'Code',
  116: 'Braces',
  200: 'Building2',
  201: 'Package',
  202: 'KeyRound',
  203: 'FolderOpen',
  204: 'Download',
  300: 'Send',
  301: 'FileText',
  302: 'History',
  500: 'ClipboardList',
  501: 'LogIn',
  502: 'History',
};

const ICON_BY_PATH: ReadonlyArray<readonly [string, string]> = [
  ['/system/user', 'Users'],
  ['/system/role', 'ShieldCheck'],
  ['/system/menu', 'ListTree'],
  ['/system/dept', 'Building2'],
  ['/system/post', 'Briefcase'],
  ['/system/dict', 'BookOpen'],
  ['/system/config', 'Settings'],
  ['/system/notice', 'Bell'],
  ['/system/tenant', 'Building2'],
  ['/system/package', 'Package'],
  ['/system/apikey', 'KeyRound'],
  ['/system/file', 'FolderOpen'],
  ['/system/export', 'Download'],
  ['/monitor/online', 'UserCheck'],
  ['/monitor/job', 'CalendarClock'],
  ['/monitor/data', 'ChartColumn'],
  ['/monitor/server', 'Server'],
  ['/monitor/cache', 'Database'],
  ['/monitor/operlog', 'ClipboardList'],
  ['/monitor/logininfor', 'LogIn'],
  ['/monitor/auditlog', 'History'],
  ['/tool/build', 'LayoutTemplate'],
  ['/tool/gen', 'Code'],
  ['/tool/swagger', 'Braces'],
  ['/notify/channel', 'Send'],
  ['/notify/template', 'FileText'],
  ['/notify/record', 'History'],
];

const ICON_BY_KEYWORD: ReadonlyArray<readonly [string, string]> = [
  ['用户', 'Users'],
  ['角色', 'ShieldCheck'],
  ['菜单', 'ListTree'],
  ['部门', 'Building2'],
  ['岗位', 'Briefcase'],
  ['字典', 'BookOpen'],
  ['参数', 'Settings'],
  ['公告', 'Bell'],
  ['日志', 'ScrollText'],
  ['在线', 'UserCheck'],
  ['任务', 'CalendarClock'],
  ['监控', 'ChartColumn'],
  ['缓存', 'Database'],
  ['代码', 'Code'],
  ['接口', 'Braces'],
  ['文件', 'FolderOpen'],
  ['通知', 'Bell'],
  ['租户', 'Building2'],
  ['渠道', 'Send'],
  ['模板', 'FileText'],
];

const TONE_BY_MENU_ID: Record<number, SidebarTone> = {
  1: 'system',
  2: 'monitor',
  3: 'tool',
  4: 'neutral',
  5: 'platform',
  6: 'notify',
};

function toneOf(node: MenuNode): SidebarTone {
  const seeded = TONE_BY_MENU_ID[node.menu_id];
  if (seeded) return seeded;
  const title = node.menu_name ?? '';
  if (title.includes('监控')) return 'monitor';
  if (title.includes('工具')) return 'tool';
  if (title.includes('通知')) return 'notify';
  if (title.includes('平台')) return 'platform';
  if (title.includes('系统')) return 'system';
  return 'neutral';
}

function buildSidebar(
  node: MenuNode,
  parentPath: string,
  trail: string[],
  inheritedTone?: SidebarTone,
): SidebarMenu | null {
  const { path, external } = ownPathOf(node, parentPath);
  const directory = node.menu_type !== 'C';
  const chain = [...trail, node.menu_name];
  const tone = inheritedTone ?? toneOf(node);

  const children = node.children
    .map((child) => buildSidebar(child, path, chain, tone))
    .filter((item): item is SidebarMenu => item !== null);

  // 目录里没有可见子项时整个隐藏，避免出现点不开的空分组。
  if (directory && children.length === 0) return null;

  return {
    id: node.menu_id,
    title: node.menu_name,
    icon: iconName(node),
    tone,
    path: directory ? children[0].path : path,
    external,
    perms: permsOf(node),
    children,
  };
}

/**
 * 图标：后端显式配置优先；为空时依次按 `menu_id`、路径前缀、名称关键词兜底。
 *
 * 返回的是 Lucide 图标名，具体解析与缺失兜底交给 `AppIcon`。
 */
function iconName(node: MenuNode): string | undefined {
  const raw = (node as MenuNode & { icon?: string | null }).icon;
  const configured = raw?.trim();
  if (configured) return configured;

  const byId = MENU_ICONS[node.menu_id];
  if (byId) return byId;

  const path = (node.path ?? '').trim();
  for (const [prefix, icon] of ICON_BY_PATH) {
    if (path === prefix || path.startsWith(`${prefix}/`)) return icon;
  }

  const title = node.menu_name ?? '';
  for (const [keyword, icon] of ICON_BY_KEYWORD) {
    if (title.includes(keyword)) return icon;
  }
  return undefined;
}

function permsOf(node: MenuNode): string[] {
  const raw = node.perms?.trim();
  return raw ? [raw] : [];
}

function walk(
  node: MenuNode,
  parentPath: string,
  trail: string[],
  out: RouteRecordRaw[],
  seenNames: Set<string>,
  firstPath: { value: string | null },
): void {
  const { path, external } = ownPathOf(node, parentPath);
  const isMenu = node.menu_type === 'C';
  const chain = [...trail, node.menu_name];

  if (isMenu && !external) {
    const view = resolveView(node);
    const component = viewIndex.get(view) ?? fallbackView();
    let name = routeName(path);
    if (seenNames.has(name)) {
      name = `${name}-${node.menu_id}`;
    }
    seenNames.add(name);
    firstPath.value ??= path;
    // 显式标成「带 component 的单视图记录」：`RouteRecordRaw` 是联合类型，
    // 直接推入对象会让 TS 挑不中成员（component 的类型含 undefined）。
    const record: RouteRecordSingleView = {
      path,
      name,
      component: component as RouteRecordSingleView['component'],
      meta: {
        title: node.menu_name,
        perms: permsOf(node),
        menuId: node.menu_id,
        icon: iconName(node),
        breadcrumb: chain.slice(0, -1),
        keepAlive: true,
      },
    };
    out.push(record);
  }

  for (const child of node.children) {
    walk(child, path, chain, out, seenNames, firstPath);
  }
}

/**
 * 框架自带菜单的显示覆盖。
 *
 * 菜单树会按角色集合缓存在 Redis 里；迁移把 `sys_menu` 修正后，旧缓存可能还会活到 TTL。
 * 这里对稳定的 `menu_id` 做一层展示兜底，保证侧栏不会继续出现早期外链菜单的旧文案。
 * 自定义菜单不受影响。
 */
const MENU_OVERRIDES: Record<number, { title?: string; path?: string }> = {
  4: { title: 'Rust 官网', path: 'https://www.rust-lang.org/' },
};

function applyMenuOverrides(node: MenuNode): MenuNode {
  const override = MENU_OVERRIDES[node.menu_id];
  if (!override) return node;
  return {
    ...node,
    menu_name: override.title ?? node.menu_name,
    path: override.path ?? node.path,
    children: node.children.map(applyMenuOverrides),
  };
}

export function buildRoutesFromMenus(menus: MenuNode[]): GeneratedRoutes {
  const routes: RouteRecordRaw[] = [];
  const seenNames = new Set<string>();
  const firstPath = { value: null as string | null };
  const normalized = menus.map(applyMenuOverrides);
  for (const node of normalized) {
    walk(node, '', [], routes, seenNames, firstPath);
  }
  const sidebar = normalized
    .map((node) => buildSidebar(node, '', []))
    .filter((item): item is SidebarMenu => item !== null);
  return { routes, menus: sidebar, firstPath: firstPath.value };
}

/** 可导航的叶子菜单（排除目录节点），供顶栏的菜单搜索用。 */
export interface NavigableMenu {
  title: string;
  path: string;
  external: boolean;
  /** 父级标题链路，搜索结果显示成「系统管理 / 用户管理」。 */
  trail: string[];
}

export function collectNavigableMenus(menus: SidebarMenu[]): NavigableMenu[] {
  const out: NavigableMenu[] = [];
  const visit = (items: SidebarMenu[], trail: string[]) => {
    for (const item of items) {
      if (item.children.length > 0) {
        visit(item.children, [...trail, item.title]);
      } else {
        out.push({ title: item.title, path: item.path, external: item.external, trail });
      }
    }
  };
  visit(menus, []);
  return out;
}
