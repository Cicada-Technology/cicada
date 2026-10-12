<script setup lang="ts">
/**
 * 角色管理（`system:role:*`）。
 *
 * 角色是**权限与数据范围的载体**，所以这张页面有两件事：
 * 基本属性（标识、排序、状态）与授权（菜单集合、数据范围、自定义部门）。
 *
 * 授权用独立的弹层而不是塞进主表单：菜单树很高，和基本属性挤在一起会把「保存」按钮
 * 推到屏幕外；而且两者失败原因不同（标识重复 vs 授权非法），分开更容易定位。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import { listDepts, type DeptVo } from '@/api/system/dept';
import { listMenus, type MenuVo } from '@/api/system/menu';
import {
  createRole,
  deleteRole,
  exportRoles,
  getRoleMenus,
  listRoles,
  updateRole,
  type RoleListQuery,
  type RoleUpsert,
  type RoleVo,
} from '@/api/system/role';
import {
  DATA_SCOPE_ALL,
  DATA_SCOPE_CUSTOM,
  DATA_SCOPE_OPTIONS,
  STATUS_ENABLED,
  STATUS_OPTIONS,
} from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppStatusTag from '@/components/AppStatusTag.vue';
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
const loadingOptions = ref(false);

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
} = useTable<RoleVo, RoleListQuery>({ fetcher: listRoles });

const statusOptions = STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label }));

const dataScopeLabels = computed(
  () => new Map(DATA_SCOPE_OPTIONS.map((item) => [item.value as number, item.label])),
);

/** 菜单树是授权弹层的数据源，也是「上级菜单」的候选（含 `F` 按钮节点）。 */
const menus = ref<MenuVo[]>([]);
const depts = ref<DeptVo[]>([]);

const deptTreeNodes = computed(() => {
  const toNode = (node: DeptVo): Record<string, unknown> => ({
    value: node.dept_id,
    label: node.dept_name,
    children: (node.children ?? []).map(toNode),
  });
  return depts.value.map(toNode);
});

// ---------------------------------------------------------------- 基本属性
const dialogVisible = ref(false);
const saving = ref(false);
const editingId = ref<number | null>(null);

const form = reactive({
  role_name: '',
  role_key: '',
  role_sort: 0,
  status: STATUS_ENABLED,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  { prop: 'role_name', label: '角色名称', required: true },
  { prop: 'role_key', label: '角色标识', required: true, tips: '用于鉴权判定，如 finance。' },
  { prop: 'role_sort', label: t('common.sort'), type: 'number' },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: statusOptions,
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

function openCreate(): void {
  editingId.value = null;
  Object.assign(form, {
    role_name: '',
    role_key: '',
    role_sort: 0,
    status: STATUS_ENABLED,
    remark: '',
  });
  formRef.value?.clearValidate();
  dialogVisible.value = true;
}

function openEdit(row: RoleVo): void {
  editingId.value = row.role_id;
  Object.assign(form, {
    role_name: row.role_name,
    role_key: row.role_key,
    role_sort: row.role_sort,
    status: row.status,
    remark: row.remark ?? '',
  });
  formRef.value?.clearValidate();
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  saving.value = true;
  const payload: RoleUpsert = {
    role_name: form.role_name,
    role_key: form.role_key,
    role_sort: form.role_sort,
    status: form.status,
    remark: form.remark || null,
  };
  try {
    if (editingId.value === null) await createRole(payload);
    else await updateRole(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 失败保留弹层与输入（提示由拦截器给出）。
  } finally {
    saving.value = false;
  }
}

async function remove(row: RoleVo): Promise<void> {
  if (!(await confirmDelete(row.role_name))) return;
  try {
    await deleteRole(row.role_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

// ---------------------------------------------------------------- 授权
const grant = reactive({
  visible: false,
  roleId: 0,
  roleName: '',
  dataScope: DATA_SCOPE_ALL,
  checkedMenus: [] as number[],
  checkedDepts: [] as number[],
});
const grantSaving = ref(false);
const treeRef = ref<{
  setCheckedKeys: (keys: number[]) => void;
  getCheckedKeys: () => unknown[];
}>();

const showDeptPicker = computed(() => grant.dataScope === DATA_SCOPE_CUSTOM);

async function openGrant(row: RoleVo): Promise<void> {
  grant.roleId = row.role_id;
  grant.roleName = row.role_name;
  grant.dataScope = row.data_scope;
  grant.visible = true;
  loadingOptions.value = true;
  try {
    // 先拉菜单树与部门树，再拉已有授权；后者失败会保持空勾选而不是清空后端数据。
    const [menuTree, deptTree, checked] = await Promise.all([
      listMenus(),
      listDepts(),
      getRoleMenus(row.role_id),
    ]);
    menus.value = menuTree;
    depts.value = deptTree;
    grant.checkedMenus = checked;
    treeRef.value?.setCheckedKeys(checked);
  } catch {
    // 提示由拦截器给出。
  } finally {
    loadingOptions.value = false;
  }
}

async function submitGrant(): Promise<void> {
  const checked = (treeRef.value?.getCheckedKeys() ?? []) as number[];
  // 半选的父节点也要提交：后端按菜单 id 落库，父目录缺了会让子菜单在侧栏失去挂载点。
  const halfChecked = ((
    treeRef.value as unknown as { getHalfCheckedKeys?: () => number[] }
  )?.getHalfCheckedKeys?.() ?? []) as number[];
  grantSaving.value = true;
  const row = rows.value.find((item) => item.role_id === grant.roleId);
  try {
    await updateRole(grant.roleId, {
      role_name: row?.role_name ?? grant.roleName,
      role_key: row?.role_key ?? '',
      role_sort: row?.role_sort ?? 0,
      status: row?.status ?? STATUS_ENABLED,
      remark: row?.remark ?? null,
      data_scope: grant.dataScope,
      menu_ids: [...checked, ...halfChecked],
      dept_ids: showDeptPicker.value ? grant.checkedDepts : undefined,
    });
    grant.visible = false;
    notifySuccess('授权已保存');
    await load();
  } catch {
    // 同上。
  } finally {
    grantSaving.value = false;
  }
}

async function exportAll(): Promise<void> {
  await exportRoles({ ...query.value });
}

void (async () => {
  try {
    menus.value = await listMenus();
  } catch {
    // 授权弹层打开时会再拉一次，这里失败不阻塞页面。
  }
})();
</script>

<template>
  <AppPage
    :title="title"
    :description="'角色是权限点与数据范围的载体，用户通过角色获得两者的并集。'"
  >
    <template #actions>
      <el-button v-perm="'system:role:export'" @click="exportAll">{{
        t('common.export')
      }}</el-button>
      <el-button v-perm="'system:role:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="角色名称">
          <el-input v-model="query.role_name" clearable placeholder="角色名称" />
        </el-form-item>
        <el-form-item label="角色标识">
          <el-input v-model="query.role_key" clearable placeholder="如 finance" />
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
      row-key="role_id"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="role_name" label="角色名称" min-width="150" />
      <el-table-column prop="role_key" label="角色标识" min-width="150" />
      <el-table-column :label="t('common.sort')" width="80" prop="role_sort" />
      <el-table-column label="数据范围" min-width="130">
        <template #default="{ row }">
          {{ dataScopeLabels.get(row.data_scope) ?? row.data_scope }}
        </template>
      </el-table-column>
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
      </el-table-column>
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="210" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'system:role:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'system:role:edit'" link type="warning" @click="openGrant(row)">
            权限分配
          </el-button>
          <el-button v-perm="'system:role:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'system:role:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增角色' : '修改角色'"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" />
    </AppDialog>

    <AppDialog
      v-model="grant.visible"
      :title="`权限分配：${grant.roleName}`"
      width="620px"
      :confirm-loading="grantSaving"
      @confirm="submitGrant"
    >
      <div v-loading="loadingOptions" class="grant">
        <el-form label-width="88px" @submit.prevent>
          <el-form-item label="数据范围">
            <el-select v-model="grant.dataScope" style="width: 100%">
              <el-option
                v-for="item in DATA_SCOPE_OPTIONS"
                :key="item.value"
                :label="item.label"
                :value="item.value"
              />
            </el-select>
          </el-form-item>
          <el-form-item v-if="showDeptPicker" label="自定义部门">
            <el-tree-select
              v-model="grant.checkedDepts"
              :data="deptTreeNodes"
              :props="{ label: 'label', value: 'value', children: 'children' }"
              multiple
              show-checkbox
              check-strictly
              check-on-click-node
              clearable
              placeholder="选择可访问的部门"
              style="width: 100%"
            />
          </el-form-item>
        </el-form>

        <p class="grant__hint">勾选该角色可见的菜单与按钮权限；父目录会随子项一并提交。</p>
        <el-scrollbar height="320px" class="grant__tree">
          <el-tree
            ref="treeRef"
            :data="menus"
            :props="{ label: 'menu_name', children: 'children' }"
            node-key="menu_id"
            show-checkbox
            default-expand-all
            :check-strictly="false"
          >
            <template #default="{ data }">
              <span class="grant__node">
                {{ data.menu_name }}
                <code v-if="data.perms" class="grant__perm">{{ data.perms }}</code>
              </span>
            </template>
          </el-tree>
        </el-scrollbar>
      </div>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.grant {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
}

.grant__hint {
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.grant__tree {
  border: 1px solid var(--ci-color-separator);
  border-radius: var(--ci-radius-md);
  padding: var(--ci-space-2);
}

.grant__node {
  display: inline-flex;
  align-items: center;
  gap: var(--ci-space-2);
}

.grant__perm {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-2xs);
  color: var(--ci-color-text-tertiary);
}
</style>
