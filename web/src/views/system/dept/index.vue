<script setup lang="ts">
/**
 * 部门管理（`system:dept:*`）。
 *
 * 部门不只是通讯录：它是**数据范围的判定依据**（「本部门及以下」靠
 * `ancestors` 链路求值），所以删除与换父节点都要求后端做校验，前端只负责把层级
 * 展示清楚、把错误提示出来。
 *
 * 树表 + 无分页：部门数量有限，分页反而让人无法核对上下级。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  createDept,
  deleteDept,
  listDepts,
  updateDept,
  type DeptUpsert,
  type DeptVo,
} from '@/api/system/dept';
import { STATUS_ENABLED, STATUS_OPTIONS } from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
import AppPage from '@/components/AppPage.vue';
import AppStatusTag from '@/components/AppStatusTag.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { notifySuccess } from '@/utils/feedback';

const { t } = useI18n();
const route = useRoute();
const { confirmDelete } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const loading = ref(false);
const rows = ref<DeptVo[]>([]);
const expandAll = ref(true);
const formRef = ref<InstanceType<typeof AppForm>>();
const dialogVisible = ref(false);
const saving = ref(false);
const editingId = ref<number | null>(null);

const parentNodes = computed(() => {
  const toNode = (node: DeptVo): Record<string, unknown> => ({
    value: node.dept_id,
    label: node.dept_name,
    children: (node.children ?? []).map(toNode),
  });
  return [{ value: 0, label: '顶级（根节点）' }, ...rows.value.map(toNode)];
});

const form = reactive({
  parent_id: 0,
  dept_name: '',
  order_num: 0,
  leader: '',
  phone: '',
  email: '',
  status: STATUS_ENABLED,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  {
    prop: 'parent_id',
    label: '上级部门',
    type: 'tree-select',
    treeData: parentNodes.value,
    treeLabelKey: 'label',
    treeValueKey: 'value',
    required: true,
    tips: '改父节点会重建祖先链路，影响「本部门及以下」的数据范围。',
  },
  { prop: 'dept_name', label: '部门名称', required: true },
  { prop: 'order_num', label: t('common.sort'), type: 'number' },
  { prop: 'leader', label: '负责人' },
  { prop: 'phone', label: '联系电话' },
  { prop: 'email', label: '邮箱' },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

async function load(): Promise<void> {
  loading.value = true;
  try {
    rows.value = await listDepts();
  } finally {
    loading.value = false;
  }
}

function resetForm(): void {
  Object.assign(form, {
    parent_id: 0,
    dept_name: '',
    order_num: 0,
    leader: '',
    phone: '',
    email: '',
    status: STATUS_ENABLED,
    remark: '',
  });
  formRef.value?.clearValidate();
}

function openCreate(parent?: DeptVo): void {
  editingId.value = null;
  resetForm();
  if (parent) form.parent_id = parent.dept_id;
  dialogVisible.value = true;
}

function openEdit(row: DeptVo): void {
  editingId.value = row.dept_id;
  resetForm();
  Object.assign(form, {
    parent_id: row.parent_id,
    dept_name: row.dept_name,
    order_num: row.order_num,
    leader: row.leader ?? '',
    phone: row.phone ?? '',
    email: row.email ?? '',
    status: row.status,
  });
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const payload: DeptUpsert = {
    parent_id: form.parent_id,
    dept_name: form.dept_name,
    order_num: form.order_num,
    leader: form.leader || null,
    phone: form.phone || null,
    email: form.email || null,
    status: form.status,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    if (editingId.value === null) await createDept(payload);
    else await updateDept(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出（父节点非法、有子部门等都会在这里被挡住）。
  } finally {
    saving.value = false;
  }
}

async function remove(row: DeptVo): Promise<void> {
  if (!(await confirmDelete(row.dept_name))) return;
  try {
    await deleteDept(row.dept_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

void load();
</script>

<template>
  <AppPage :title="title" :description="'部门决定数据范围（本部门 / 本部门及以下）的判定边界。'">
    <template #actions>
      <el-button @click="expandAll = !expandAll">
        {{ expandAll ? t('table.collapseAll') : t('table.expandAll') }}
      </el-button>
      <el-button v-perm="'system:dept:add'" type="primary" @click="openCreate()">
        {{ t('common.add') }}
      </el-button>
    </template>

    <AppTable
      :rows="rows"
      :loading="loading"
      :total="null"
      tree
      row-key="dept_id"
      :expand-all="expandAll"
      empty-hint="部门树为空时，用户无法归属部门，数据范围会退化为「仅本人」。"
    >
      <el-table-column prop="dept_name" label="部门名称" min-width="220" />
      <el-table-column prop="order_num" :label="t('common.sort')" width="80" />
      <el-table-column prop="leader" label="负责人" min-width="120" />
      <el-table-column prop="phone" label="联系电话" min-width="140" />
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="200" fixed="right">
        <template #default="{ row }">
          <el-button
            v-perm="'system:dept:add'"
            link
            type="primary"
            @click="openCreate(row as DeptVo)"
          >
            新增下级
          </el-button>
          <el-button v-perm="'system:dept:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'system:dept:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增部门' : '修改部门'"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" :columns="2" />
    </AppDialog>
  </AppPage>
</template>
