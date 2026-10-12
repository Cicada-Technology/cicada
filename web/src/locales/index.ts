import { createI18n } from 'vue-i18n';

import { readStorage, writeStorage, STORAGE_KEYS } from '@/utils/storage';

import enUS from './en-US';
import zhCN from './zh-CN';

export const SUPPORTED_LOCALES = ['zh-CN', 'en-US'] as const;
export type AppLocale = (typeof SUPPORTED_LOCALES)[number];

export const LOCALE_LABELS: Record<AppLocale, string> = {
  'zh-CN': '简体中文',
  'en-US': 'English',
};

function resolveInitialLocale(): AppLocale {
  const stored = readStorage(STORAGE_KEYS.locale);
  if (stored && (SUPPORTED_LOCALES as readonly string[]).includes(stored)) {
    return stored as AppLocale;
  }
  const preferred = navigator.language;
  return preferred && preferred.toLowerCase().startsWith('en') ? 'en-US' : 'zh-CN';
}

export const i18n = createI18n({
  // Composition API 模式：`legacy: false` 才有 `i18n.global` 的 `Composer` 语义。
  legacy: false,
  locale: resolveInitialLocale(),
  fallbackLocale: 'zh-CN',
  messages: { 'zh-CN': zhCN, 'en-US': enUS },
  // 缺键只在开发期提示：生产环境静默用回退语言，不把控制台刷满。
  missingWarn: import.meta.env.DEV,
  fallbackWarn: import.meta.env.DEV,
});

export function currentLocale(): AppLocale {
  return i18n.global.locale.value as AppLocale;
}

export function setLocale(locale: AppLocale): void {
  i18n.global.locale.value = locale;
  writeStorage(STORAGE_KEYS.locale, locale);
  document.documentElement.lang = locale;
}

/** 供非组件上下文（拦截器、指令）取文案。 */
export function translate(key: string, named?: Record<string, unknown>): string {
  return named ? i18n.global.t(key, named) : i18n.global.t(key);
}
