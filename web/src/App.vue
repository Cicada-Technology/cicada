<script setup lang="ts">
/**
 * 应用根组件。
 *
 * 只做一件事：把 Element Plus 的内置文案与当前语言对齐（分页「共 x 条」、日期选择器
 * 的月份名、确认框按钮等）。页面级文案走 vue-i18n，组件库文案只能通过 `ElConfigProvider`
 * 注入——两套并存是刻意的：把 Element 的 60 多种语言表抄进 locales 没有意义。
 */
import { ElConfigProvider } from 'element-plus';
import en from 'element-plus/es/locale/lang/en';
import zhCn from 'element-plus/es/locale/lang/zh-cn';
import { computed } from 'vue';

import { useAppStore } from '@/stores/app';
import AppErrorBoundary from '@/components/AppErrorBoundary.vue';

const app = useAppStore();

const elementLocale = computed(() => (app.locale === 'en-US' ? en : zhCn));
</script>

<template>
  <ElConfigProvider :locale="elementLocale">
    <!-- 根边界是最后一道闸：外壳自身出错时也得有出路，而不是白屏。 -->
    <AppErrorBoundary>
      <RouterView />
    </AppErrorBoundary>
  </ElConfigProvider>
</template>
