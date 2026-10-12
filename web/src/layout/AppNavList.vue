<script setup lang="ts">
/**
 * 侧栏导航（递归）。
 *
 * 为什么不用 `el-menu`：Element 的菜单把层级、高亮、折叠都写死在它的 DOM 结构里，
 * 想做成 Apple 观感就得层层覆盖（还覆盖不干净）。这里用 `nav > ul > li > button` 自己写，
 * 语义与键盘可达性反而更清楚：`aria-current` 标当前项、`aria-expanded` 标分组展开态。
 */
import { computed } from 'vue';
import { useRoute, useRouter } from 'vue-router';

import AppIcon from '@/components/AppIcon.vue';
import type { SidebarMenu } from '@/router/dynamic';
import { useAppStore } from '@/stores/app';

const props = withDefaults(
  defineProps<{
    items: SidebarMenu[];
    depth?: number;
    /** 收起态（只显示图标）；只在最外层生效。 */
    collapsed?: boolean;
  }>(),
  { depth: 0, collapsed: false },
);

const emit = defineEmits<{ navigate: [] }>();

const route = useRoute();
const router = useRouter();
const app = useAppStore();

function isActive(item: SidebarMenu): boolean {
  if (item.children.length > 0) return item.children.some(isActive);
  return item.path === route.path;
}

function isExpanded(item: SidebarMenu): boolean {
  // 默认展开：只有用户手动折叠过才收起，避免「进来全是折叠的」。
  return !app.collapsedMenus.includes(item.id);
}

function toggle(item: SidebarMenu): void {
  if (item.children.length === 0) {
    if (item.external) {
      window.open(item.path, '_blank', 'noopener,noreferrer');
    } else {
      void router.push(item.path);
    }
    emit('navigate');
    return;
  }
  // 收起态没有空间展开子菜单：目录直接去第一个可见子页。
  if (props.collapsed) {
    void router.push(item.path);
    emit('navigate');
    return;
  }
  app.toggleMenuCollapsed(item.id);
}

const indentStyle = computed(() => ({
  '--ci-nav-indent': `${props.collapsed ? 0 : props.depth * 12}px`,
}));
</script>

<template>
  <ul class="app-nav" :style="indentStyle">
    <li v-for="item in items" :key="item.id" class="app-nav__item">
      <button
        type="button"
        class="app-nav__button"
        :class="[
          `is-tone-${item.tone}`,
          {
            'is-active': item.children.length === 0 && isActive(item),
            'is-ancestor': item.children.length > 0 && isActive(item),
            'is-collapsed': collapsed,
          },
        ]"
        :aria-current="item.children.length === 0 && isActive(item) ? 'page' : undefined"
        :aria-expanded="
          collapsed ? undefined : item.children.length > 0 ? isExpanded(item) : undefined
        "
        :aria-label="collapsed ? item.title : undefined"
        :title="item.title"
        @click="toggle(item)"
      >
        <AppIcon class="app-nav__icon" :name="item.icon" :size="17" fallback="Menu" />
        <span v-if="!collapsed" class="app-nav__label">{{ item.title }}</span>
        <AppIcon
          v-if="!collapsed && item.children.length > 0"
          class="app-nav__chevron"
          :name="isExpanded(item) ? 'ChevronDown' : 'ChevronRight'"
          :size="12"
          fallback="ChevronRight"
        />
        <AppIcon
          v-if="!collapsed && item.external"
          class="app-nav__external"
          name="ExternalLink"
          :size="12"
          fallback="ExternalLink"
        />
      </button>

      <AppNavList
        v-if="!collapsed && item.children.length > 0 && isExpanded(item)"
        :items="item.children"
        :depth="depth + 1"
        :collapsed="collapsed"
        @navigate="emit('navigate')"
      />
    </li>
  </ul>
</template>

<style scoped>
.app-nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding-left: var(--ci-nav-indent, 0);
}

.app-nav__button {
  --ci-nav-icon-color: var(--ci-nav-tone-neutral);

  display: flex;
  align-items: center;
  gap: var(--ci-space-3);
  width: 100%;
  padding: var(--ci-space-2) var(--ci-space-3);
  border: none;
  border-radius: var(--ci-radius-sm);
  background: transparent;
  color: var(--ci-color-text-secondary);
  font-size: var(--ci-text-base);
  font-weight: var(--ci-weight-regular);
  letter-spacing: var(--ci-tracking-base);
  text-align: left;
  cursor: pointer;
  transition: var(--ci-transition-control);
}

.app-nav__button.is-tone-system {
  --ci-nav-icon-color: var(--ci-nav-tone-system);
}

.app-nav__button.is-tone-monitor {
  --ci-nav-icon-color: var(--ci-nav-tone-monitor);
}

.app-nav__button.is-tone-tool {
  --ci-nav-icon-color: var(--ci-nav-tone-tool);
}

.app-nav__button.is-tone-platform {
  --ci-nav-icon-color: var(--ci-nav-tone-platform);
}

.app-nav__button.is-tone-notify {
  --ci-nav-icon-color: var(--ci-nav-tone-notify);
}

.app-nav__button:hover {
  background: var(--ci-color-surface-sunken);
  color: var(--ci-color-text);
}

.app-nav__button:active {
  transform: scale(var(--ci-press-scale));
}

.app-nav__button.is-active {
  /* 不支持 color-mix 的浏览器退回中性强调底，不影响可读性。 */
  background: var(--ci-color-accent-soft);
  background: color-mix(in srgb, var(--ci-nav-icon-color) 12%, transparent);
  color: var(--ci-nav-icon-color);
  font-weight: var(--ci-weight-medium);
}

.app-nav__button.is-ancestor {
  color: var(--ci-color-text);
  font-weight: var(--ci-weight-medium);
}

/* 收起态看不到子项，用底色标出「当前页属于哪个顶级模块」。 */
.app-nav__button.is-collapsed.is-ancestor {
  background: var(--ci-color-accent-soft);
  background: color-mix(in srgb, var(--ci-nav-icon-color) 12%, transparent);
}

.app-nav__button.is-collapsed {
  justify-content: center;
  padding: var(--ci-space-2);
}

.app-nav__icon,
.app-nav__chevron,
.app-nav__external {
  flex-shrink: 0;
}

.app-nav__icon {
  color: var(--ci-nav-icon-color);
  opacity: 0.85;
  transition: opacity var(--ci-duration-fast) var(--ci-ease-out);
}

.app-nav__button:hover .app-nav__icon,
.app-nav__button.is-active .app-nav__icon,
.app-nav__button.is-ancestor .app-nav__icon {
  opacity: 1;
}

.app-nav__chevron,
.app-nav__external {
  color: var(--ci-color-text-tertiary);
}

.app-nav__chevron {
  margin-left: auto;
}

.app-nav__label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

@media (prefers-reduced-motion: reduce) {
  .app-nav__button:active {
    transform: none;
  }
}
</style>
