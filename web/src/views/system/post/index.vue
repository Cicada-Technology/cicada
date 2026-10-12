<script setup lang="ts">
/**
 * 岗位管理（`system:post:*`）。
 *
 * 骨架里最标准的一张分页 CRUD：字段少、无层级、无字典依赖。新增业务模块时照抄它
 * 的结构（列表 → 查询条 → 树/表 → 弹层表单）即可，其余页面的复杂度都来自这三个要素的
 * 某种组合。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  createPost,
  deletePost,
  exportPosts,
  listPosts,
  updatePost,
  type PostListQuery,
  type PostUpsert,
  type PostVo,
} from '@/api/system/post';
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
} = useTable<PostVo, PostListQuery>({ fetcher: listPosts });

const form = reactive({
  post_code: '',
  post_name: '',
  post_sort: 0,
  status: STATUS_ENABLED,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  {
    prop: 'post_code',
    label: '岗位编码',
    required: true,
    tips: '如 ceo、se，创建后建议不再修改。',
  },
  { prop: 'post_name', label: '岗位名称', required: true },
  { prop: 'post_sort', label: t('common.sort'), type: 'number' },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

function resetForm(): void {
  Object.assign(form, {
    post_code: '',
    post_name: '',
    post_sort: 0,
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

function openEdit(row: PostVo): void {
  editingId.value = row.post_id;
  resetForm();
  Object.assign(form, {
    post_code: row.post_code,
    post_name: row.post_name,
    post_sort: row.post_sort,
    status: row.status,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const payload: PostUpsert = {
    post_code: form.post_code,
    post_name: form.post_name,
    post_sort: form.post_sort,
    status: form.status,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    if (editingId.value === null) await createPost(payload);
    else await updatePost(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    saving.value = false;
  }
}

async function remove(row: PostVo): Promise<void> {
  if (!(await confirmDelete(row.post_name))) return;
  try {
    await deletePost(row.post_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

async function exportAll(): Promise<void> {
  await exportPosts({ ...query.value });
}
</script>

<template>
  <AppPage :title="title" :description="'岗位是用户在组织内的职务标签，与权限无关。'">
    <template #actions>
      <el-button v-perm="'system:post:export'" @click="exportAll">{{
        t('common.export')
      }}</el-button>
      <el-button v-perm="'system:post:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="岗位编码">
          <el-input v-model="query.post_code" clearable placeholder="如 ceo" />
        </el-form-item>
        <el-form-item label="岗位名称">
          <el-input v-model="query.post_name" clearable placeholder="岗位名称" />
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
      row-key="post_id"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="post_code" label="岗位编码" min-width="140" />
      <el-table-column prop="post_name" label="岗位名称" min-width="160" />
      <el-table-column prop="post_sort" :label="t('common.sort')" width="80" />
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
      </el-table-column>
      <el-table-column
        prop="remark"
        :label="t('common.remark')"
        min-width="200"
        show-overflow-tooltip
      />
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="150" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'system:post:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'system:post:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'system:post:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增岗位' : '修改岗位'"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" />
    </AppDialog>
  </AppPage>
</template>
