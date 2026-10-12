/**
 * 本地存储的薄封装。
 *
 * 只封装三件容易踩坑的事：统一的键前缀、隐私模式/禁用存储时的降级、以及 JSON 读写。
 * 不做响应式——需要响应式的地方用 pinia store（见 `stores/app.ts`）。
 */

export const STORAGE_KEYS = {
  /** 访问令牌（Bearer）。与后端 `/auth/*` 的响应字段同名。 */
  accessToken: 'cicada.access_token',
  /** 刷新令牌（不透明，内含会话 id）。 */
  refreshToken: 'cicada.refresh_token',
  locale: 'cicada.locale',
  /** 显式主题；缺省时按系统偏好。 */
  theme: 'cicada.theme',
  sidebarCollapsed: 'cicada.sidebar_collapsed',
  /** 用户主动收起的菜单分组 id 列表。 */
  menuCollapsed: 'cicada.menu_collapsed',
} as const;

export type StorageKey = (typeof STORAGE_KEYS)[keyof typeof STORAGE_KEYS];

function backing(): Storage | null {
  try {
    // 触碰一次才会在 Safari 隐私模式下抛异常，因此这里必须真正访问 `length`。
    void window.localStorage.length;
    return window.localStorage;
  } catch {
    return null;
  }
}

export function readStorage(key: StorageKey): string | null {
  return backing()?.getItem(key) ?? null;
}

export function writeStorage(key: StorageKey, value: string): void {
  try {
    backing()?.setItem(key, value);
  } catch {
    // 配额满或禁用存储：忽略。令牌丢失只会导致重新登录，不该让写操作抛错。
  }
}

export function removeStorage(key: StorageKey): void {
  try {
    backing()?.removeItem(key);
  } catch {
    // 同上。
  }
}

export function readJson<T>(key: StorageKey, fallback: T): T {
  const raw = readStorage(key);
  if (!raw) return fallback;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

export function writeJson(key: StorageKey, value: unknown): void {
  writeStorage(key, JSON.stringify(value));
}
