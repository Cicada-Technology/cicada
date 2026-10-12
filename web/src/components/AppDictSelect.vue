<script setup lang="ts">
/**
 * 字典下拉框：选项来自 `sys_dict_data`，值仍然是字符串（后端 `dict_value` 是 text）。
 *
 * 表单与查询条件都会用它，所以「选项未加载完成」这件事必须处理好：加载中禁用而不是
 * 让用户点开一个空列表。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import { useDictOptions } from '@/composables/useDict';

const props = withDefaults(
  defineProps<{
    dictType: string;
    modelValue?: string | number | null;
    placeholder?: string;
    clearable?: boolean;
    disabled?: boolean;
    multiple?: boolean;
    /** 渲染成单选按钮组而不是下拉框（选项少时更好点）。 */
    asRadio?: boolean;
    width?: string;
  }>(),
  {
    modelValue: '',
    placeholder: '',
    clearable: true,
    disabled: false,
    multiple: false,
    asRadio: false,
    width: '100%',
  },
);

const emit = defineEmits<{ 'update:modelValue': [value: string] }>();

const { t } = useI18n();
const options = useDictOptions(() => props.dictType);
const resolvedPlaceholder = computed(() => props.placeholder || t('common.pleaseChoose'));
</script>

<template>
  <el-radio-group
    v-if="asRadio"
    :model-value="modelValue ?? ''"
    :disabled="disabled"
    @update:model-value="emit('update:modelValue', String($event))"
  >
    <el-radio v-for="option in options" :key="option.value" :value="option.value">
      {{ option.label }}
    </el-radio>
  </el-radio-group>
  <el-select
    v-else
    :model-value="modelValue ?? ''"
    :placeholder="resolvedPlaceholder"
    :clearable="clearable"
    :disabled="disabled"
    :multiple="multiple"
    :style="{ width }"
    @update:model-value="emit('update:modelValue', String($event ?? ''))"
  >
    <el-option
      v-for="option in options"
      :key="option.value"
      :label="option.label"
      :value="option.value"
    />
  </el-select>
</template>
