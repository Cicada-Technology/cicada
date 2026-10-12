<script setup lang="ts">
/**
 * 消息模板（`notify:template:*`）。
 *
 * 关键概念是**同 code 多语言**：`code` + `locale` 唯一，例如 `order_paid` 同时有
 * `zh-CN` 与 `en-US` 两份。因此列表里把 code 与 locale 都显示出来，否则两行看起来
 * 一模一样，用户会以为重复了。
 *
 * 正文是模板文本（占位符由后端渲染时替换），不是富文本：这版只要「能写出正确文案」，
 * 富文本编辑器属于后续批次。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  CHANNEL_KIND_OPTIONS,
  createTemplate,
  deleteTemplate,
  listTemplates,
  updateTemplate,
  type TemplateListQuery,
  type TemplateUpsert,
  type TemplateVo,
} from '@/api/notify';
import { SUPPORTED_LOCALES } from '@/locales';
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
const kindLabels = new Map(CHANNEL_KIND_OPTIONS.map((item) => [item.value as string, item.label]));

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
} = useTable<TemplateVo, TemplateListQuery>({ fetcher: listTemplates });

const formRef = ref<InstanceType<typeof AppForm>>();
const dialogVisible = ref(false);
const saving = ref(false);
const editingId = ref<number | null>(null);

const form = reactive({
  code: '',
  name: '',
  channel_kind: 'webhook' as string,
  locale: 'zh-CN' as string,
  title: '',
  body: '',
  enabled: true,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  {
    prop: 'code',
    label: '模板编码',
    required: true,
    tips: '发送时按其引用；同一编码可按语言存多份。',
  },
  { prop: 'name', label: '模板名称', required: true },
  {
    prop: 'channel_kind',
    label: '适用通道',
    type: 'select',
    required: true,
    options: CHANNEL_KIND_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  {
    prop: 'locale',
    label: '语言',
    type: 'select',
    required: true,
    options: SUPPORTED_LOCALES.map((locale) => ({ value: locale, label: locale })),
  },
  { prop: 'title', label: '标题', required: true },
  {
    prop: 'body',
    label: '正文',
    type: 'textarea',
    rows: 6,
    required: true,
    tips: '支持后端约定的占位符，例如 你好 {name}。',
  },
  { prop: 'enabled', label: '启用', type: 'switch' },
  { prop: 'remark', label: t('common.remark') },
]);

function resetForm(): void {
  Object.assign(form, {
    code: '',
    name: '',
    channel_kind: 'webhook',
    locale: 'zh-CN',
    title: '',
    body: '',
    enabled: true,
    remark: '',
  });
  formRef.value?.clearValidate();
}

function openCreate(): void {
  editingId.value = null;
  resetForm();
  dialogVisible.value = true;
}

function openEdit(row: TemplateVo): void {
  editingId.value = row.id;
  resetForm();
  Object.assign(form, {
    code: row.code,
    name: row.name,
    channel_kind: row.channel_kind,
    locale: row.locale,
    title: row.title,
    body: row.body,
    enabled: row.enabled,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const payload: TemplateUpsert = {
    code: form.code,
    name: form.name,
    channel_kind: form.channel_kind,
    locale: form.locale,
    title: form.title,
    body: form.body,
    enabled: form.enabled,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    if (editingId.value === null) await createTemplate(payload);
    else await updateTemplate(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出（code + locale 重复会返回 409）。
  } finally {
    saving.value = false;
  }
}

async function remove(row: TemplateVo): Promise<void> {
  if (!(await confirmDelete(`${row.name}（${row.locale}）`))) return;
  try {
    await deleteTemplate(row.id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

/** 正文预览：列表列不出全文，用 tooltip 会太窄，这里给一个只读弹层。 */
const preview = reactive({ visible: false, row: null as TemplateVo | null });

function openPreview(row: TemplateVo): void {
  preview.row = row;
  preview.visible = true;
}
</script>

<template>
  <AppPage :title="title" :description="'同一编码可按语言存多份模板；发送时按 locale 选择。'">
    <template #actions>
      <el-button v-perm="'notify:template:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="模板编码">
          <el-input v-model="query.code" clearable placeholder="如 order_paid" />
        </el-form-item>
        <el-form-item label="适用通道">
          <el-select v-model="query.channel_kind" clearable placeholder="全部" style="width: 140px">
            <el-option
              v-for="item in CHANNEL_KIND_OPTIONS"
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
      row-key="id"
      empty-title="还没有模板"
      empty-hint="模板决定发出去的内容；先建一个再配渠道测试发送。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="code" label="编码" min-width="150" />
      <el-table-column prop="name" label="名称" min-width="150" />
      <el-table-column label="通道" width="110">
        <template #default="{ row }">{{
          kindLabels.get(row.channel_kind) ?? row.channel_kind
        }}</template>
      </el-table-column>
      <el-table-column label="语言" width="100">
        <template #default="{ row }">
          <el-tag effect="plain" round size="small">{{ row.locale }}</el-tag>
        </template>
      </el-table-column>
      <el-table-column label="标题" min-width="180">
        <template #default="{ row }">
          <el-button link type="primary" @click="openPreview(row as TemplateVo)">
            {{ row.title }}
          </el-button>
        </template>
      </el-table-column>
      <el-table-column label="启用" width="90">
        <template #default="{ row }">
          <el-tag :type="row.enabled ? 'success' : 'info'" effect="light" round size="small">
            {{ row.enabled ? '启用' : '停用' }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="150" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'notify:template:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'notify:template:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'notify:template:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增模板' : '修改模板'"
      width="720px"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" :columns="2" />
    </AppDialog>

    <AppDialog
      :model-value="preview.visible"
      :title="`模板预览：${preview.row?.code ?? ''}（${preview.row?.locale ?? ''}）`"
      width="560px"
      hide-footer
      @update:model-value="preview.visible = false"
    >
      <h3 class="template__title">{{ preview.row?.title }}</h3>
      <pre class="template__body">{{ preview.row?.body }}</pre>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.template__title {
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-text);
  margin-bottom: var(--ci-space-2);
}

.template__body {
  margin: 0;
  padding: var(--ci-space-3);
  border-radius: var(--ci-radius-md);
  background: var(--ci-color-surface-sunken);
  font-size: var(--ci-text-sm);
  line-height: var(--ci-leading-relaxed);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
