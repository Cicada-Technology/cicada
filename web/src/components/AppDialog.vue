<script setup lang="ts">
/**
 * 弹层（表格/表单/弹层/抽屉等高频组件封装成项目自有组件）。
 *
 * 三条默认行为是有意为之，不是偷懒：
 * - 点击遮罩**不**关闭：表单弹层误触关闭会丢输入。
 * - `destroy-on-close`：保证每次打开都是干净的组件实例（校验状态、上传列表不会残留）。
 * - 居中（`align-center`）：Apple 风格的浮层居中，而不是贴顶。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

const props = withDefaults(
  defineProps<{
    /** 标题；空字符串表示不显示标题区。 */
    title?: string;
    width?: string | number;
    /** 底部主按钮的加载态。 */
    confirmLoading?: boolean;
    confirmText?: string;
    cancelText?: string;
    /** 主按钮语义：表单提交用 primary，破坏性操作用 danger。 */
    confirmType?: 'primary' | 'danger' | 'success' | 'warning';
    /** 只读详情用它隐藏底部按钮。 */
    hideFooter?: boolean;
    fullscreen?: boolean;
  }>(),
  {
    title: '',
    width: '640px',
    confirmLoading: false,
    confirmText: '',
    cancelText: '',
    confirmType: 'primary',
    hideFooter: false,
    fullscreen: false,
  },
);

const emit = defineEmits<{ confirm: []; closed: [] }>();

const { t } = useI18n();

const visible = defineModel<boolean>({ default: false });

const resolvedWidth = computed(() => props.width);
const resolvedConfirmText = computed(() => props.confirmText || t('common.confirm'));
const resolvedCancelText = computed(() => props.cancelText || t('common.cancel'));
</script>

<template>
  <el-dialog
    v-model="visible"
    :title="title"
    :width="resolvedWidth"
    :fullscreen="fullscreen"
    :close-on-click-modal="false"
    destroy-on-close
    align-center
    append-to-body
    @closed="emit('closed')"
  >
    <slot />

    <template v-if="!hideFooter" #footer>
      <slot name="footer">
        <el-button :disabled="confirmLoading" @click="visible = false">
          {{ resolvedCancelText }}
        </el-button>
        <el-button :type="confirmType" :loading="confirmLoading" @click="emit('confirm')">
          {{ resolvedConfirmText }}
        </el-button>
      </slot>
    </template>
  </el-dialog>
</template>
