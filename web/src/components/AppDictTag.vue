<script setup lang="ts">
/**
 * 字典值标签：表格列里显示 `sys_dict_data` 的标签而不是裸值。
 *
 * 例：`<AppDictTag dict-type="sys_normal_disable" :value="row.status" />`
 * 找不到标签时退回原始值（而不是留空），保证列不会莫名出现空白。
 */
import { computed } from 'vue';

import { useDictStore } from '@/stores/dict';
import { useDictOptions } from '@/composables/useDict';

const props = defineProps<{
  dictType: string;
  value?: string | number | null;
  /** 不显示为标签，只显示文本（详情页/表单回显用）。 */
  plain?: boolean;
}>();

const store = useDictStore();
// 先触发加载：标签样式也在同一份数据里，光靠 store 同步读取会拿不到。
useDictOptions(() => props.dictType);

const target = computed(() => String(props.value ?? ''));

const label = computed(() => {
  if (target.value === '') return '';
  return store.labelOf(props.dictType, target.value) ?? target.value;
});

/** `list_class` 是后端存好的 Element Plus 标签样式名。 */
const tagType = computed(() => {
  const name = store.classOf(props.dictType, target.value);
  const allowed = ['success', 'warning', 'danger', 'info', 'primary'];
  return name && allowed.includes(name) ? name : 'info';
});
</script>

<template>
  <span v-if="target === ''" class="ci-tertiary">—</span>
  <span v-else-if="plain">{{ label }}</span>
  <el-tag v-else :type="tagType" size="small" effect="light" round>{{ label }}</el-tag>
</template>
