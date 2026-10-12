<script setup lang="ts" generic="TRow extends object">
/**
 * 表格 + 分页 + 空状态 + 行选择：表格/表单/弹层/抽屉等高频组件封装成项目自有
 * 组件。
 *
 * 设计取舍（ux-admin-portal）：
 * - 空表给「下一步做什么」，不是一句「暂无数据」——空状态插槽由调用方给行动入口。
 * - 行选择一旦发生，批量操作条只在**有选中项时**出现，不常驻占位。
 * - 列由调用方用 `<el-table-column>` 声明（生成器产物也是这么写的），组件不发明列 DSL。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import AppEmpty from './AppEmpty.vue';

const props = withDefaults(
  defineProps<{
    rows: TRow[];
    loading?: boolean;
    /** 分页总数；传 `null` 表示不分页（树表/全量列表）。 */
    total?: number | null;
    page?: number;
    pageSize?: number;
    /** 行键：树表与跨页选择需要它，一般传主键字段名。 */
    rowKey?: string;
    selectable?: boolean;
    /** 树表模式：使用 `children` 递归。 */
    tree?: boolean;
    expandAll?: boolean;
    emptyTitle?: string;
    emptyHint?: string;
    /** 可选页容量。 */
    pageSizes?: number[];
  }>(),
  {
    loading: false,
    total: null,
    page: 1,
    pageSize: 20,
    rowKey: undefined,
    selectable: false,
    tree: false,
    expandAll: true,
    emptyTitle: '',
    emptyHint: '',
    pageSizes: () => [10, 20, 50, 100],
  },
);

const emit = defineEmits<{
  'update:page': [value: number];
  'update:pageSize': [value: number];
  'selection-change': [rows: TRow[]];
}>();

const { t } = useI18n();

const selection = defineModel<TRow[]>('selection', { default: () => [] });

const paged = computed(() => props.total !== null && props.total !== undefined);

function onSelectionChange(rows: TRow[]): void {
  selection.value = rows;
  emit('selection-change', rows);
}

function onPageChange(value: number): void {
  emit('update:page', value);
}

function onPageSizeChange(value: number): void {
  emit('update:pageSize', value);
}

/**
 * 鼠标滚轮横向滚动。
 *
 * 宽表在 1366/1440 宽度下会超出卡片，Element Plus 的横向滚动条默认只在 hover 时出现，
 * 且滚轮只作用于页面纵向。这里补两件事：Shift + 滚轮、以及触控板的横向 deltaX，
 * 都转成表格自身的横向滚动；没有横向溢出时完全放行，不抢页面纵向滚动。
 */
function onWheel(event: WheelEvent): void {
  const root = event.currentTarget as HTMLElement | null;
  const wrap = root?.querySelector<HTMLElement>('.el-scrollbar__wrap');
  if (!wrap || wrap.scrollWidth <= wrap.clientWidth) return;
  const horizontal = event.shiftKey || Math.abs(event.deltaX) > Math.abs(event.deltaY);
  if (!horizontal) return;
  const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
  event.preventDefault();
  wrap.scrollLeft += delta;
}
</script>

<template>
  <div class="app-table" @wheel="onWheel">
    <div v-if="selectable && selection.length > 0" class="app-table__selection">
      <span class="app-table__selection-count">
        {{ t('common.selected', { count: selection.length }) }}
      </span>
      <el-button link type="primary" @click="selection = []">
        {{ t('table.clearSelection') }}
      </el-button>
      <div class="ci-spacer" />
      <slot name="bulk" :selection="selection" />
    </div>

    <el-table
      v-loading="loading"
      class="app-table__table"
      scrollbar-always-on
      :data="rows"
      :row-key="rowKey"
      :default-expand-all="tree ? expandAll : undefined"
      :tree-props="tree ? { children: 'children' } : undefined"
      @selection-change="onSelectionChange"
    >
      <el-table-column
        v-if="selectable"
        type="selection"
        width="46"
        :reserve-selection="!!rowKey"
      />
      <slot />
      <template #empty>
        <AppEmpty :title="emptyTitle" :hint="emptyHint">
          <slot name="empty-action" />
        </AppEmpty>
      </template>
    </el-table>

    <div v-if="paged" class="app-table__footer">
      <el-pagination
        :current-page="page"
        :page-size="pageSize"
        :page-sizes="pageSizes"
        :total="total ?? 0"
        background
        layout="total, sizes, prev, pager, next, jumper"
        @current-change="onPageChange"
        @size-change="onPageSizeChange"
      />
    </div>
  </div>
</template>

<style scoped>
.app-table {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

/* 表格自带卡片边界（与 `.app-container > .el-table` 的样式一致）。 */
.app-table__table {
  background: var(--ci-color-surface);
  border: 1px solid var(--ci-color-separator);
  border-radius: var(--ci-radius-lg);
  overflow: hidden;
}

/* 横向滚动条常驻且加高，宽表在小屏上不会“看起来没有更多列”。 */
.app-table__table :deep(.el-scrollbar__bar.is-horizontal) {
  height: 8px;
}

.app-table__table :deep(.el-scrollbar__bar.is-horizontal .el-scrollbar__thumb) {
  background: var(--ci-color-text-tertiary);
  opacity: 0.55;
}

/* 有批量条时把圆角让给批量条，避免出现双层边框。 */
.app-table__selection + .app-table__table {
  border-top-left-radius: 0;
  border-top-right-radius: 0;
}

.app-table__selection {
  display: flex;
  align-items: center;
  gap: var(--ci-space-2);
  padding: var(--ci-space-2) var(--ci-space-4);
  background: var(--ci-color-accent-soft);
  border: 1px solid var(--ci-color-separator);
  border-bottom: none;
  border-top-left-radius: var(--ci-radius-lg);
  border-top-right-radius: var(--ci-radius-lg);
}

.app-table__selection-count {
  font-size: var(--ci-text-sm);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-accent);
}

.app-table__footer {
  display: flex;
  justify-content: flex-end;
  padding-top: var(--ci-space-3);
}
</style>
