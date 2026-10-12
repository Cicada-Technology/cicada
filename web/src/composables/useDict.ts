/**
 * 字典选项的取用入口。
 *
 * 页面不直接碰 `useDictStore`：这里把「类型变化 → 重新加载」「未给类型 → 空选项」
 * 这两条容易漏掉的边界收在一处（表格列、表单控件、筛选器三处都要用）。
 */

import { computed, ref, toValue, watch, type MaybeRefOrGetter } from 'vue';

import { useDictStore, type DictOption } from '@/stores/dict';

export type DictTypeSource = MaybeRefOrGetter<string | null | undefined>;

export function useDictOptions(dictType: DictTypeSource) {
  const store = useDictStore();
  const options = ref<DictOption[]>([]);

  watch(
    () => toValue(dictType),
    async (type) => {
      options.value = type ? await store.load(type) : [];
    },
    { immediate: true },
  );

  return options;
}

/** 值 → 标签；查不到时回退成原始值，避免表格里出现空单元格。 */
export function useDictLabel(dictType: DictTypeSource, value: MaybeRefOrGetter<unknown>) {
  const options = useDictOptions(dictType);
  return computed(() => {
    const target = String(toValue(value) ?? '');
    if (target === '') return '';
    return options.value.find((option) => option.value === target)?.label ?? target;
  });
}
