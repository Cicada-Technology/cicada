<script setup lang="ts">
/**
 * 发送记录（`notify:record:list` + `notify:send`）。
 *
 * 记录是只读的投递流水：成功与失败都在这里，失败条目带 `error`，这是排查「用户说没收到」
 * 的第一现场。列表只放可筛选的字段，变量与错误信息在抽屉里看。
 *
 * 「发送测试」放在这里而不是只放在渠道页：验证模板渲染时应该能立即看到记录，
 * 两个页面各承担一半的验证职责。
 */
import { computed, reactive } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  listRecords,
  RECORD_STATUS_OPTIONS,
  sendNotification,
  type RecordListQuery,
  type RecordVo,
} from '@/api/notify';
import AppDialog from '@/components/AppDialog.vue';
import AppDrawer from '@/components/AppDrawer.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppTable from '@/components/AppTable.vue';
import { useTable } from '@/composables/useTable';
import { notifyWarning } from '@/utils/feedback';
import { formatDateTime, orPlaceholder } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();

const title = computed(() => route.meta.title ?? '');
const statusLabels = new Map(
  RECORD_STATUS_OPTIONS.map((item) => [item.value as number, item.label]),
);

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
} = useTable<RecordVo, RecordListQuery>({ fetcher: listRecords });

const detail = reactive({ visible: false, row: null as RecordVo | null });

function openDetail(row: RecordVo): void {
  detail.row = row;
  detail.visible = true;
}

const send = reactive({
  visible: false,
  channelCode: '',
  templateCode: '',
  recipient: '',
  locale: '',
  payload: '{}',
  sending: false,
  result: '',
});

function openSend(): void {
  send.channelCode = '';
  send.templateCode = '';
  send.recipient = '';
  send.locale = '';
  send.payload = '{}';
  send.result = '';
  send.visible = true;
}

async function submitSend(): Promise<void> {
  if (send.channelCode.trim() === '') {
    notifyWarning('渠道编码不能为空');
    return;
  }
  if (send.templateCode.trim() === '') {
    notifyWarning('模板编码不能为空');
    return;
  }
  if (send.recipient.trim() === '') {
    notifyWarning('接收方不能为空');
    return;
  }
  let payload: Record<string, unknown>;
  try {
    const parsed: unknown = JSON.parse(send.payload || '{}');
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new Error('not an object');
    }
    payload = parsed as Record<string, unknown>;
  } catch {
    notifyWarning('变量必须是合法 JSON 对象');
    return;
  }
  send.sending = true;
  try {
    const result = await sendNotification({
      channel_code: send.channelCode,
      template_code: send.templateCode,
      recipient: send.recipient,
      locale: send.locale || undefined,
      payload,
    });
    send.result = result.error
      ? `失败：${result.error}`
      : `已发送（记录 #${result.record_id}）。渲染内容：\n${result.rendered_body}`;
    await load();
  } catch {
    // 提示由拦截器给出。
  } finally {
    send.sending = false;
  }
}
</script>

<template>
  <AppPage :title="title" :description="'投递流水；失败条目是排查「没收到」的第一现场。'">
    <template #actions>
      <el-button v-perm="'notify:send'" type="primary" @click="openSend">发送测试</el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="模板编码">
          <el-input v-model="query.template_code" clearable placeholder="如 order_paid" />
        </el-form-item>
        <el-form-item label="接收方">
          <el-input v-model="query.recipient" clearable placeholder="手机号 / 邮箱" />
        </el-form-item>
        <el-form-item :label="t('common.status')">
          <el-select v-model="query.status" clearable placeholder="全部" style="width: 120px">
            <el-option
              v-for="item in RECORD_STATUS_OPTIONS"
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
      empty-title="还没有发送记录"
      empty-hint="记录在模板通过渠道投递时产生；也可以先用「发送测试」造一条。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="template_code" label="模板编码" min-width="150" />
      <el-table-column prop="recipient" label="接收方" min-width="180" show-overflow-tooltip />
      <el-table-column label="结果" width="100">
        <template #default="{ row }">
          <el-tag :type="row.status === 1 ? 'success' : 'danger'" effect="light" round size="small">
            {{ statusLabels.get(row.status) ?? row.status }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="错误" min-width="180">
        <template #default="{ row }">
          <span :class="{ 'ci-tertiary': !row.error }">{{ orPlaceholder(row.error) }}</span>
        </template>
      </el-table-column>
      <el-table-column prop="create_by" label="触发者" min-width="120" />
      <el-table-column label="创建时间" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>
      <el-table-column label="发送时间" width="170">
        <template #default="{ row }">{{ formatDateTime(row.sent_at) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="100" fixed="right">
        <template #default="{ row }">
          <el-button link type="primary" @click="openDetail(row)">{{
            t('common.detail')
          }}</el-button>
        </template>
      </el-table-column>
    </AppTable>

    <AppDrawer v-model="detail.visible" title="发送详情" size="560px">
      <div class="record__detail">
        <dl>
          <dt>模板</dt>
          <dd>{{ orPlaceholder(detail.row?.template_code) }}</dd>
          <dt>接收方</dt>
          <dd>{{ orPlaceholder(detail.row?.recipient) }}</dd>
          <dt>渠道 id</dt>
          <dd>{{ detail.row?.channel_id }}</dd>
          <dt>触发者</dt>
          <dd>{{ orPlaceholder(detail.row?.create_by) }}</dd>
          <dt>状态</dt>
          <dd>{{ statusLabels.get(detail.row?.status ?? 0) ?? detail.row?.status }}</dd>
        </dl>

        <h3 class="record__section">变量</h3>
        <pre class="record__pre">{{ JSON.stringify(detail.row?.payload ?? {}, null, 2) }}</pre>

        <template v-if="detail.row?.error">
          <h3 class="record__section">错误信息</h3>
          <pre class="record__pre record__pre--error">{{ detail.row.error }}</pre>
        </template>
      </div>
    </AppDrawer>

    <AppDialog
      v-model="send.visible"
      title="发送测试"
      :confirm-loading="send.sending"
      confirm-text="发送"
      @confirm="submitSend"
    >
      <el-form label-width="88px" @submit.prevent>
        <el-form-item label="渠道编码" required>
          <el-input v-model="send.channelCode" placeholder="如 sms-main" />
        </el-form-item>
        <el-form-item label="模板编码" required>
          <el-input v-model="send.templateCode" placeholder="如 order_paid" />
        </el-form-item>
        <el-form-item label="接收方" required>
          <el-input v-model="send.recipient" placeholder="手机号 / 邮箱" />
        </el-form-item>
        <el-form-item label="语言">
          <el-input v-model="send.locale" placeholder="留空用默认语言，如 zh-CN" />
        </el-form-item>
        <el-form-item label="变量">
          <el-input v-model="send.payload" type="textarea" :rows="4" />
        </el-form-item>
      </el-form>
      <pre v-if="send.result" class="record__pre">{{ send.result }}</pre>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.record__detail {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  font-size: var(--ci-text-sm);
}

.record__detail dl {
  display: grid;
  grid-template-columns: 72px 1fr;
  gap: var(--ci-space-2);
}

.record__detail dt {
  color: var(--ci-color-text-tertiary);
}

.record__detail dd {
  margin: 0;
  color: var(--ci-color-text);
  overflow-wrap: anywhere;
}

.record__section {
  font-size: var(--ci-text-sm);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-text-secondary);
}

.record__pre {
  max-height: 240px;
  overflow: auto;
  margin: 0;
  padding: var(--ci-space-3);
  border-radius: var(--ci-radius-md);
  background: var(--ci-color-surface-sunken);
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  line-height: var(--ci-leading-normal);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.record__pre--error {
  color: var(--ci-color-danger);
}
</style>
