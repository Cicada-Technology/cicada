<script setup lang="ts">
/**
 * 状态标签（后端 `status: i16`，`0` 正常 / `1` 停用）。
 *
 * 只处理这一处最容易写错的映射：参考实现用 `0` 表示「正常」、`1` 表示「停用」，
 * 而直觉恰好相反，散在各页面里迟早会有人写反。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import { STATUS_ENABLED } from '@/api/types';

const { t } = useI18n();

const props = withDefaults(
  defineProps<{
    value?: number | null;
    /** 自定义文案（如日志里的「成功/失败」）。 */
    enabledText?: string;
    disabledText?: string;
    /** 日志类状态位用「成功/异常」而不是「正常/停用」。 */
    variant?: 'default' | 'outcome';
  }>(),
  { value: null, enabledText: '', disabledText: '', variant: 'default' },
);

const enabled = computed(() => props.value === STATUS_ENABLED);

const label = computed(() => {
  if (props.variant === 'outcome') {
    return enabled.value ? props.enabledText || '成功' : props.disabledText || '失败';
  }
  return enabled.value
    ? props.enabledText || t('common.enabled')
    : props.disabledText || t('common.disabled');
});

const type = computed(() => {
  if (props.variant === 'outcome') return enabled.value ? 'success' : 'danger';
  return enabled.value ? 'success' : 'info';
});
</script>

<template>
  <el-tag :type="type" size="small" effect="light" round>{{ label }}</el-tag>
</template>
