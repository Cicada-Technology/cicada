<script setup lang="ts">
/**
 * 租户管理（`system:tenant:*`， / ADR 0003）。
 *
 * 只有平台级账号能进（后端 `require_platform` 会把越权请求判 403），所以这里不再做前端
 * 角色判断；限额来自**套餐**，租户自己不持有额度，界面因此把用量做成只读抽屉而不是表单。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  listPackages,
  type PackageVo,
  type QuotaVo,
  type TenantListQuery,
  type TenantUpsert,
  type TenantVo,
  createTenant,
  deleteTenant,
  getTenantQuota,
  listTenants,
  updateTenant,
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
import { notifySuccess } from '@/utils/feedback';
import { formatBytes, formatDateTime, formatNumber, orPlaceholder } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirmDelete } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const formRef = ref<InstanceType<typeof AppForm>>();
const dialogVisible = ref(false);
const saving = ref(false);
const editingId = ref<number | null>(null);

/** 套餐下拉：限额定义在套餐上，新建/修改租户时只能选，不能改。 */
const packages = ref<PackageVo[]>([]);

const quotaVisible = ref(false);
const quotaLoading = ref(false);
const quota = ref<QuotaVo | null>(null);
const quotaTenant = ref<TenantVo | null>(null);

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
} = useTable<TenantVo, TenantListQuery>({ fetcher: listTenants });

const form = reactive({
  tenant_name: '',
  status: STATUS_ENABLED,
  package_id: null as number | null,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  { prop: 'tenant_name', label: '租户名称', required: true },
  {
    prop: 'package_id',
    label: '套餐',
    type: 'select',
    clearable: true,
    tips: '不选套餐时，租户只有平台默认限额，用户数与存储都可能受限。',
    options: packages.value.map((item) => ({
      value: item.id,
      label: `${item.name}（${item.code}）`,
    })),
  },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

async function loadPackages(): Promise<void> {
  try {
    const data = await listPackages({ page: 1, page_size: 200 });
    packages.value = data.items;
  } catch {
    // 提示由拦截器给出；套餐拉不到时下拉为空，表单仍可保存（后端允许 package_id 为空）。
  }
}

function resetForm(): void {
  Object.assign(form, { tenant_name: '', status: STATUS_ENABLED, package_id: null, remark: '' });
  formRef.value?.clearValidate();
}

function openCreate(): void {
  editingId.value = null;
  resetForm();
  dialogVisible.value = true;
  void loadPackages();
}

function openEdit(row: TenantVo): void {
  editingId.value = row.tenant_id;
  resetForm();
  Object.assign(form, {
    tenant_name: row.tenant_name,
    status: row.status,
    package_id: row.package_id ?? null,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
  void loadPackages();
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const payload: TenantUpsert = {
    tenant_name: form.tenant_name,
    status: form.status,
    package_id: form.package_id,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    if (editingId.value === null) await createTenant(payload);
    else await updateTenant(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    saving.value = false;
  }
}

async function remove(row: TenantVo): Promise<void> {
  if (!(await confirmDelete(row.tenant_name))) return;
  try {
    await deleteTenant(row.tenant_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

async function openQuota(row: TenantVo): Promise<void> {
  quotaTenant.value = row;
  quota.value = null;
  quotaVisible.value = true;
  quotaLoading.value = true;
  try {
    quota.value = await getTenantQuota(row.tenant_id);
  } catch {
    // 同上：弹层保持打开并显示空态。
  } finally {
    quotaLoading.value = false;
  }
}

/** -1 是「不限」的约定值，不能显示成负数。 */
function limit(value: number, unit = ''): string {
  if (value < 0) return '不限';
  return `${formatNumber(value)}${unit}`;
}

function quotaRows(): Array<{ label: string; value: string; percent?: number }> {
  const data = quota.value;
  if (!data) return [];
  const usage = (used: number, cap: number) => (cap > 0 ? Math.min(100, (used / cap) * 100) : 0);
  return [
    {
      label: '用户数',
      value: `${formatNumber(data.users)} / ${limit(data.user_limit, ' 人')}`,
      percent: usage(data.users, data.user_limit),
    },
    {
      label: '存储',
      value: `${formatBytes(data.storage_mb * 1024 * 1024)} / ${
        data.storage_limit_mb < 0 ? '不限' : formatBytes(data.storage_limit_mb * 1024 * 1024)
      }`,
      percent: usage(data.storage_mb, data.storage_limit_mb),
    },
    {
      label: '接口 QPS 上限',
      value: limit(data.api_qps_limit, ' / 秒'),
    },
  ];
}

const featureList = computed(() => {
  const features = quota.value?.features;
  if (!features || typeof features !== 'object') return [];
  return Object.entries(features as Record<string, unknown>).map(
    ([key, value]) => `${key}：${String(value)}`,
  );
});
</script>

<template>
  <AppPage
    :title="title"
    :description="'租户是多租户数据的隔离边界；限额来自套餐，租户自身不持有额度。'"
  >
    <template #actions>
      <el-button v-perm="'system:tenant:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="租户名称">
          <el-input v-model="query.tenant_name" clearable placeholder="租户名称" />
        </el-form-item>
        <el-form-item :label="t('common.status')">
          <el-select v-model="query.status" clearable placeholder="全部" style="width: 120px">
            <el-option
              v-for="item in STATUS_OPTIONS"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </el-select>
        </el-form-item>
      </AppSearchForm>
    </template>

    <AppTable
      :rows="rows"
      :loading="loading"
      :total="total"
      :page="page"
      :page-size="pageSize"
      row-key="tenant_id"
      empty-title="还没有租户"
      empty-hint="默认租户由迁移创建；这里新增的是其它业务租户。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="tenant_id" label="租户 ID" width="100" />
      <el-table-column prop="tenant_name" label="租户名称" min-width="180" show-overflow-tooltip />
      <el-table-column label="套餐" min-width="160">
        <template #default="{ row }">{{ orPlaceholder(row.package_name) }}</template>
      </el-table-column>
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
      </el-table-column>
      <el-table-column
        prop="remark"
        :label="t('common.remark')"
        min-width="180"
        show-overflow-tooltip
      />
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="210" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'system:tenant:list'" link type="primary" @click="openQuota(row)">
            用量
          </el-button>
          <el-button v-perm="'system:tenant:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'system:tenant:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'system:tenant:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增租户' : '修改租户'"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" />
    </AppDialog>

    <AppDialog v-model="quotaVisible" title="用量与限额" :hide-footer="true" width="560px">
      <div v-loading="quotaLoading" class="tenant__quota">
        <p v-if="quotaTenant" class="tenant__quota-head">
          {{ quotaTenant.tenant_name }}
          <span class="tenant__hint">套餐 {{ orPlaceholder(quota?.package_code) }}</span>
        </p>
        <div v-for="row in quotaRows()" :key="row.label" class="tenant__quota-row">
          <span class="tenant__quota-label">{{ row.label }}</span>
          <span class="tenant__quota-value">{{ row.value }}</span>
          <el-progress
            v-if="row.percent !== undefined"
            class="tenant__quota-bar"
            :percentage="Number(row.percent.toFixed(1))"
            :stroke-width="8"
            :show-text="false"
          />
        </div>
        <template v-if="featureList.length > 0">
          <h3 class="tenant__quota-sub">套餐功能</h3>
          <ul class="tenant__features">
            <li v-for="item in featureList" :key="item">{{ item }}</li>
          </ul>
        </template>
      </div>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.tenant__quota {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  min-height: 120px;
}

.tenant__quota-head {
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-semibold);
  color: var(--ci-color-text);
}

.tenant__hint {
  margin-left: var(--ci-space-2);
  font-size: var(--ci-text-xs);
  font-weight: var(--ci-weight-regular);
  color: var(--ci-color-text-tertiary);
}

.tenant__quota-row {
  display: flex;
  align-items: center;
  gap: var(--ci-space-3);
  font-size: var(--ci-text-sm);
}

.tenant__quota-label {
  width: 104px;
  color: var(--ci-color-text-secondary);
}

.tenant__quota-value {
  color: var(--ci-color-text);
}

.tenant__quota-bar {
  flex: 1 1 auto;
  min-width: 0;
}

.tenant__quota-sub {
  font-size: var(--ci-text-sm);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-text-secondary);
}

.tenant__features {
  display: flex;
  flex-wrap: wrap;
  gap: var(--ci-space-2);
  margin: 0;
  padding: 0;
  list-style: none;
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}

.tenant__features li {
  padding: var(--ci-space-1) var(--ci-space-2);
  background: var(--ci-color-surface-sunken);
  border-radius: var(--ci-radius-xs);
}
</style>
