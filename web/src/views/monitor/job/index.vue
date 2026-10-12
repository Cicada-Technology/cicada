<script setup lang="ts">
/**
 * 定时任务（`monitor:job:*`）。
 *
 * 三个概念在这张页面上必须分清，否则用户会误操作：
 * - **状态位**（`status`）：0 正常 / 1 暂停。改了它才影响调度，持久生效。
 * - **立即执行一次**：单次触发的旁路动作，不改状态，成功后去看执行日志。
 * - **并发策略**：0 允许并发 / 1 禁止并发。集群里由分布式锁选主，
 *   禁止并发是集群下的默认建议值——同一次调度只允许一个实例执行。
 *
 * 执行日志用抽屉：它是「看一眼」的次要信息，不该把主表挤走。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  changeJobStatus,
  createJob,
  deleteJob,
  exportJobs,
  JOB_CONCURRENCY_OPTIONS,
  listJobLogs,
  listJobs,
  runJobOnce,
  updateJob,
  type JobListQuery,
  type JobLogListQuery,
  type JobLogVo,
  type JobUpsert,
  type JobVo,
} from '@/api/monitor/job';
import { STATUS_ENABLED, STATUS_OPTIONS } from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
import AppDrawer from '@/components/AppDrawer.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
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
const { confirmDelete, confirm } = useConfirm();

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
} = useTable<JobVo, JobListQuery>({ fetcher: listJobs });

const form = reactive({
  job_name: '',
  job_group: '',
  invoke_target: '',
  cron_expression: '',
  concurrency: 1,
  timeout_secs: 0,
  max_retry: 0,
  status: STATUS_ENABLED,
  remark: '',
});

const fields = computed<AppFormField[]>(() => [
  { prop: 'job_name', label: '任务名称', required: true },
  { prop: 'job_group', label: '任务分组', required: true, tips: '如 DEFAULT、report。' },
  {
    prop: 'invoke_target',
    label: '调用目标',
    required: true,
    tips: '后端注册的任务标识（Bean 名或函数路径），不是任意字符串。',
  },
  {
    prop: 'cron_expression',
    label: 'Cron',
    required: true,
    tips: '标准 6 段 cron，例：0 0/5 * * * ?',
  },
  {
    prop: 'concurrency',
    label: '并发策略',
    type: 'select',
    options: JOB_CONCURRENCY_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
    tips: '集群下建议禁止并发：同一次调度只让一个实例执行。',
  },
  { prop: 'timeout_secs', label: '超时（秒）', type: 'number' },
  { prop: 'max_retry', label: '失败重试', type: 'number' },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: [
      { value: 0, label: '正常（参与调度）' },
      { value: 1, label: '暂停' },
    ],
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

function resetForm(): void {
  Object.assign(form, {
    job_name: '',
    job_group: '',
    invoke_target: '',
    cron_expression: '',
    concurrency: 1,
    timeout_secs: 0,
    max_retry: 0,
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

function openEdit(row: JobVo): void {
  editingId.value = row.job_id;
  resetForm();
  Object.assign(form, {
    job_name: row.job_name,
    job_group: row.job_group,
    invoke_target: row.invoke_target,
    cron_expression: row.cron_expression,
    concurrency: row.concurrency,
    timeout_secs: row.timeout_secs,
    max_retry: row.max_retry,
    status: row.status,
    remark: row.remark ?? '',
  });
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const payload: JobUpsert = { ...form, remark: form.remark || null };
  saving.value = true;
  try {
    if (editingId.value === null) await createJob(payload);
    else await updateJob(editingId.value, payload);
    dialogVisible.value = false;
    notifySuccess();
    await load();
  } catch {
    // 提示由拦截器给出（cron 非法或目标不存在都会在这里被挡住）。
  } finally {
    saving.value = false;
  }
}

async function remove(row: JobVo): Promise<void> {
  if (!(await confirmDelete(row.job_name))) return;
  try {
    await deleteJob(row.job_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

/** 切换调度状态：文案里说明「下一步会怎样」，避免用户以为是立即执行。 */
async function toggleStatus(row: JobVo): Promise<void> {
  const next = row.status === STATUS_ENABLED ? 1 : 0;
  const action = next === 0 ? '恢复调度' : '暂停调度';
  const agreed = await confirm(`确定${action}「${row.job_name}」吗？`, { danger: false });
  if (!agreed) return;
  try {
    await changeJobStatus(row.job_id, next);
    notifySuccess(`已${action}`);
    await load();
  } catch {
    // 同上。
  }
}

async function runOnce(row: JobVo): Promise<void> {
  const agreed = await confirm(`立即执行一次「${row.job_name}」？不影响它的调度状态。`);
  if (!agreed) return;
  try {
    await runJobOnce(row.job_id);
    notifySuccess('已触发一次执行，稍后可在执行日志里查看结果');
  } catch {
    // 同上。
  }
}

// ------------------------------------------------------------ 执行日志抽屉
const logDrawer = reactive({ visible: false, jobId: 0, jobName: '' });
const logLoading = ref(false);
const logs = ref<JobLogVo[]>([]);

async function openLogs(row: JobVo): Promise<void> {
  logDrawer.jobId = row.job_id;
  logDrawer.jobName = row.job_name;
  logDrawer.visible = true;
  await loadLogs();
}

async function loadLogs(): Promise<void> {
  logLoading.value = true;
  try {
    const params: JobLogListQuery = { page: 1, page_size: 50 };
    const data = await listJobLogs(logDrawer.jobId, params);
    logs.value = data.items;
  } finally {
    logLoading.value = false;
  }
}

async function exportAll(): Promise<void> {
  await exportJobs({ ...query.value });
}
</script>

<template>
  <AppPage
    :title="title"
    :description="'调度状态与「立即执行」是两件事：前者持久生效，后者只跑一次。'"
  >
    <template #actions>
      <el-button v-perm="'monitor:job:export'" @click="exportAll">{{
        t('common.export')
      }}</el-button>
      <el-button v-perm="'monitor:job:add'" type="primary" @click="openCreate">
        {{ t('common.add') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="任务名称">
          <el-input v-model="query.job_name" clearable placeholder="任务名称" />
        </el-form-item>
        <el-form-item label="任务分组">
          <el-input v-model="query.job_group" clearable placeholder="如 DEFAULT" />
        </el-form-item>
        <el-form-item :label="t('common.status')">
          <el-select v-model="query.status" clearable placeholder="全部" style="width: 130px">
            <el-option
              v-for="item in STATUS_OPTIONS"
              :key="item.value"
              :label="item.value === 0 ? '正常' : '暂停'"
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
      row-key="job_id"
      empty-title="还没有定时任务"
      empty-hint="任务要先在后端注册调用目标，再在这里配置调度。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="job_name" label="任务名称" min-width="160" />
      <el-table-column prop="job_group" label="分组" min-width="110" />
      <el-table-column label="调用目标" min-width="200">
        <template #default="{ row }">
          <code class="job__code">{{ row.invoke_target }}</code>
        </template>
      </el-table-column>
      <el-table-column label="Cron" min-width="150">
        <template #default="{ row }">
          <code class="job__code">{{ row.cron_expression }}</code>
        </template>
      </el-table-column>
      <el-table-column label="并发" width="100">
        <template #default="{ row }">
          {{ row.concurrency === 0 ? '允许' : '禁止' }}
        </template>
      </el-table-column>
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }">
          <AppStatusTag :value="row.status" enabled-text="正常" disabled-text="暂停" />
        </template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="280" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'monitor:job:edit'" link type="primary" @click="runOnce(row)">
            执行一次
          </el-button>
          <el-button v-perm="'monitor:job:edit'" link type="warning" @click="toggleStatus(row)">
            {{ row.status === 0 ? '暂停' : '恢复' }}
          </el-button>
          <el-button v-perm="'monitor:job:detail'" link @click="openLogs(row)">日志</el-button>
          <el-button v-perm="'monitor:job:edit'" link type="primary" @click="openEdit(row)">
            {{ t('common.edit') }}
          </el-button>
          <el-button v-perm="'monitor:job:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'monitor:job:add'" type="primary" @click="openCreate">
          {{ t('common.add') }}
        </el-button>
      </template>
    </AppTable>

    <AppDialog
      v-model="dialogVisible"
      :title="editingId === null ? '新增任务' : '修改任务'"
      width="720px"
      :confirm-loading="saving"
      @confirm="submit"
    >
      <AppForm ref="formRef" :model="form" :fields="fields" :columns="2" />
    </AppDialog>

    <AppDrawer v-model="logDrawer.visible" :title="`执行日志：${logDrawer.jobName}`" size="720px">
      <div class="job__logs">
        <el-button :loading="logLoading" @click="loadLogs">{{ t('common.refresh') }}</el-button>
        <el-table v-loading="logLoading" :data="logs" border>
          <el-table-column label="开始时间" width="170">
            <template #default="{ row }">{{ formatDateTime(row.start_time) }}</template>
          </el-table-column>
          <el-table-column label="耗时" width="100">
            <template #default="{ row }">{{ formatDuration(row.duration_ms) }}</template>
          </el-table-column>
          <el-table-column label="触发方式" width="100">
            <template #default="{ row }">{{ row.trigger_kind === 2 ? '手动' : '定时' }}</template>
          </el-table-column>
          <el-table-column label="结果" width="90">
            <template #default="{ row }">
              <AppStatusTag :value="row.status" variant="outcome" />
            </template>
          </el-table-column>
          <el-table-column label="输出" min-width="220">
            <template #default="{ row }">{{ orPlaceholder(row.result) }}</template>
          </el-table-column>
          <el-table-column prop="instance_id" label="实例" min-width="140" show-overflow-tooltip />
        </el-table>
      </div>
    </AppDrawer>
  </AppPage>
</template>

<style scoped>
.job__code {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}

.job__logs {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  align-items: flex-start;
}
</style>
