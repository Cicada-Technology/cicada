<script setup lang="ts">
/**
 * 通知公告（`system:notice:*`）。
 *
 * 两点与普通 CRUD 不同：
 * - 正文可能很长，放列表里只会把行高撑开，所以列表只显示标题，正文在弹层里编辑/阅读。
 * - `notice_type` 是后端常量（1 通知 / 2 公告），不是字典表——选项直接来自 API 模块，
 *   与后端 `NoticeType` 一一对应，避免字典缺项时下拉为空。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  createNotice,
  deleteNotice,
  listNotices,
  NOTICE_TYPE_OPTIONS,
  updateNotice,
  type NoticeListQuery,
  type NoticeUpsert,
  type NoticeVo,
} from '@/api/system/notice';
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
} = useTable<NoticeVo, NoticeListQuery>({ fetcher: listNotices });

const typeLabels = new Map(NOTICE_TYPE_OPTIONS.map((item) => [item.value as number, item.label]));

/** 阅读视图：正文已经拉回来就不要再打一次接口（列表里带了 `notice_content`）。 */
const reading = ref<NoticeVo | null>(null);

const form = reactive({
  notice_title: '',
  notice_type: NOTICE_TYPE_OPTIONS[0].value as number,
  notice_content: '',
  status: STATUS_ENABLED,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  { prop: 'notice_title', label: '标题', required: true },
  {
    prop: 'notice_type',
    label: '类型',
    type: 'radio',
    required: true,
    options: NOTICE_TYPE_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  {
    prop: 'notice_content',
    label: '正文',
    type: 'textarea',
    rows: 6,
    tips: '纯文本；富文本编辑器属于后续批次。',
  },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  { prop: 'remark', label: t('common.remark') },
]);

function resetForm(): void {
  Object.assign(form, {
    notice_title: '',
    notice_type: NOTICE_TYPE_OPTIONS[0].value,
    notice_content: '',
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

function openEdit(row: NoticeVo): void {
  editingId.value = row.notice_id;
  resetForm();
  Object.assign(form, {
    notice_title: row.notice_title,
    notice_type: row.notice_type,
    notice_content: row.notice_content ?? '',
    status: row.status,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const payload: NoticeUpsert = {
    notice_title: form.notice_title,
    notice_type: form.notice_type,
    notice_content: form.notice_content || null,
    status: form.status,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    if (editingId.value === null) await createNotice(payload);
    else await updateNotice(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    saving.value = false;
  }
}

async function remove(row: NoticeVo): Promise<void> {
  if (!(await confirmDelete(row.notice_title))) return;
  try {
    await deleteNotice(row.notice_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}
</script>

<template>
  <AppPage
    :title="title"
    :description="'面向租户内用户的通知与公告；列表只显示标题，正文点标题查看。'"
  >
    <template #actions>
      <el-button v-perm="'system:notice:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="标题">
          <el-input v-model="query.notice_title" clearable placeholder="标题关键词" />
        </el-form-item>
        <el-form-item label="类型">
          <el-select v-model="query.notice_type" clearable placeholder="全部" style="width: 120px">
            <el-option
              v-for="item in NOTICE_TYPE_OPTIONS"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </el-select>
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
      row-key="notice_id"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column label="标题" min-width="260">
        <template #default="{ row }">
          <el-button link type="primary" @click="reading = row as NoticeVo">
            {{ row.notice_title }}
          </el-button>
        </template>
      </el-table-column>
      <el-table-column label="类型" width="90">
        <template #default="{ row }">
          <el-tag
            :type="row.notice_type === 2 ? 'warning' : 'primary'"
            effect="light"
            round
            size="small"
          >
            {{ typeLabels.get(row.notice_type) ?? row.notice_type }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
      </el-table-column>
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="150" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'system:notice:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'system:notice:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'system:notice:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增公告' : '修改公告'"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" />
    </AppDialog>

    <AppDialog
      :model-value="reading !== null"
      :title="reading?.notice_title ?? ''"
      width="560px"
      hide-footer
      @update:model-value="reading = null"
    >
      <p class="notice__meta">
        {{ typeLabels.get(reading?.notice_type ?? 0) }} ·
        {{ formatDateTime(reading?.create_time) }}
      </p>
      <p class="notice__content">{{ reading?.notice_content || '（无正文）' }}</p>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.notice__meta {
  margin-bottom: var(--ci-space-3);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.notice__content {
  font-size: var(--ci-text-base);
  line-height: var(--ci-leading-relaxed);
  color: var(--ci-color-text);
  white-space: pre-wrap;
}
</style>
