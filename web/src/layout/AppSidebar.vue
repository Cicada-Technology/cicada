<script setup lang="ts">
/**
 * 侧栏：品牌区 + 菜单（来自 `GET /auth/routers`，走 `stores/permission`）。
 *
 * 侧栏本身是**半透明材料**（`backdrop-filter`）：它是「浮在内容之上的一层」，
 * 而不是另一块面板。内容滚动时能透出下方的层次感（半透明材料表达层级）。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import AppEmpty from '@/components/AppEmpty.vue';
import AppIcon from '@/components/AppIcon.vue';
import { useAppStore } from '@/stores/app';
import { usePermissionStore } from '@/stores/permission';

import AppNavList from './AppNavList.vue';

defineProps<{ collapsed?: boolean; mobile?: boolean }>();

const emit = defineEmits<{ navigate: [] }>();

const { t } = useI18n();
const app = useAppStore();
const permission = usePermissionStore();
/** 横版标志按主题切换；收起态用图形标，颜色在深浅背景上都成立。 */
const brandLogo = computed(() => (app.isDark ? '/brand/logo-dark.svg' : '/brand/logo.svg'));
</script>

<template>
  <nav class="app-sidebar" :class="{ 'is-collapsed': collapsed, 'is-mobile': mobile }">
    <div class="app-sidebar__brand">
      <img
        v-if="!collapsed"
        class="app-sidebar__logo"
        :src="brandLogo"
        :alt="t('app.brand')"
        draggable="false"
      />
      <img
        v-else
        class="app-sidebar__logo-mark"
        src="/brand/logo-mark.svg"
        :alt="t('app.brand')"
        draggable="false"
      />
    </div>

    <el-scrollbar class="app-sidebar__scroll">
      <AppNavList
        v-if="permission.menus.length > 0"
        :items="permission.menus"
        :collapsed="collapsed"
        @navigate="emit('navigate')"
      />
      <AppIcon
        v-else-if="collapsed"
        class="app-sidebar__empty"
        name="Lock"
        :size="18"
        fallback="Lock"
      />
      <AppEmpty v-else icon="Lock" :title="t('layout.noMenu')" :hint="t('layout.noMenuHint')" />
    </el-scrollbar>

    <div v-if="!collapsed" class="app-sidebar__footer">
      <button
        type="button"
        class="app-sidebar__collapse"
        :title="t('layout.collapse')"
        @click="app.toggleSidebar()"
      >
        <AppIcon name="ChevronsLeft" :size="14" fallback="Ellipsis" />
        <span>{{ t('layout.collapse') }}</span>
      </button>
    </div>
  </nav>
</template>

<style scoped>
.app-sidebar {
  display: flex;
  flex-direction: column;
  width: var(--ci-layout-sidebar);
  height: 100%;
  padding: var(--ci-space-3) var(--ci-space-2);
  gap: var(--ci-space-2);
  background: var(--ci-material-bg);
  border-right: 1px solid var(--ci-color-separator);
  backdrop-filter: var(--ci-material-blur);
  -webkit-backdrop-filter: var(--ci-material-blur);
  transition: width var(--ci-duration-base) var(--ci-ease-damped);
}

.app-sidebar.is-collapsed {
  width: var(--ci-layout-sidebar-collapsed);
}

.app-sidebar.is-mobile {
  width: 100%;
  border-right: none;
}

/* 收起态只留图形标：横版字标在 68px 里放不下。 */
.app-sidebar.is-collapsed .app-sidebar__brand {
  justify-content: center;
  padding: 0;
}

.app-sidebar__empty {
  display: block;
  margin: var(--ci-space-5) auto;
  color: var(--ci-color-text-tertiary);
}

.app-sidebar__brand {
  display: flex;
  align-items: center;
  gap: var(--ci-space-2);
  height: 40px;
  padding: 0 var(--ci-space-3);
  flex-shrink: 0;
}

.app-sidebar__logo,
.app-sidebar__logo-mark {
  display: block;
  flex-shrink: 0;
}

.app-sidebar__logo {
  width: 136px;
  height: auto;
}

.app-sidebar__logo-mark {
  width: 34px;
  height: 34px;
}

.app-sidebar__scroll {
  flex: 1 1 auto;
  min-height: 0;
}

.app-sidebar__footer {
  flex-shrink: 0;
  padding-top: var(--ci-space-1);
  border-top: 1px solid var(--ci-color-separator);
}

.app-sidebar__collapse {
  display: flex;
  align-items: center;
  gap: var(--ci-space-2);
  width: 100%;
  padding: var(--ci-space-2) var(--ci-space-3);
  border: none;
  border-radius: var(--ci-radius-sm);
  background: transparent;
  color: var(--ci-color-text-tertiary);
  font-size: var(--ci-text-sm);
  cursor: pointer;
  transition: var(--ci-transition-control);
}

.app-sidebar__collapse:hover {
  background: var(--ci-color-surface-sunken);
  color: var(--ci-color-text-secondary);
}
</style>
