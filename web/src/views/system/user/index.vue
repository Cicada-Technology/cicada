<script setup lang="ts">
/**
 * 用户管理（`system:user:*`）。
 *
 * 这是骨架里最完整的一张 CRUD 页，其他模块（岗位/参数/字典…）按同一套结构展开：
 * `useTable` 管列表状态 → `AppTable` 管呈现 → `AppDialog + AppForm` 管表单。
 *
 * 三个刻意的取舍：
 * - 行内操作按权限点分开显隐：「重置密码」与「修改」是两件事，只被授予前者的角色
 *   不该看见后者（界面侧裁剪，判定仍在后端）。
 * - 新增与修改共用一个弹层，字段差异用 `when` 表达：后端 `UserUpdate` 里确实没有
 *   `user_name`（改名会破坏审计链路），所以账号只在新增时出现。
 * - 写操作的失败提示交给请求拦截器，这里只在失败时**保持弹层打开**（失败往往是
 *   账号重复 409 或字段非法 422，用户要能原地改）。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import { listDepts, type DeptVo } from '@/api/system/dept';
import { listRoles, type RoleVo } from '@/api/system/role';
import {
  createUser,
  deleteUser,
  exportUsers,
  importUsers,
  listUsers,
  resetUserPassword,
  updateUser,
  type UserListQuery,
  type UserVo,
} from '@/api/system/user';
import { STATUS_ENABLED, STATUS_OPTIONS } from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppStatusTag from '@/components/AppStatusTag.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { pickFile } from '@/utils/download';
import { notifySuccess, notifyWarning } from '@/utils/feedback';
import { formatDateTime } from '@/utils/format';

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
} = useTable<UserVo, UserListQuery>({ fetcher: listUsers });

const depts = ref<DeptVo[]>([]);
const roles = ref<RoleVo[]>([]);
const roleNames = computed(
  () => new Map(roles.value.map((role) => [role.role_id, role.role_name])),
);

const deptNames = computed(() => {
  const map = new Map<number, string>();
  const walk = (nodes: DeptVo[]): void => {
    for (const node of nodes) {
      map.set(node.dept_id, node.dept_name);
      walk(node.children ?? []);
    }
  };
  walk(depts.value);
  return map;
});

/** 部门树转成选择器要的 `{ label, value }` 形状（AppTreeSelect 只认键名，不认类型）。 */
const deptTreeNodes = computed(() => {
  const toNode = (node: DeptVo): Record<string, unknown> => ({
    value: node.dept_id,
    label: node.dept_name,
    children: (node.children ?? []).map(toNode),
  });
  return depts.value.map(toNode);
});

/** 表单模型：`user_name` / `password` 仅在新增时提交（后端 DTO 决定，见文件头注释）。 */
const form = reactive({
  user_name: '',
  nick_name: '',
  password: '',
  dept_id: null as number | null,
  email: '',
  phonenumber: '',
  status: STATUS_ENABLED,
  remark: '',
  role_ids: [] as number[],
});

const isCreate = computed(() => editingId.value === null);

const fields = computed<AppFormField[]>(() => [
  {
    prop: 'user_name',
    label: '账号',
    required: true,
    when: () => isCreate.value,
    tips: '登录名，创建后不可修改。',
  },
  { prop: 'nick_name', label: '昵称', required: true },
  {
    prop: 'password',
    label: '密码',
    type: 'password',
    required: true,
    when: () => isCreate.value,
    tips: '至少 8 位，由后端校验强度。',
  },
  {
    prop: 'dept_id',
    label: '部门',
    type: 'tree-select',
    treeData: deptTreeNodes.value,
    treeLabelKey: 'label',
    treeValueKey: 'value',
  },
  { prop: 'email', label: '邮箱' },
  { prop: 'phonenumber', label: '手机号' },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  {
    prop: 'role_ids',
    label: '角色',
    type: 'select',
    multiple: true,
    options: roles.value.map((role) => ({ value: role.role_id, label: role.role_name })),
    tips: '角色决定权限集合，保存后后端会失效该租户的鉴权缓存。',
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

/** 重置密码用的是独立小弹层：`window.prompt` 与整体观感冲突，也挡不住弱口令。 */
const pwdDialog = reactive({ visible: false, userId: 0, userName: '', password: '' });
const pwdSaving = ref(false);

async function loadOptions(): Promise<void> {
  const [deptTree, rolePage] = await Promise.all([
    listDepts(),
    listRoles({ page: 1, page_size: 200 }),
  ]);
  depts.value = deptTree;
  roles.value = rolePage.items;
}

function resetForm(): void {
  Object.assign(form, {
    user_name: '',
    nick_name: '',
    password: '',
    dept_id: null,
    email: '',
    phonenumber: '',
    status: STATUS_ENABLED,
    remark: '',
    role_ids: [],
  });
  formRef.value?.clearValidate();
}

function openCreate(): void {
  editingId.value = null;
  resetForm();
  dialogVisible.value = true;
}

function openEdit(row: UserVo): void {
  editingId.value = row.user_id;
  resetForm();
  Object.assign(form, {
    nick_name: row.nick_name,
    dept_id: row.dept_id ?? null,
    email: row.email ?? '',
    phonenumber: row.phonenumber ?? '',
    status: row.status,
    role_ids: [...(row.role_ids ?? [])],
  });
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  saving.value = true;
  try {
    if (editingId.value === null) {
      await createUser({
        user_name: form.user_name,
        nick_name: form.nick_name,
        password: form.password,
        dept_id: form.dept_id,
        email: form.email || null,
        phonenumber: form.phonenumber || null,
        status: form.status,
        remark: form.remark || null,
        role_ids: form.role_ids,
      });
    } else {
      await updateUser(editingId.value, {
        nick_name: form.nick_name,
        dept_id: form.dept_id,
        email: form.email || null,
        phonenumber: form.phonenumber || null,
        status: form.status,
        remark: form.remark || null,
        role_ids: form.role_ids,
      });
    }
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 拦截器已给出提示；这里保留弹层与已填内容。
  } finally {
    saving.value = false;
  }
}

async function remove(row: UserVo): Promise<void> {
  if (!(await confirmDelete(row.nick_name || row.user_name))) return;
  try {
    await deleteUser(row.user_id);
    notifySuccess();
    await load();
  } catch {
    // 同上：提示由拦截器负责。
  }
}

function openResetPassword(row: UserVo): void {
  pwdDialog.userId = row.user_id;
  pwdDialog.userName = row.user_name;
  pwdDialog.password = '';
  pwdDialog.visible = true;
}

async function submitResetPassword(): Promise<void> {
  if (pwdDialog.password.length < 8) {
    notifyWarning('密码长度至少 8 位');
    return;
  }
  pwdSaving.value = true;
  try {
    await resetUserPassword(pwdDialog.userId, pwdDialog.password);
    pwdDialog.visible = false;
    notifySuccess('密码已重置');
  } catch {
    // 保留弹层，用户可重试。
  } finally {
    pwdSaving.value = false;
  }
}

async function exportAll(): Promise<void> {
  await exportUsers({ ...query.value });
}

async function importFile(): Promise<void> {
  const file = await pickFile({ accept: '.xlsx,.xls' });
  if (!file) return;
  try {
    const report = await importUsers(file, 'partial');
    const summary = `导入完成：新增 ${report.inserted}，更新 ${report.updated}`;
    if (report.failures.length > 0) notifyWarning(`${summary}，失败 ${report.failures.length}`);
    else notifySuccess(summary);
    await load();
  } catch {
    // 同上。
  }
}

// 部门与角色是表单的依赖数据，挂载时取一次；取失败不影响列表本身。
void loadOptions();
</script>

<template>
  <AppPage :title="title" :description="'租户内的账号、部门归属与角色授权。'">
    <template #actions>
      <el-button v-perm="'system:user:export'" @click="exportAll">
        {{ t('common.export') }}
      </el-button>
      <el-button v-perm="'system:user:import'" @click="importFile">
        {{ t('common.import') }}
      </el-button>
      <el-button v-perm="'system:user:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="账号">
          <el-input v-model="query.user_name" clearable placeholder="登录名" />
        </el-form-item>
        <el-form-item label="部门">
          <el-tree-select
            v-model="query.dept_id"
            :data="deptTreeNodes"
            :props="{ label: 'label', value: 'value', children: 'children' }"
            check-strictly
            check-on-click-node
            clearable
            placeholder="全部"
            style="width: 160px"
          />
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
      row-key="user_id"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="user_name" label="账号" min-width="140" show-overflow-tooltip />
      <el-table-column prop="nick_name" label="昵称" min-width="140" show-overflow-tooltip />
      <el-table-column label="部门" min-width="140">
        <template #default="{ row }">
          {{ row.dept_id ? (deptNames.get(row.dept_id) ?? row.dept_id) : '—' }}
        </template>
      </el-table-column>
      <el-table-column label="角色" min-width="180">
        <template #default="{ row }">
          <span v-if="!row.role_ids?.length" class="ci-tertiary">—</span>
          <el-tag
            v-for="id in row.role_ids ?? []"
            v-else
            :key="id"
            class="user__role"
            effect="plain"
            round
          >
            {{ roleNames.get(id) ?? id }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column prop="email" label="邮箱" min-width="180" show-overflow-tooltip />
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }">
          <AppStatusTag :value="row.status" />
        </template>
      </el-table-column>
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="230" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'system:user:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button
            v-perm="'system:user:resetPwd'"
            link
            type="warning"
            @click="openResetPassword(row)"
          >
            重置密码
          </el-button>
          <el-button v-perm="'system:user:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'system:user:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="isCreate ? '新增用户' : '修改用户'"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" :columns="2" />
    </AppDialog>

    <AppDialog
      v-model="pwdDialog.visible"
      title="重置密码"
      width="440px"
      :confirm-loading="pwdSaving"
      @confirm="submitResetPassword"
    >
      <el-form label-width="88px" @submit.prevent>
        <el-form-item label="账号">
          <span>{{ pwdDialog.userName }}</span>
        </el-form-item>
        <el-form-item label="新密码" required>
          <el-input
            v-model="pwdDialog.password"
            type="password"
            autocomplete="new-password"
            show-password
            placeholder="至少 8 位"
          />
        </el-form-item>
      </el-form>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.user__role {
  margin-right: var(--ci-space-1);
}
</style>
