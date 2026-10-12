<script setup lang="ts">
/**
 * 菜单管理（`system:menu:*`）。
 *
 * 菜单是**全局字典**（`sys_menu` 无 `tenant_id`），它同时决定三件事：侧栏导航、
 * 动态路由的组件路径、权限点（`perms`）。所以这张表的字段比一般 CRUD 多，且互相牵连。
 *
 * 呈现用树表而不是分页表：菜单天然是树，分页会把父子拆到不同页，用户无法核对层级。
 * 类型差异（目录/菜单/按钮）用 `when` 控制字段显隐——`F` 按钮没有 path/component，
 * 显示它们只会诱导填错。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  createMenu,
  deleteMenu,
  listMenus,
  updateMenu,
  type MenuUpsert,
  type MenuVo,
} from '@/api/system/menu';
import { MENU_TYPE_OPTIONS, STATUS_ENABLED, STATUS_OPTIONS } from '@/api/types';
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
const rows = ref<MenuVo[]>([]);
const expandAll = ref(true);
const formRef = ref<InstanceType<typeof AppForm>>();
const dialogVisible = ref(false);
const saving = ref(false);
const editingId = ref<number | null>(null);

const typeLabels = new Map(MENU_TYPE_OPTIONS.map((item) => [item.value as string, item.label]));
const typeTag = new Map([
  ['M', 'info'],
  ['C', 'primary'],
  ['F', 'warning'],
]);

/** 上级菜单候选项：顶层 + 完整菜单树（目录下可挂菜单，菜单下可挂按钮）。 */
const parentNodes = computed(() => {
  const toNode = (node: MenuVo): Record<string, unknown> => ({
    value: node.menu_id,
    label: node.menu_name,
    children: (node.children ?? []).map(toNode),
  });
  return [{ value: 0, label: '顶级（根节点）' }, ...rows.value.map(toNode)];
});

const form = reactive({
  menu_name: '',
  parent_id: 0,
  order_num: 0,
  menu_type: 'C' as string,
  path: '',
  component: '',
  perms: '',
  icon: '',
  is_frame: 1,
  is_cache: 0,
  visible: 0,
  status: STATUS_ENABLED,
  remark: '',
});

const isButton = computed(() => form.menu_type === 'F');

const fields = computed<AppFormField[]>(() => [
  {
    prop: 'parent_id',
    label: '上级菜单',
    type: 'tree-select',
    treeData: parentNodes.value,
    treeLabelKey: 'label',
    treeValueKey: 'value',
    required: true,
  },
  {
    prop: 'menu_type',
    label: '类型',
    type: 'radio',
    options: MENU_TYPE_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
    required: true,
  },
  { prop: 'menu_name', label: '名称', required: true },
  { prop: 'order_num', label: t('common.sort'), type: 'number' },
  {
    prop: 'path',
    label: '路由路径',
    when: () => !isButton.value,
    tips: '种子菜单用绝对路径（/system/user），生成器产物用相对路径（goods）。',
  },
  {
    prop: 'component',
    label: '组件路径',
    when: () => form.menu_type === 'C',
    tips: '留空则按 path 推导 views/<path>/index.vue；生成器会写 <module>/<business>/index。',
  },
  {
    prop: 'perms',
    label: '权限标识',
    tips: '与后端 #[require_perm] 的字符串逐字一致，例如 system:user:list。',
  },
  { prop: 'icon', label: '图标', tips: 'Element Plus 图标名，如 User、Setting。' },
  {
    prop: 'visible',
    label: '可见',
    type: 'select',
    options: [
      { value: 0, label: '显示' },
      { value: 1, label: '隐藏' },
    ],
    when: () => !isButton.value,
  },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  {
    prop: 'is_frame',
    label: '外链',
    type: 'select',
    options: [
      { value: 1, label: '否' },
      { value: 0, label: '是' },
    ],
    when: () => form.menu_type === 'C',
    tips: '沿用参考实现的取反语义：1 非外链，0 外链。',
  },
  {
    prop: 'is_cache',
    label: '缓存页面',
    type: 'select',
    options: [
      { value: 1, label: '缓存' },
      { value: 0, label: '不缓存' },
    ],
    when: () => form.menu_type === 'C',
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

async function load(): Promise<void> {
  loading.value = true;
  try {
    rows.value = await listMenus();
  } finally {
    loading.value = false;
  }
}

function resetForm(): void {
  Object.assign(form, {
    menu_name: '',
    parent_id: 0,
    order_num: 0,
    menu_type: 'C',
    path: '',
    component: '',
    perms: '',
    icon: '',
    is_frame: 1,
    is_cache: 0,
    visible: 0,
    status: STATUS_ENABLED,
    remark: '',
  });
  formRef.value?.clearValidate();
}

function openCreate(parent?: MenuVo): void {
  editingId.value = null;
  resetForm();
  if (parent) form.parent_id = parent.menu_id;
  dialogVisible.value = true;
}

function openEdit(row: MenuVo): void {
  editingId.value = row.menu_id;
  resetForm();
  Object.assign(form, {
    menu_name: row.menu_name,
    parent_id: row.parent_id,
    order_num: row.order_num,
    menu_type: row.menu_type,
    path: row.path ?? '',
    component: row.component ?? '',
    perms: row.perms ?? '',
    icon: row.icon ?? '',
    is_frame: row.is_frame ?? 1,
    is_cache: row.is_cache ?? 0,
    visible: row.visible,
    status: row.status,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
}

function payload(): MenuUpsert {
  return {
    menu_name: form.menu_name,
    parent_id: form.parent_id,
    order_num: form.order_num,
    menu_type: form.menu_type as MenuUpsert['menu_type'],
    path: isButton.value ? null : form.path || null,
    component: form.menu_type === 'C' ? form.component || null : null,
    perms: form.perms || null,
    icon: form.icon || null,
    is_frame: form.is_frame,
    is_cache: form.is_cache,
    visible: isButton.value ? 0 : form.visible,
    status: form.status,
    remark: form.remark || null,
  };
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  saving.value = true;
  try {
    if (editingId.value === null) await createMenu(payload());
    else await updateMenu(editingId.value, payload());
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    saving.value = false;
  }
}

async function remove(row: MenuVo): Promise<void> {
  if (!(await confirmDelete(row.menu_name))) return;
  try {
    await deleteMenu(row.menu_id);
    notifySuccess();
    await load();
  } catch {
    // 有子菜单时后端会拒绝，提示由拦截器给出。
  }
}

void load();
</script>

<template>
  <AppPage
    :title="title"
    :description="'菜单同时决定侧栏导航、动态路由与权限点，修改后需重新登录生效。'"
  >
    <template #actions>
      <el-button @click="expandAll = !expandAll">
        {{ expandAll ? t('table.collapseAll') : t('table.expandAll') }}
      </el-button>
      <el-button v-perm="'system:menu:add'" type="primary" @click="openCreate()">
        {{ t('common.add') }}
      </el-button>
    </template>

    <AppTable
      :rows="rows"
      :loading="loading"
      :total="null"
      tree
      row-key="menu_id"
      :expand-all="expandAll"
      empty-hint="菜单树为空说明种子数据未执行，先跑一次 migrate up --seed。"
    >
      <el-table-column prop="menu_name" label="名称" min-width="200" />
      <el-table-column label="类型" width="90">
        <template #default="{ row }">
          <el-tag :type="typeTag.get(row.menu_type) ?? 'info'" effect="light" round size="small">
            {{ typeLabels.get(row.menu_type) ?? row.menu_type }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column prop="path" label="路由路径" min-width="160" show-overflow-tooltip />
      <el-table-column prop="component" label="组件" min-width="180" show-overflow-tooltip />
      <el-table-column label="权限标识" min-width="180">
        <template #default="{ row }">
          <code v-if="row.perms" class="menu__perm">{{ row.perms }}</code>
          <span v-else class="ci-tertiary">—</span>
        </template>
      </el-table-column>
      <el-table-column :label="t('common.sort')" width="80" prop="order_num" />
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="200" fixed="right">
        <template #default="{ row }">
          <el-button
            v-perm="'system:menu:add'"
            link
            type="primary"
            @click="openCreate(row as MenuVo)"
          >
            新增子项
          </el-button>
          <el-button v-perm="'system:menu:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'system:menu:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增菜单' : '修改菜单'"
      width="720px"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" :columns="2" />
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.menu__perm {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}
</style>
