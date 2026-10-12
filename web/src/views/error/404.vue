<script setup lang="ts">
/**
 * 404：不是「页面不存在」四个字就完事。
 *
 * 走到这里通常有两种情况：链接过期，或菜单被移除。两种情况都需要一条出路，
 * 所以固定给「回到概览」与「返回上一页」两个动作（ux-admin-portal：错误页也要有下一步）。
 */
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';

import AppIcon from '@/components/AppIcon.vue';
import { HOME_PATH } from '@/router/routes';

const { t } = useI18n();
const router = useRouter();

function goBack(): void {
  // 从外部链接直接进来时没有历史可退，退回概览而不是卡住。
  if (window.history.length > 1) router.back();
  else void router.push(HOME_PATH);
}
</script>

<template>
  <div class="app-error">
    <AppIcon name="Compass" :size="34" fallback="Menu" />
    <h1 class="app-error__code">404</h1>
    <p class="app-error__title">{{ t('error.notFoundTitle') }}</p>
    <p class="app-error__hint">{{ t('error.notFoundHint') }}</p>
    <div class="app-error__actions">
      <el-button type="primary" @click="router.push(HOME_PATH)">
        {{ t('error.backHome') }}
      </el-button>
      <el-button @click="goBack">{{ t('error.backPrev') }}</el-button>
    </div>
  </div>
</template>

<style scoped>
.app-error {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--ci-space-2);
  min-height: 60vh;
  padding: var(--ci-space-8) var(--ci-space-4);
  color: var(--ci-color-text-tertiary);
}

.app-error__code {
  font-size: var(--ci-text-4xl);
  font-weight: var(--ci-weight-semibold);
  letter-spacing: var(--ci-tracking-4xl);
  color: var(--ci-color-text);
}

.app-error__title {
  font-size: var(--ci-text-lg);
  color: var(--ci-color-text-secondary);
}

.app-error__hint {
  max-width: 46ch;
  text-align: center;
  font-size: var(--ci-text-sm);
  line-height: var(--ci-leading-relaxed);
}

.app-error__actions {
  display: flex;
  gap: var(--ci-space-2);
  margin-top: var(--ci-space-4);
}
</style>
