<script setup lang="ts">
/**
 * 占位页：菜单已配、界面未做时的落点（`router/dynamic.ts` 里视图缺失会路由到这里）。
 *
 * 为什么不直接 404：菜单是后端下发的、接口也真的存在，用户点进来看到「页面不存在」
 * 会以为系统坏了。这里把三件排查信息摊开——菜单名、所需权限、路由路径——让用户知道
 * 「后端已就绪，只是前端界面属于后续批次」，也方便开发者一眼看出该建哪个文件。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import AppIcon from '@/components/AppIcon.vue';

const { t } = useI18n();
const route = useRoute();

const menuTitle = computed(() => route.meta.title ?? '');
const perms = computed(() => route.meta.perms ?? []);
const path = computed(() => route.path);
/** 视图文件应有的位置：菜单路径去掉前导斜杠再加上 `index.vue`（推导规则见 dynamic.ts）。 */
const expectedFile = computed(
  () => `src/views/${path.value.replace(/^\/+/, '').replace(/\/+$/, '')}/index.vue`,
);
</script>

<template>
  <div class="app-container">
    <div class="app-placeholder">
      <AppIcon name="Hammer" :size="30" fallback="Menu" />
      <p class="app-placeholder__title">{{ t('error.wipTitle') }}</p>
      <p class="app-placeholder__hint">{{ t('error.wipHint') }}</p>

      <dl class="app-placeholder__facts">
        <template v-if="menuTitle">
          <dt>{{ t('error.wipMenu') }}</dt>
          <dd>{{ menuTitle }}</dd>
        </template>
        <template v-if="perms.length > 0">
          <dt>{{ t('error.wipPerm') }}</dt>
          <dd>
            <code v-for="perm in perms" :key="perm">{{ perm }}</code>
          </dd>
        </template>
        <dt>{{ t('error.wipRoute') }}</dt>
        <dd>
          <code>{{ path }}</code>
        </dd>
        <dt>预期视图文件</dt>
        <dd>
          <code>{{ expectedFile }}</code>
        </dd>
      </dl>
    </div>
  </div>
</template>

<style scoped>
.app-placeholder {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--ci-space-2);
  padding: var(--ci-space-12) var(--ci-space-4);
  background: var(--ci-color-surface);
  border: 1px solid var(--ci-color-separator);
  border-radius: var(--ci-radius-lg);
  box-shadow: var(--ci-shadow-1);
  color: var(--ci-color-text-tertiary);
}

.app-placeholder__title {
  font-size: var(--ci-text-lg);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-text);
}

.app-placeholder__hint {
  max-width: 52ch;
  text-align: center;
  font-size: var(--ci-text-sm);
  line-height: var(--ci-leading-relaxed);
}

.app-placeholder__facts {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: var(--ci-space-2) var(--ci-space-4);
  margin-top: var(--ci-space-4);
  padding: var(--ci-space-4);
  border-radius: var(--ci-radius-md);
  background: var(--ci-color-surface-sunken);
  font-size: var(--ci-text-sm);
  text-align: left;
}

.app-placeholder__facts dt {
  color: var(--ci-color-text-tertiary);
}

.app-placeholder__facts dd {
  display: flex;
  flex-wrap: wrap;
  gap: var(--ci-space-2);
  margin: 0;
  color: var(--ci-color-text-secondary);
}

.app-placeholder__facts code {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
}
</style>
