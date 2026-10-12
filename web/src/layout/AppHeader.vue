<script setup lang="ts">
/**
 * 顶栏：折叠开关、面包屑、菜单搜索、外观/语言、账号菜单。
 *
 * 与侧栏同样是半透明材料——顶栏属于「外壳层」，内容从它下面滚过时要能透出来。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute, useRouter } from 'vue-router';

import AppIcon from '@/components/AppIcon.vue';
import { LOCALE_LABELS, SUPPORTED_LOCALES, type AppLocale } from '@/locales';
import { collectNavigableMenus, type NavigableMenu } from '@/router/dynamic';
import { useAppStore } from '@/stores/app';
import { useAuthStore } from '@/stores/auth';
import { usePermissionStore } from '@/stores/permission';
import { useConfirm } from '@/composables/useConfirm';

const emit = defineEmits<{ toggleSidebar: [] }>();

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const app = useAppStore();
const auth = useAuthStore();
const permission = usePermissionStore();
const { confirm } = useConfirm();

const searchRef = ref<HTMLInputElement>();
const keyword = ref('');

const navigable = computed(() => collectNavigableMenus(permission.menus));

const searchOptions = computed(() => {
  const text = keyword.value.trim().toLowerCase();
  const all = navigable.value;
  if (text === '') return all.slice(0, 8);
  return all
    .filter(
      (item) =>
        item.title.toLowerCase().includes(text) ||
        item.trail.join('/').toLowerCase().includes(text),
    )
    .slice(0, 12);
});

const breadcrumb = computed(() => {
  const trail = route.meta.breadcrumb ?? [];
  const title = route.meta.title;
  return [...trail, ...(title ? [title] : [])];
});

const themeLabel = computed(() => (app.isDark ? t('layout.themeDark') : t('layout.themeLight')));

function openMenu(item: NavigableMenu): void {
  keyword.value = '';
  if (item.external) {
    window.open(item.path, '_blank', 'noopener,noreferrer');
    return;
  }
  void router.push(item.path);
}

async function signOut(): Promise<void> {
  const agreed = await confirm(t('layout.logoutConfirm'), {
    title: t('layout.logout'),
    danger: true,
  });
  if (!agreed) return;
  await auth.signOut();
  permission.reset();
  await router.push({ path: '/login' });
}

/** ⌘K / Ctrl+K 聚焦菜单搜索：后台用户最常用的动作是「跳到某个页面」。 */
function onKeydown(event: KeyboardEvent): void {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
    event.preventDefault();
    searchRef.value?.focus();
  }
}

onMounted(() => window.addEventListener('keydown', onKeydown));
onUnmounted(() => window.removeEventListener('keydown', onKeydown));
</script>

<template>
  <header class="app-header">
    <button
      type="button"
      class="app-header__icon-button"
      :title="app.sidebarCollapsed ? t('layout.expand') : t('layout.collapse')"
      @click="emit('toggleSidebar')"
    >
      <AppIcon
        :name="app.sidebarCollapsed ? 'PanelLeftOpen' : 'PanelLeftClose'"
        :size="17"
        fallback="Menu"
      />
    </button>

    <el-breadcrumb class="app-header__breadcrumb" separator="/">
      <el-breadcrumb-item v-for="(item, index) in breadcrumb" :key="`${item}-${index}`">
        {{ item }}
      </el-breadcrumb-item>
    </el-breadcrumb>

    <div class="ci-spacer" />

    <el-select
      ref="searchRef"
      v-model="keyword"
      class="app-header__search"
      filterable
      clearable
      remote
      :remote-method="(value: string) => (keyword = value)"
      :placeholder="t('layout.searchPlaceholder')"
      :no-data-text="t('layout.searchEmpty')"
    >
      <el-option
        v-for="item in searchOptions"
        :key="item.path"
        :label="[...item.trail, item.title].join(' / ')"
        :value="item.title"
        @click="openMenu(item)"
      />
    </el-select>

    <button
      type="button"
      class="app-header__icon-button"
      :title="`${t('layout.theme')}：${themeLabel}`"
      @click="app.toggleTheme()"
    >
      <AppIcon :name="app.isDark ? 'Moon' : 'Sun'" :size="17" fallback="Sun" />
    </button>

    <el-dropdown
      trigger="click"
      @command="(value: unknown) => app.changeLocale(value as AppLocale)"
    >
      <button type="button" class="app-header__icon-button" :title="t('layout.language')">
        <AppIcon name="Languages" :size="16" fallback="Languages" />
        <span class="app-header__locale">{{ LOCALE_LABELS[app.locale] }}</span>
      </button>
      <template #dropdown>
        <el-dropdown-menu>
          <el-dropdown-item
            v-for="locale in SUPPORTED_LOCALES"
            :key="locale"
            :command="locale"
            :disabled="locale === app.locale"
          >
            {{ LOCALE_LABELS[locale] }}
          </el-dropdown-item>
        </el-dropdown-menu>
      </template>
    </el-dropdown>

    <el-dropdown trigger="click">
      <button type="button" class="app-header__user">
        <span class="app-header__avatar" aria-hidden="true">
          {{ (auth.userName || '?').slice(0, 1).toUpperCase() }}
        </span>
        <span class="app-header__user-name">{{ auth.userName }}</span>
        <AppIcon name="ChevronDown" :size="12" fallback="ChevronDown" />
      </button>
      <template #dropdown>
        <el-dropdown-menu>
          <el-dropdown-item disabled>
            {{ t('dashboard.tenant') }}：#{{ auth.tenantId ?? '-' }}
          </el-dropdown-item>
          <el-dropdown-item divided @click="signOut">
            {{ t('layout.logout') }}
          </el-dropdown-item>
        </el-dropdown-menu>
      </template>
    </el-dropdown>
  </header>
</template>

<style scoped>
.app-header {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: var(--ci-space-3);
  height: var(--ci-layout-header);
  padding: 0 var(--ci-space-4);
  background: var(--ci-material-bg);
  border-bottom: 1px solid var(--ci-color-separator);
  backdrop-filter: var(--ci-material-blur);
  -webkit-backdrop-filter: var(--ci-material-blur);
}

.app-header__icon-button {
  display: inline-flex;
  align-items: center;
  gap: var(--ci-space-1);
  height: 32px;
  padding: 0 var(--ci-space-2);
  border: none;
  border-radius: var(--ci-radius-xs);
  background: transparent;
  color: var(--ci-color-text-secondary);
  font-size: var(--ci-text-sm);
  cursor: pointer;
  transition: var(--ci-transition-control);
}

.app-header__icon-button:hover {
  background: var(--ci-color-surface-sunken);
  color: var(--ci-color-text);
}

.app-header__icon-button:active {
  transform: scale(var(--ci-press-scale));
}

.app-header__breadcrumb {
  font-size: var(--ci-text-sm);
  min-width: 0;
}

.app-header__breadcrumb :deep(.el-breadcrumb__inner) {
  color: var(--ci-color-text-secondary);
  font-weight: var(--ci-weight-regular);
}

.app-header__breadcrumb :deep(.el-breadcrumb__item:last-child .el-breadcrumb__inner) {
  color: var(--ci-color-text);
  font-weight: var(--ci-weight-medium);
}

.app-header__search {
  width: 220px;
}

.app-header__locale {
  white-space: nowrap;
}

.app-header__user {
  display: inline-flex;
  align-items: center;
  gap: var(--ci-space-2);
  height: 34px;
  padding: 0 var(--ci-space-2) 0 var(--ci-space-1);
  border: none;
  border-radius: var(--ci-radius-pill);
  background: transparent;
  color: var(--ci-color-text);
  font-size: var(--ci-text-sm);
  cursor: pointer;
  transition: var(--ci-transition-control);
}

.app-header__user:hover {
  background: var(--ci-color-surface-sunken);
}

.app-header__avatar {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  border-radius: 50%;
  background: var(--ci-color-accent-soft);
  color: var(--ci-color-accent);
  font-weight: var(--ci-weight-semibold);
}

.app-header__user-name {
  max-width: 10ch;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

@media (max-width: 900px) {
  .app-header__search,
  .app-header__user-name,
  .app-header__breadcrumb {
    display: none;
  }
}
</style>
