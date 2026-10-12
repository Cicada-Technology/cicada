<script setup lang="ts">
/**
 * 操作日志（只读，`monitor:operlog:*`）。
 *
 * 审计日志的价值全在**详情**：谁、什么时候、调了哪个接口、带了什么参数、返回/报错是什么。
 * 列表列不下这些东西，所以正文（`oper_param` / `json_result` / `error_msg`）放抽屉里，
 * 并且用等宽字体 + 不换行折行展示——JSON 被折成散文就没法看了。
 *
 * 「清空」按当前筛选条件走（后端语义如此），所以确认文案里必须写清是「清空筛选结果」
 * 还是「清空全部」，这是最容易误操作的一步。
 */
import { computed, reactive } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  BUSINESS_TYPE_OPTIONS,
  cleanOperLogs,
  deleteOperLog,
  exportOperLogs,
  getOperLog,
  listOperLogs,
  type OperLogListQuery,
  type OperLogVo,
} from '@/api/monitor/log';
import { STATUS_OPTIONS } from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
import AppDrawer from '@/components/AppDrawer.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppStatusTag from '@/components/AppStatusTag.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { notifySuccess } from '@/utils/feedback';
import { formatDateTime, formatDuration, orPlaceholder } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirm, confirmDelete } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const businessLabels = new Map(
  BUSINESS_TYPE_OPTIONS.map((item) => [item.value as number, item.label]),
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
} = useTable<OperLogVo, OperLogListQuery>({ fetcher: listOperLogs });

const detail = reactive({ visible: false, loading: false, log: null as OperLogVo | null });
const cleanDialog = reactive({ visible: false, all: false });

const hasFilter = computed(() =>
  Object.entries(query.value).some(([key, value]) =>
    key === 'page' || key === 'page_size' ? false : value !== undefined && value !== '',
  ),
);

async function openDetail(row: OperLogVo): Promise<void> {
  detail.visible = true;
  detail.loading = true;
  detail.log = row;
  try {
    // 列表里的 `oper_param` 等字段可能被后端截断，详情接口给完整内容。
    detail.log = await getOperLog(row.oper_id);
  } catch {
    // 保留列表里的那份内容，提示由拦截器给出。
  } finally {
    detail.loading = false;
  }
}

async function remove(row: OperLogVo): Promise<void> {
  if (!(await confirmDelete(`「${row.title}」于 ${formatDateTime(row.oper_time)} 的记录`))) return;
  try {
    await deleteOperLog(row.oper_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

async function clean(): Promise<void> {
  const scope = cleanDialog.all ? '全部' : '当前筛选结果';
  const agreed = await confirm(`确定清空${scope}操作日志吗？此操作不可撤销。`, { danger: true });
  if (!agreed) return;
  try {
    await cleanOperLogs(cleanDialog.all ? {} : { ...query.value });
    cleanDialog.visible = false;
    notifySuccess('日志已清理');
    await load();
  } catch {
    // 同上。
  }
}

async function exportAll(): Promise<void> {
  await exportOperLogs({ ...query.value });
}

/** 详情里的 JSON 尽量格式化，失败就原样显示（半截 JSON 也比空着有用）。 */
function prettyJson(raw?: string | null): string {
  if (!raw) return '';
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}
</script>

<template>
  <AppPage :title="title" :description="'写入型接口的审计记录；正文看详情抽屉。'">
    <template #actions>
      <el-button v-perm="'monitor:operlog:remove'" @click="cleanDialog.visible = true">
        清空
      </el-button>
      <el-button v-perm="'monitor:operlog:export'" @click="exportAll">
        {{ t('common.export') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="模块标题">
          <el-input v-model="query.title" clearable placeholder="如 role:add" />
        </el-form-item>
        <el-form-item label="操作账号">
          <el-input v-model="query.oper_name" clearable placeholder="登录名" />
        </el-form-item>
        <el-form-item label="业务类型">
          <el-select
            v-model="query.business_type"
            clearable
            placeholder="全部"
            style="width: 130px"
          >
            <el-option
              v-for="item in BUSINESS_TYPE_OPTIONS"
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
              :label="item.value === 0 ? '成功' : '失败'"
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
      row-key="oper_id"
      empty-title="没有操作日志"
      empty-hint="调整筛选条件；写操作会在成功与失败时各记一条。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="title" label="模块" min-width="140" />
      <el-table-column label="类型" width="90">
        <template #default="{ row }">
          {{ businessLabels.get(row.business_type) ?? row.business_type }}
        </template>
      </el-table-column>
      <el-table-column prop="request_method" label="方法" width="90" />
      <el-table-column prop="oper_name" label="账号" min-width="120" />
      <el-table-column label="URL" min-width="220">
        <template #default="{ row }">
          <code class="operlog__code">{{ row.oper_url }}</code>
        </template>
      </el-table-column>
      <el-table-column prop="oper_ip" label="IP" min-width="130" />
      <el-table-column label="结果" width="90">
        <template #default="{ row }">
          <AppStatusTag :value="row.status" variant="outcome" />
        </template>
      </el-table-column>
      <el-table-column label="耗时" width="100">
        <template #default="{ row }">{{ formatDuration(row.cost_ms) }}</template>
      </el-table-column>
      <el-table-column label="时间" width="170">
        <template #default="{ row }">{{ formatDateTime(row.oper_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="140" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'monitor:operlog:detail'" link type="primary" @click="openDetail(row)">
            {{ t('common.detail') }}
          </el-button>
          <el-button v-perm="'monitor:operlog:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>
    </AppTable>

    <AppDialog
      v-model="cleanDialog.visible"
      title="清空操作日志"
      width="460px"
      confirm-type="danger"
      @confirm="clean"
    >
      <el-form label-width="110px" @submit.prevent>
        <el-form-item label="清空范围">
          <el-radio-group v-model="cleanDialog.all">
            <el-radio :value="false">当前筛选结果</el-radio>
            <el-radio :value="true">全部</el-radio>
          </el-radio-group>
        </el-form-item>
      </el-form>
      <p class="operlog__hint">
        {{
          hasFilter
            ? '当前查询带筛选条件，选「当前筛选结果」只清掉这些记录。'
            : '当前没有筛选条件，「当前筛选结果」等于全部。'
        }}
      </p>
    </AppDialog>

    <AppDrawer v-model="detail.visible" title="操作详情" size="640px">
      <div v-loading="detail.loading" class="operlog__detail">
        <dl>
          <dt>模块</dt>
          <dd>{{ orPlaceholder(detail.log?.title) }}</dd>
          <dt>调用</dt>
          <dd>
            <code>{{ detail.log?.method }}</code>
          </dd>
          <dt>请求</dt>
          <dd>
            <code>{{ detail.log?.request_method }} {{ detail.log?.oper_url }}</code>
          </dd>
          <dt>账号</dt>
          <dd>
            {{ orPlaceholder(detail.log?.oper_name) }} · {{ orPlaceholder(detail.log?.oper_ip) }}
          </dd>
          <dt>时间</dt>
          <dd>
            {{ formatDateTime(detail.log?.oper_time) }}（{{ formatDuration(detail.log?.cost_ms) }}）
          </dd>
        </dl>

        <template v-if="detail.log?.oper_param">
          <h3 class="operlog__section">请求参数</h3>
          <pre class="operlog__pre">{{ prettyJson(detail.log?.oper_param) }}</pre>
        </template>

        <template v-if="detail.log?.error_msg">
          <h3 class="operlog__section">错误信息</h3>
          <pre class="operlog__pre operlog__pre--error">{{ detail.log?.error_msg }}</pre>
        </template>

        <template v-if="detail.log?.json_result">
          <h3 class="operlog__section">返回结果</h3>
          <pre class="operlog__pre">{{ prettyJson(detail.log?.json_result) }}</pre>
        </template>
      </div>
    </AppDrawer>
  </AppPage>
</template>

<style scoped>
.operlog__code {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}

.operlog__hint {
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.operlog__detail {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  font-size: var(--ci-text-sm);
}

.operlog__detail dl {
  display: grid;
  grid-template-columns: 68px 1fr;
  gap: var(--ci-space-2);
}

.operlog__detail dt {
  color: var(--ci-color-text-tertiary);
}

.operlog__detail dd {
  margin: 0;
  color: var(--ci-color-text);
  overflow-wrap: anywhere;
}

.operlog__section {
  font-size: var(--ci-text-sm);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-text-secondary);
}

.operlog__pre {
  max-height: 260px;
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

.operlog__pre--error {
  color: var(--ci-color-danger);
}
</style>
