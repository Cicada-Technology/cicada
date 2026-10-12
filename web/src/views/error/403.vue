<script setup lang="ts">
/**
 * 403：说清楚「为什么进不来」与「怎么办」。
 *
 * 只说「没有权限」会让用户反复重试同一个链接；这里明确写出判定依据（角色授权）与
 * 唯一的解法（联系管理员），并提供一条确定可用的出路。
 */
import { useI18n } from 'vue-i18n';
import { useRoute, useRouter } from 'vue-router';

import AppIcon from '@/components/AppIcon.vue';
import { HOME_PATH } from '@/router/routes';

const { t } = useI18n();
const route = useRoute();
const router = useRouter();

/** 被拒的路径：从 URL 直接输入时用户需要知道是哪一个地址没权限。 */
const blockedPath = String(route.query.from ?? '');
</script>

<template>
  <div class="app-error">
    <AppIcon name="Lock" :size="34" fallback="Menu" />
    <h1 class="app-error__code">403</h1>
    <p class="app-error__title">{{ t('error.forbiddenTitle') }}</p>
    <p class="app-error__hint">{{ t('error.forbiddenHint') }}</p>
    <p v-if="blockedPath" class="app-error__path">{{ blockedPath }}</p>
    <div class="app-error__actions">
      <el-button type="primary" @click="router.push(HOME_PATH)">
        {{ t('error.backHome') }}
      </el-button>
      <el-button @click="router.back()">{{ t('error.backPrev') }}</el-button>
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

.app-error__path {
  padding: var(--ci-space-1) var(--ci-space-3);
  border-radius: var(--ci-radius-sm);
  background: var(--ci-color-surface-sunken);
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
}

.app-error__actions {
  display: flex;
  gap: var(--ci-space-2);
  margin-top: var(--ci-space-4);
}
</style>
