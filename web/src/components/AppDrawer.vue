<script setup lang="ts">
/**
 * 抽屉：用于「不改主流程的次要信息」（详情、日志、配置片段）。
 *
 * 与弹层的分工：**必须做完才能继续**的用弹层（表单提交），**可以顺带看**的用抽屉。
 * 抽屉用动量曲线进出（`transitions.css` 里的 `--ci-ease-momentum`），这是唯一允许
 * 轻微过冲的场景。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

const props = withDefaults(
  defineProps<{
    title?: string;
    size?: string | number;
    direction?: 'ltr' | 'rtl' | 'ttb' | 'btt';
    confirmLoading?: boolean;
    confirmText?: string;
    hideFooter?: boolean;
  }>(),
  {
    title: '',
    size: '480px',
    direction: 'rtl',
    confirmLoading: false,
    confirmText: '',
    hideFooter: true,
  },
);

const emit = defineEmits<{ confirm: []; closed: [] }>();

const { t } = useI18n();

const visible = defineModel<boolean>({ default: false });

const resolvedConfirmText = computed(() => props.confirmText || t('common.confirm'));
</script>

<template>
  <el-drawer
    v-model="visible"
    :title="title"
    :size="size"
    :direction="direction"
    :close-on-click-modal="true"
    destroy-on-close
    append-to-body
    @closed="emit('closed')"
  >
    <slot />

    <template v-if="!hideFooter" #footer>
      <slot name="footer">
        <el-button :disabled="confirmLoading" @click="visible = false">
          {{ t('common.cancel') }}
        </el-button>
        <el-button type="primary" :loading="confirmLoading" @click="emit('confirm')">
          {{ resolvedConfirmText }}
        </el-button>
      </slot>
    </template>
  </el-drawer>
</template>
