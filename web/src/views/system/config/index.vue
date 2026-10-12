<script setup lang="ts">
/**
 * 参数设置（`system:config:*`）。
 *
 * `config_key` 是 Redis 里的缓存键，所以这张页面有一个别处没有的动作：**手动刷新缓存**。
 * 有人绕过界面直接改库时，缓存里的旧值不会自己失效，这就是那个补丁开关（版本号失效
 * 版本号失效只覆盖「经接口修改」的路径）。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  createConfig,
  deleteConfig,
  exportConfigs,
  listConfigs,
  refreshConfigCache,
  updateConfig,
  type ConfigListQuery,
  type ConfigUpsert,
  type ConfigVo,
} from '@/api/system/config';
import AppDialog from '@/components/AppDialog.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { notifySuccess } from '@/utils/feedback';
import { formatDateTime } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirmDelete } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const formRef = ref<InstanceType<typeof AppForm>>();
const dialogVisible = ref(false);
const saving = ref(false);
const refreshing = ref(false);
const editingId = ref<number | null>(null);

const {
  loading,
  rows,
  total,
  page,
  pageSize,
  query,
  load,
  search,
  reset,
  changePage,
  changePageSize,
} = useTable<ConfigVo, ConfigListQuery>({ fetcher: listConfigs });

const form = reactive({
  config_name: '',
  config_key: '',
  config_value: '',
  config_type: 'N',
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  { prop: 'config_name', label: '参数名称', required: true },
  {
    prop: 'config_key',
    label: '参数键名',
    required: true,
    tips: '缓存键的一部分，改动等于新增一个参数，旧键的缓存需手动刷新。',
  },
  { prop: 'config_value', label: '参数值', required: true },
  {
    prop: 'config_type',
    label: '内置',
    type: 'select',
    options: [
      { value: 'Y', label: '是（Y）' },
      { value: 'N', label: '否（N）' },
    ],
    tips: '内置参数由框架读取，界面上不建议删除。',
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

function resetForm(): void {
  Object.assign(form, {
    config_name: '',
    config_key: '',
    config_value: '',
    config_type: 'N',
    remark: '',
  });
  formRef.value?.clearValidate();
}

function openCreate(): void {
  editingId.value = null;
  resetForm();
  dialogVisible.value = true;
}

function openEdit(row: ConfigVo): void {
  editingId.value = row.config_id;
  resetForm();
  Object.assign(form, {
    config_name: row.config_name,
    config_key: row.config_key,
    config_value: row.config_value,
    config_type: row.config_type,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const payload: ConfigUpsert = {
    config_name: form.config_name,
    config_key: form.config_key,
    config_value: form.config_value,
    config_type: form.config_type,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    if (editingId.value === null) await createConfig(payload);
    else await updateConfig(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    saving.value = false;
  }
}

async function remove(row: ConfigVo): Promise<void> {
  if (!(await confirmDelete(row.config_name))) return;
  try {
    await deleteConfig(row.config_id);
    notifySuccess();
    await load();
  } catch {
    // 内置参数（config_type = Y）会被后端拒绝，提示由拦截器给出。
  }
}

async function refresh(): Promise<void> {
  refreshing.value = true;
  try {
    await refreshConfigCache();
    notifySuccess('参数缓存已刷新');
  } catch {
    // 缓存后端不可用时后端返回 503，提示由拦截器给出。
  } finally {
    refreshing.value = false;
  }
}

async function exportAll(): Promise<void> {
  await exportConfigs({ ...query.value });
}
</script>

<template>
  <AppPage :title="title" :description="'运行期可调的参数；参数键名同时也是缓存键。'">
    <template #actions>
      <el-button v-perm="'system:config:remove'" :loading="refreshing" @click="refresh">
        {{ t('common.refresh') }}
      </el-button>
      <el-button v-perm="'system:config:export'" @click="exportAll">{{
        t('common.export')
      }}</el-button>
      <el-button v-perm="'system:config:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="参数名称">
          <el-input v-model="query.config_name" clearable placeholder="参数名称" />
        </el-form-item>
        <el-form-item label="参数键名">
          <el-input v-model="query.config_key" clearable placeholder="如 sys.index.skinName" />
        </el-form-item>
      </AppSearchForm>
    </template>

    <AppTable
      :rows="rows"
      :loading="loading"
      :total="total"
      :page="page"
      :page-size="pageSize"
      row-key="config_id"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="config_name" label="参数名称" min-width="180" show-overflow-tooltip />
      <el-table-column label="参数键名" min-width="220">
        <template #default="{ row }">
          <code class="config__key">{{ row.config_key }}</code>
        </template>
      </el-table-column>
      <el-table-column prop="config_value" label="参数值" min-width="200" show-overflow-tooltip />
      <el-table-column label="内置" width="90">
        <template #default="{ row }">
          <el-tag
            :type="row.config_type === 'Y' ? 'info' : 'primary'"
            effect="light"
            round
            size="small"
          >
            {{ row.config_type === 'Y' ? '内置' : '自定义' }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="150" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'system:config:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'system:config:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'system:config:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增参数' : '修改参数'"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" />
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.config__key {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}
</style>
