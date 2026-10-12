<script setup lang="ts">
/**
 * 查询条：一键给出「查询 / 重置」，并统一回车提交。
 *
 * 代码生成器产出的页面用的是裸 `el-form inline`（那种风格由 `.app-container` 兜住），
 * 手写页面用这个组件，两者视觉一致但后者省掉重复的按钮与回车处理。
 */
import { useI18n } from 'vue-i18n';

defineProps<{
  /**
   * 查询条件对象；仅用于 `el-form` 的 model 绑定与重置时机提示。
   *
   * 类型取 `object` 而不是 `Record<string, unknown>`：后端的查询 DTO 声明成 interface
   * （与 Rust 结构体逐字对应），而 interface 没有隐式索引签名，写成 `Record` 会让每个
   * 调用点都报「缺少索引签名」——这里只把 model 透传给 el-form，不按 key 取值。
   */
  model: object;
  loading?: boolean;
  /** 隐藏重置按钮（例如筛选条件由外层管理）。 */
  hideReset?: boolean;
}>();

const emit = defineEmits<{ search: []; reset: [] }>();

const { t } = useI18n();
</script>

<template>
  <el-form
    class="app-search"
    :model="model as Record<string, unknown>"
    inline
    @submit.prevent="emit('search')"
    @keyup.enter="emit('search')"
  >
    <slot />
    <el-form-item class="app-search__actions">
      <el-button type="primary" :loading="loading" native-type="submit" @click="emit('search')">
        {{ t('common.search') }}
      </el-button>
      <el-button v-if="!hideReset" :disabled="loading" @click="emit('reset')">
        {{ t('common.reset') }}
      </el-button>
      <slot name="actions" />
    </el-form-item>
  </el-form>
</template>

<style scoped>
.app-search {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--ci-space-2) var(--ci-space-3);
}

.app-search :deep(.el-form-item) {
  margin: 0;
}

.app-search__actions {
  margin-left: auto;
}
</style>
