/** 应用外壳状态：主题、语言、侧栏折叠（都是「刷新后应保持」的偏好）。 */

import { defineStore } from 'pinia';
import { computed, ref, watch } from 'vue';

import { currentLocale, setLocale, type AppLocale } from '@/locales';
import { readJson, readStorage, STORAGE_KEYS, writeJson, writeStorage } from '@/utils/storage';

export type ThemeMode = 'light' | 'dark';

function systemPrefersDark(): boolean {
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

function resolveInitialTheme(): ThemeMode {
  const stored = readStorage(STORAGE_KEYS.theme);
  if (stored === 'dark' || stored === 'light') return stored;
  return systemPrefersDark() ? 'dark' : 'light';
}

export const useAppStore = defineStore('app', () => {
  const theme = ref<ThemeMode>(resolveInitialTheme());
  const locale = ref<AppLocale>(currentLocale());
  const sidebarCollapsed = ref(readStorage(STORAGE_KEYS.sidebarCollapsed) === 'true');
  /** 用户手动折叠的菜单分组（目录）id；默认全部展开。 */
  const collapsedMenus = ref<number[]>(readJson<number[]>(STORAGE_KEYS.menuCollapsed, []));

  const isDark = computed(() => theme.value === 'dark');

  function applyTheme(): void {
    document.documentElement.dataset.theme = theme.value;
    // 只写显式选择：用户没手动选过时不要写死，否则系统主题切换后会失效。
    if (readStorage(STORAGE_KEYS.theme)) {
      writeStorage(STORAGE_KEYS.theme, theme.value);
    }
  }

  function setTheme(next: ThemeMode): void {
    theme.value = next;
    writeStorage(STORAGE_KEYS.theme, next);
    applyTheme();
  }

  function toggleTheme(): void {
    setTheme(theme.value === 'dark' ? 'light' : 'dark');
  }

  function changeLocale(next: AppLocale): void {
    locale.value = next;
    setLocale(next);
  }

  function toggleSidebar(): void {
    sidebarCollapsed.value = !sidebarCollapsed.value;
  }

  function toggleMenuCollapsed(menuId: number): void {
    const index = collapsedMenus.value.indexOf(menuId);
    if (index >= 0) {
      collapsedMenus.value.splice(index, 1);
    } else {
      collapsedMenus.value.push(menuId);
    }
  }

  watch(sidebarCollapsed, (value) => writeStorage(STORAGE_KEYS.sidebarCollapsed, String(value)));
  watch(collapsedMenus, (value) => writeJson(STORAGE_KEYS.menuCollapsed, value), { deep: true });

  /** 系统主题变化时跟随——前提是用户没有显式选择过。 */
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', (event) => {
    if (!readStorage(STORAGE_KEYS.theme)) {
      theme.value = event.matches ? 'dark' : 'light';
      applyTheme();
    }
  });

  applyTheme();

  return {
    theme,
    locale,
    isDark,
    sidebarCollapsed,
    collapsedMenus,
    setTheme,
    toggleTheme,
    changeLocale,
    toggleSidebar,
    toggleMenuCollapsed,
  };
});
