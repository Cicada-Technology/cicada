<script setup lang="ts">
/**
 * 通知渠道（`notify:channel:*`，ADR 0002）。
 *
 * `config` 是按 `kind` 结构不同的 JSON（webhook 地址、SMTP 账号…）。这一版直接给一个
 * JSON 文本域而不是动态表单：每种通道的字段由 `crates/notify` 的实现决定，前端若硬编码
 * 一份字段表，后端加一个通道就会不同步。文本域在提交前做 `JSON.parse` 校验，至少保证
 * 不会把坏 JSON 存进去。
 *
 * 「测试发送」是这张页面的核心动作：渠道配置对不对，只有真发一次才知道。
 */
import { computed, reactive, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  CHANNEL_KIND_OPTIONS,
  createChannel,
  deleteChannel,
  listChannels,
  sendNotification,
  updateChannel,
  type ChannelListQuery,
  type ChannelUpsert,
  type ChannelVo,
} from '@/api/notify';
import AppDialog from '@/components/AppDialog.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { notifySuccess, notifyWarning } from '@/utils/feedback';
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
} = useTable<ChannelVo, ChannelListQuery>({ fetcher: listChannels });

const kindOptions = CHANNEL_KIND_OPTIONS.map((item) => ({ value: item.value, label: item.label }));

const formRef = ref<InstanceType<typeof AppForm>>();
const dialogVisible = ref(false);
const saving = ref(false);
const editingId = ref<number | null>(null);

const CONFIG_TEMPLATES: Record<string, string> = {
  webhook: JSON.stringify({ endpoint: 'https://example.com/webhook' }, null, 2),
  sms: JSON.stringify({ endpoint: 'https://example.com/sms', sign: 'cicada' }, null, 2),
  im: JSON.stringify({ endpoint: 'https://example.com/im' }, null, 2),
  email: JSON.stringify(
    { endpoint: 'https://example.com/email', from: 'no-reply@example.com' },
    null,
    2,
  ),
};

const form = reactive({
  code: '',
  name: '',
  kind: 'webhook' as string,
  config: CONFIG_TEMPLATES.webhook,
  enabled: true,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  { prop: 'code', label: '渠道编码', required: true, tips: '发送时按其引用，例如 sms-main。' },
  { prop: 'name', label: '渠道名称', required: true },
  { prop: 'kind', label: '通道类型', type: 'select', required: true, options: kindOptions },
  {
    prop: 'config',
    label: '配置（JSON）',
    type: 'textarea',
    rows: 8,
    required: true,
    tips:
      form.kind === 'email'
        ? 'email 需要 config.endpoint（HTTP 邮件网关）或 config.host（SMTP），并填写 from。'
        : 'webhook / sms / im 需要 config.endpoint；token、sign 等字段按通道类型补充。',
  },
  { prop: 'enabled', label: '启用', type: 'switch' },
  { prop: 'remark', label: t('common.remark') },
]);

function resetForm(): void {
  Object.assign(form, {
    code: '',
    name: '',
    kind: 'webhook',
    config: CONFIG_TEMPLATES.webhook,
    enabled: true,
    remark: '',
  });
  formRef.value?.clearValidate();
}

function isTemplateConfig(raw: string): boolean {
  const trimmed = raw.trim();
  return (
    trimmed === '' ||
    trimmed === '{}' ||
    Object.values(CONFIG_TEMPLATES).some((template) => template.trim() === trimmed)
  );
}

watch(
  () => form.kind,
  (kind) => {
    if (isTemplateConfig(form.config)) form.config = CONFIG_TEMPLATES[kind] ?? '{}';
  },
);

function openCreate(): void {
  editingId.value = null;
  resetForm();
  dialogVisible.value = true;
}

function openEdit(row: ChannelVo): void {
  editingId.value = row.id;
  resetForm();
  Object.assign(form, {
    code: row.code,
    name: row.name,
    kind: row.kind,
    config: JSON.stringify(row.config ?? {}, null, 2),
    enabled: row.enabled,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
}

/** 把文本域解析成对象；失败时给出可直接显示的中文错误。 */
function parseConfig(): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(form.config || '{}');
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) return null;
    return parsed as Record<string, unknown>;
  } catch {
    return null;
  }
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const config = parseConfig();
  if (!config) {
    notifyWarning('配置必须是合法的 JSON 对象，例如 {"endpoint": "https://…"}');
    return;
  }
  const endpoint = typeof config.endpoint === 'string' ? config.endpoint.trim() : '';
  const host = typeof config.host === 'string' ? config.host.trim() : '';
  if (form.kind !== 'email' && endpoint === '') {
    notifyWarning('config.endpoint 不能为空');
    return;
  }
  if (form.kind === 'email' && endpoint === '' && host === '') {
    notifyWarning('email 渠道需要 config.endpoint（HTTP 网关）或 config.host（SMTP）');
    return;
  }
  const payload: ChannelUpsert = {
    code: form.code,
    name: form.name,
    kind: form.kind,
    config,
    enabled: form.enabled,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    if (editingId.value === null) await createChannel(payload);
    else await updateChannel(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    saving.value = false;
  }
}

async function remove(row: ChannelVo): Promise<void> {
  if (!(await confirmDelete(row.name))) return;
  try {
    await deleteChannel(row.id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

// ---------------------------------------------------------------- 测试发送
const test = reactive({
  visible: false,
  channel: null as ChannelVo | null,
  templateCode: '',
  recipient: '',
  payload: '{}',
  sending: false,
  result: '',
});

function openTest(row: ChannelVo): void {
  test.channel = row;
  test.templateCode = '';
  test.recipient = '';
  test.payload = '{}';
  test.result = '';
  test.visible = true;
}

async function submitTest(): Promise<void> {
  if (!test.channel) return;
  if (test.templateCode.trim() === '') {
    notifyWarning('模板编码不能为空');
    return;
  }
  if (test.recipient.trim() === '') {
    notifyWarning('接收方不能为空');
    return;
  }
  let payload: Record<string, unknown>;
  try {
    const parsed: unknown = JSON.parse(test.payload || '{}');
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new Error('not an object');
    }
    payload = parsed as Record<string, unknown>;
  } catch {
    notifyWarning('变量必须是合法 JSON 对象');
    return;
  }
  test.sending = true;
  try {
    const result = await sendNotification({
      channel_code: test.channel.code,
      template_code: test.templateCode,
      recipient: test.recipient,
      payload,
    });
    test.result = result.error
      ? `失败：${result.error}`
      : `已发送。渲染内容：\n${result.rendered_body}`;
  } catch {
    // 失败详情由 sendNotification 的响应体给出，拦截器会提示。
  } finally {
    test.sending = false;
  }
}
</script>

<template>
  <AppPage :title="title" :description="'渠道是消息的实际出口；配置存 JSON，测试发送验证连通性。'">
    <template #actions>
      <el-button v-perm="'notify:channel:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="渠道编码">
          <el-input v-model="query.code" clearable placeholder="如 sms-main" />
        </el-form-item>
        <el-form-item label="通道类型">
          <el-select v-model="query.kind" clearable placeholder="全部" style="width: 140px">
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
      empty-title="还没有通知渠道"
      empty-hint="先配一个渠道（如 webhook），模板才能通过它投递。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="code" label="渠道编码" min-width="140" />
      <el-table-column prop="name" label="名称" min-width="140" />
      <el-table-column label="类型" width="110">
        <template #default="{ row }">{{ kindLabels.get(row.kind) ?? row.kind }}</template>
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

      <el-table-column :label="t('common.actions')" width="200" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'notify:send'" link type="primary" @click="openTest(row)">
            测试发送
          </el-button>
          <el-button v-perm="'notify:channel:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'notify:channel:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'notify:channel:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增渠道' : '修改渠道'"
      width="720px"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" />
    </AppDialog>

    <AppDialog
      v-model="test.visible"
      :title="`测试发送：${test.channel?.name ?? ''}`"
      :confirm-loading="test.sending"
      confirm-text="发送"
      @confirm="submitTest"
    >
      <el-form label-width="88px" @submit.prevent>
        <el-form-item label="模板编码" required>
          <el-input v-model="test.templateCode" placeholder="模板 code" />
        </el-form-item>
        <el-form-item label="接收方" required>
          <el-input v-model="test.recipient" placeholder="手机号 / 邮箱 / webhook 目标" />
        </el-form-item>
        <el-form-item label="变量">
          <el-input v-model="test.payload" type="textarea" :rows="4" />
        </el-form-item>
      </el-form>
      <pre v-if="test.result" class="channel__result">{{ test.result }}</pre>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.channel__result {
  margin-top: var(--ci-space-3);
  padding: var(--ci-space-3);
  border-radius: var(--ci-radius-md);
  background: var(--ci-color-surface-sunken);
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
