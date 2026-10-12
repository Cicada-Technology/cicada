<script setup lang="ts">
/**
 * 页面外壳：标题区 + 筛选区 + 内容区。
 *
 * 为什么要有它：代码生成器产出的页面根节点是 `.app-container`（样式在 base.css 里给），
 * 手写页面若各自排版，间距与卡片边界会慢慢长歪。用同一个外壳，改一处全局一致。
 *
 * 布局节奏（Apple 的层级观感）：标题区无卡片（属于页面背景层），筛选与内容各是一张卡片。
 */
defineProps<{
  /** 页面标题；缺省用路由 meta.title（由路由守卫填好后从 `useRoute` 取也可）。 */
  title?: string;
  description?: string;
  /** 内容区是否包一层卡片；表格页一般是 false（表格自带卡片）。 */
  card?: boolean;
  /** 占满剩余高度（详情页/看板会更舒服）。 */
  fill?: boolean;
}>();
</script>

<template>
  <section class="app-page" :class="{ 'app-page--fill': fill }">
    <header class="app-page__header">
      <div class="app-page__heading">
        <h1 class="app-page__title">{{ title }}</h1>
        <p v-if="description" class="app-page__description">{{ description }}</p>
      </div>
      <div class="app-page__actions">
        <slot name="actions" />
      </div>
    </header>

    <slot name="notice" />

    <div v-if="$slots.search" class="app-page__panel">
      <slot name="search" />
    </div>

    <div class="app-page__body" :class="{ 'app-page__body--card': card }">
      <slot />
    </div>

    <footer v-if="$slots.footer" class="app-page__footer">
      <slot name="footer" />
    </footer>
  </section>
</template>

<style scoped>
.app-page {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-4);
  min-width: 0;
}

.app-page--fill {
  height: 100%;
}

.app-page__header {
  display: flex;
  align-items: flex-start;
  gap: var(--ci-space-4);
  padding-top: var(--ci-space-1);
}

.app-page__heading {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-1);
  min-width: 0;
}

.app-page__title {
  font-size: var(--ci-text-2xl);
  font-weight: var(--ci-weight-semibold);
  line-height: var(--ci-leading-tight);
  letter-spacing: var(--ci-tracking-2xl);
  color: var(--ci-color-text);
}

.app-page__description {
  font-size: var(--ci-text-sm);
  color: var(--ci-color-text-secondary);
  line-height: var(--ci-leading-snug);
  max-width: 72ch;
}

.app-page__actions {
  display: flex;
  align-items: center;
  gap: var(--ci-space-2);
  margin-left: auto;
  flex-shrink: 0;
}

.app-page__panel,
.app-page__body--card {
  background: var(--ci-color-surface);
  border: 1px solid var(--ci-color-separator);
  border-radius: var(--ci-radius-lg);
  box-shadow: var(--ci-shadow-1);
}

.app-page__panel {
  padding: var(--ci-space-4);
}

.app-page__body {
  min-width: 0;
}

.app-page__body--card {
  padding: var(--ci-space-5);
}

.app-page__footer {
  display: flex;
  align-items: center;
  gap: var(--ci-space-2);
  justify-content: flex-end;
}
</style>
