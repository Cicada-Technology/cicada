<script setup lang="ts">
/**
 * 空状态。
 *
 * ux-admin-portal 的取向：空表不能只写「暂无数据」——要告诉用户**下一步做什么**，
 * 所以这里固定给出一个可选的行动入口（新建 / 清除筛选）。
 */
import { useI18n } from 'vue-i18n';

import AppIcon from './AppIcon.vue';

const { t } = useI18n();

defineProps<{
  /** 主文案；缺省用通用「暂无数据」。 */
  title?: string;
  /** 补充说明；缺省提示换筛选条件或新建。 */
  hint?: string;
  icon?: string;
}>();
</script>

<template>
  <div class="app-empty">
    <AppIcon :name="icon || 'Box'" :size="28" fallback="Box" />
    <p class="app-empty__title">{{ title || t('common.empty') }}</p>
    <p class="app-empty__hint">{{ hint || t('common.emptyHint') }}</p>
    <div v-if="$slots.default" class="app-empty__actions">
      <slot />
    </div>
  </div>
</template>

<style scoped>
.app-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--ci-space-2);
  padding: var(--ci-space-10) var(--ci-space-4);
  color: var(--ci-color-text-tertiary);
}

.app-empty__title {
  font-size: var(--ci-text-base);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-text-secondary);
}

.app-empty__hint {
  font-size: var(--ci-text-sm);
  max-width: 42ch;
  text-align: center;
  line-height: var(--ci-leading-snug);
}

.app-empty__actions {
  margin-top: var(--ci-space-3);
}
</style>
