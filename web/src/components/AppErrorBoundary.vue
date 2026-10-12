<script setup lang="ts">
/**
 * 渲染错误边界（错误要有出处，不能白屏）。
 *
 * 为什么需要它：动态路由的页面是运行时拼出来的，任何视图在渲染期抛错，Vue 都会沿
 * 父链找 `errorCaptured`；找不到边界就一路冒到根组件，把整个应用卸载掉——用户看到
 * 白屏，连侧栏都点不动。开发期就真踩到过：视图漏引 `AppTable`，`<el-table-column>`
 * 找不到所属表格，`Cannot use 'in' operator to search for 'tableId' in null` 直接把
 * 应用打空。
 *
 * 三个决定：
 * - 兜底 UI 就地渲染，不跳错误页：侧栏与顶栏还活着，用户能切到别的页面自救。
 * - 换路由自动复位：错误基本属于某一页，离开它就不该继续看红屏。
 * - 报错细节只在开发期显示：生产环境给人话，不给终端用户一屏 `at setup (...)`。
 */
import { onErrorCaptured, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import AppIcon from '@/components/AppIcon.vue';
import { ApiError } from '@/utils/api-error';

withDefaults(
  defineProps<{
    /** 覆盖默认标题；缺省用 `error.renderTitle`。 */
    title?: string;
    hint?: string;
  }>(),
  { title: '', hint: '' },
);

const emit = defineEmits<{ error: [error: unknown] }>();

const { t } = useI18n();
const route = useRoute();

const failed = ref(false);
const detail = ref('');
/** 换 key 重挂出错子树：这是「重试」能做的最小动作（不刷整页、不丢会话）。 */
const attempt = ref(0);
const showDetail = import.meta.env.DEV;

/**
 * 返回 `false` 是 Vue 约定的「已处理」写法，它会阻止错误继续上抛。
 * 代价是 Vue 不再打印这条错误，所以这里自己 `console.error`：白屏可以兜住，线索不能丢。
 */
onErrorCaptured((error) => {
  // 接口失败不是「渲染出错」：拦截器已经弹过提示，页面本身还好好的，
  // 把它整块换成错误卡片反而把用户正在看的列表弄没了。这里只阻止它继续上抛
  // （不返回 `false` 的话 Vue 会打印 "Unhandled error during execution of..."）。
  if (error instanceof ApiError) return false;
  failed.value = true;
  detail.value = error instanceof Error ? error.message : String(error);
  console.error('[AppErrorBoundary]', error);
  emit('error', error);
  return false;
});

function reset(): void {
  failed.value = false;
  detail.value = '';
  attempt.value += 1;
}

watch(() => route.fullPath, reset);
</script>

<template>
  <div v-if="failed" class="app-error-boundary" role="alert">
    <AppIcon class="app-error-boundary__icon" name="TriangleAlert" :size="30" />
    <p class="app-error-boundary__title">{{ title || t('error.renderTitle') }}</p>
    <p class="app-error-boundary__hint">{{ hint || t('error.renderHint') }}</p>
    <pre v-if="showDetail && detail" class="app-error-boundary__detail">{{ detail }}</pre>
    <el-button type="primary" @click="reset">{{ t('common.retry') }}</el-button>
  </div>
  <div v-else :key="attempt">
    <slot />
  </div>
</template>

<style scoped>
.app-error-boundary {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--ci-space-2);
  min-height: 40vh;
  padding: var(--ci-space-8) var(--ci-space-4);
  background: var(--ci-color-surface);
  border: 1px solid var(--ci-color-separator);
  border-radius: var(--ci-radius-lg);
}

.app-error-boundary__icon {
  color: var(--ci-color-danger);
}

.app-error-boundary__title {
  font-size: var(--ci-text-lg);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-text);
}

.app-error-boundary__hint {
  max-width: 52ch;
  text-align: center;
  font-size: var(--ci-text-sm);
  line-height: var(--ci-leading-relaxed);
  color: var(--ci-color-text-secondary);
}

.app-error-boundary__detail {
  max-width: 100%;
  margin: var(--ci-space-1) 0 0;
  padding: var(--ci-space-2) var(--ci-space-3);
  overflow: auto;
  font-size: var(--ci-text-xs);
  color: var(--ci-color-danger);
  background: var(--ci-color-surface-sunken);
  border-radius: var(--ci-radius-md);
  white-space: pre-wrap;
}

.app-error-boundary .el-button {
  margin-top: var(--ci-space-3);
}
</style>
