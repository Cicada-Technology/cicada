<script setup lang="ts">
/**
 * 租户套餐（`system:package:*`， / ADR 0003）。
 *
 * 套餐是租户的**限额模板**：`-1` 表示不限（后端 `unlimited_i32` 的约定），所以数字输入框
 * 下限给到 `-1` 并在提示里写明语义。功能开关（`features`）后端是 `jsonb`，界面按 JSON
 * 文本编辑——比再造一个键值对控件更直白，也和数据库里存的东西一致。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  createPackage,
  deletePackage,
  listPackages,
  updatePackage,
  type PackageListQuery,
  type PackageUpsert,
  type PackageVo,
} from '@/api/system/platform';
import { STATUS_ENABLED, STATUS_OPTIONS } from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppStatusTag from '@/components/AppStatusTag.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { notifyError, notifySuccess } from '@/utils/feedback';
import { formatDateTime, formatNumber } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirmDelete } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const formRef = ref<InstanceType<typeof AppForm>>();
const dialogVisible = ref(false);
const saving = ref(false);
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
} = useTable<PackageVo, PackageListQuery>({ fetcher: listPackages });

const UNLIMITED = -1;

const form = reactive({
  code: '',
  name: '',
  user_limit: UNLIMITED,
  storage_limit_mb: UNLIMITED,
  api_qps_limit: UNLIMITED,
  /** `features` 的编辑态：JSON 文本，提交前解析成对象。 */
  features: '{}',
  status: STATUS_ENABLED,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  {
    prop: 'code',
    label: '套餐编码',
    required: true,
    tips: '如 free、pro，创建后建议不再修改。',
  },
  { prop: 'name', label: '套餐名称', required: true },
  {
    prop: 'user_limit',
    label: '用户数上限',
    type: 'number',
    tips: '-1 表示不限。',
    controlProps: { min: UNLIMITED, step: 1 },
  },
  {
    prop: 'storage_limit_mb',
    label: '存储上限(MB)',
    type: 'number',
    tips: '-1 表示不限。',
    controlProps: { min: UNLIMITED, step: 1 },
  },
  {
    prop: 'api_qps_limit',
    label: '接口 QPS',
    type: 'number',
    tips: '-1 表示不限。',
    controlProps: { min: UNLIMITED, step: 1 },
  },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  { prop: 'features', label: '功能开关', type: 'slot', span: 24 },
  { prop: 'remark', label: t('common.remark'), type: 'textarea', span: 24 },
]);

/** 限额列：-1 显示「不限」而不是负数。 */
function limitText(value: number, unit = ''): string {
  if (value < 0) return '不限';
  return `${formatNumber(value)}${unit}`;
}

function featuresText(features: Record<string, unknown>): string {
  try {
    return JSON.stringify(features ?? {}, null, 2);
  } catch {
    return '{}';
  }
}

function resetForm(): void {
  Object.assign(form, {
    code: '',
    name: '',
    user_limit: UNLIMITED,
    storage_limit_mb: UNLIMITED,
    api_qps_limit: UNLIMITED,
    features: '{}',
    status: STATUS_ENABLED,
    remark: '',
  });
  formRef.value?.clearValidate();
}

function openCreate(): void {
  editingId.value = null;
  resetForm();
  dialogVisible.value = true;
}

function openEdit(row: PackageVo): void {
  editingId.value = row.id;
  resetForm();
  Object.assign(form, {
    code: row.code,
    name: row.name,
    user_limit: row.user_limit,
    storage_limit_mb: row.storage_limit_mb,
    api_qps_limit: row.api_qps_limit,
    features: featuresText(row.features),
    status: row.status,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;

  let features: Record<string, unknown>;
  try {
    const parsed: unknown = form.features.trim() === '' ? {} : JSON.parse(form.features);
    if (Array.isArray(parsed) || typeof parsed !== 'object' || parsed === null) {
      notifyError('功能开关需是 JSON 对象，如 {"export": true}');
      return;
    }
    features = parsed as Record<string, unknown>;
  } catch {
    notifyError('功能开关不是合法 JSON');
    return;
  }

  const payload: PackageUpsert = {
    code: form.code,
    name: form.name,
    user_limit: form.user_limit,
    storage_limit_mb: form.storage_limit_mb,
    api_qps_limit: form.api_qps_limit,
    features,
    status: form.status,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    if (editingId.value === null) await createPackage(payload);
    else await updatePackage(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    saving.value = false;
  }
}

async function remove(row: PackageVo): Promise<void> {
  if (!(await confirmDelete(row.name))) return;
  try {
    await deletePackage(row.id);
    notifySuccess();
    await load();
  } catch {
    // 仍被租户绑定时后端返回 409，提示由拦截器给出。
  }
}
</script>

<template>
  <AppPage
    :title="title"
    :description="'套餐定义租户的用户数、存储与接口 QPS 上限；限额只在这里配置。'"
  >
    <template #actions>
      <el-button v-perm="'system:package:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="套餐编码">
          <el-input v-model="query.code" clearable placeholder="如 free" />
        </el-form-item>
        <el-form-item label="套餐名称">
          <el-input v-model="query.name" clearable placeholder="套餐名称" />
        </el-form-item>
      </AppSearchForm>
    </template>

    <AppTable
      :rows="rows"
      :loading="loading"
      :total="total"
      :page="page"
      :page-size="pageSize"
      row-key="id"
      empty-title="还没有套餐"
      empty-hint="套餐是租户的限额模板；没有套餐时也可以建租户，但没有配额。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="code" label="套餐编码" min-width="120" />
      <el-table-column prop="name" label="套餐名称" min-width="160" />
      <el-table-column label="用户数" width="110">
        <template #default="{ row }">{{ limitText(row.user_limit, ' 人') }}</template>
      </el-table-column>
      <el-table-column label="存储" width="120">
        <template #default="{ row }">
          {{ row.storage_limit_mb < 0 ? '不限' : `${formatNumber(row.storage_limit_mb)} MB` }}
        </template>
      </el-table-column>
      <el-table-column label="接口 QPS" width="120">
        <template #default="{ row }">{{ limitText(row.api_qps_limit, ' / 秒') }}</template>
      </el-table-column>
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
      </el-table-column>
      <el-table-column
        prop="remark"
        :label="t('common.remark')"
        min-width="160"
        show-overflow-tooltip
      />
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="150" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'system:package:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'system:package:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'system:package:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增套餐' : '修改套餐'"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" :columns="2">
        <template #field:features="{ model, field }">
          <el-input
            v-model="(model as Record<string, unknown>)[field.prop] as string"
            type="textarea"
            :rows="5"
            spellcheck="false"
            placeholder='{"audit": true, "export": true}'
          />
          <p class="package__tips">JSON 对象，按套餐开关功能；留空视为无开关。</p>
        </template>
      </AppForm>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.package__tips {
  width: 100%;
  margin-top: var(--ci-space-1);
  font-size: var(--ci-text-xs);
  line-height: var(--ci-leading-snug);
  color: var(--ci-color-text-tertiary);
}
</style>
