<script setup lang="ts">
/**
 * 树选择器（部门 / 菜单上级 / 树表父节点）。
 *
 * 默认 `check-strictly`：这里选的是**节点本身**（绑定 id），不是聚合后代——
 * 与 Element 默认的「父子联动」语义正好相反，写反了会让用户选中一个无法预料的集合。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

export interface TreeSelectNode {
  [key: string]: unknown;
  children?: TreeSelectNode[];
}

const props = withDefaults(
  defineProps<{
    modelValue?: number | string | null;
    data: TreeSelectNode[];
    labelKey?: string;
    valueKey?: string;
    childrenKey?: string;
    placeholder?: string;
    clearable?: boolean;
    disabled?: boolean;
    /** 允许选中任意节点（含目录），部门/菜单上级都需要。 */
    checkStrictly?: boolean;
    width?: string;
  }>(),
  {
    modelValue: null,
    labelKey: 'label',
    valueKey: 'value',
    childrenKey: 'children',
    placeholder: '',
    clearable: true,
    disabled: false,
    checkStrictly: true,
    width: '100%',
  },
);

const emit = defineEmits<{ 'update:modelValue': [value: number | string | null] }>();

const { t } = useI18n();

const treeProps = computed(() => ({
  label: props.labelKey,
  value: props.valueKey,
  children: props.childrenKey,
}));

const resolvedPlaceholder = computed(() => props.placeholder || t('common.pleaseChoose'));
</script>

<template>
  <el-tree-select
    class="app-tree-select"
    :model-value="modelValue ?? undefined"
    :data="data"
    :props="treeProps"
    :placeholder="resolvedPlaceholder"
    :clearable="clearable"
    :disabled="disabled"
    :check-strictly="checkStrictly"
    :render-after-expand="false"
    check-on-click-node
    default-expand-all
    :style="{ width }"
    @update:model-value="emit('update:modelValue', $event as number | string | null)"
  />
</template>

<style scoped>
.app-tree-select {
  min-width: 0;
}
</style>
