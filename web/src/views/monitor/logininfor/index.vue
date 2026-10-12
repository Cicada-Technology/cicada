<script setup lang="ts">
/**
 * 登录日志（只读，`monitor:logininfor:*`）。
 *
 * 属主是 system 域：日志表由写日志的一方维护，这张页面只是它的视图。
 * 因此除了筛选与查看，只提供两个动作——**解锁账号**与**清理**——
 * 它们分别对应「连续失败被锁」和「日志无限增长」这两个真实运维诉求。
 */
import { computed, reactive } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  cleanLoginLogs,
  deleteLoginLog,
  exportLoginLogs,
  listLoginLogs,
  unlockLogin,
  type LoginLogListQuery,
  type LoginLogVo,
} from '@/api/monitor/log';
import { STATUS_ENABLED, STATUS_OPTIONS } from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
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
const { confirm, confirmDelete } = useConfirm();

const title = computed(() => route.meta.title ?? '');

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
} = useTable<LoginLogVo, LoginLogListQuery>({ fetcher: listLoginLogs });

/** 清理默认只清「成功」记录：失败记录是排障依据，误清会让人没法复现问题。 */
const clean = reactive({ visible: false, onlySuccess: true });

async function unlock(row: LoginLogVo): Promise<void> {
  const agreed = await confirm(`确定解锁账号「${row.user_name}」吗？解锁后可立即重新尝试登录。`, {
    danger: true,
  });
  if (!agreed) return;
  try {
    await unlockLogin(row.user_name);
    notifySuccess('账号已解锁');
  } catch {
    // 提示由拦截器给出。
  }
}

async function remove(row: LoginLogVo): Promise<void> {
  if (
    !(await confirmDelete(`「${row.user_name}」于 ${formatDateTime(row.login_time)} 的登录记录`))
  ) {
    return;
  }
  try {
    await deleteLoginLog(row.info_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

async function submitClean(): Promise<void> {
  const agreed = await confirm(
    clean.onlySuccess
      ? '确定清空全部**成功**登录日志吗？'
      : '确定清空全部登录日志吗？此操作不可撤销。',
    { danger: true },
  );
  if (!agreed) return;
  try {
    await cleanLoginLogs(clean.onlySuccess ? { status: STATUS_ENABLED } : {});
    clean.visible = false;
    notifySuccess('日志已清理');
    await load();
  } catch {
    // 同上。
  }
}

async function exportAll(): Promise<void> {
  await exportLoginLogs({ ...query.value });
}
</script>

<template>
  <AppPage :title="title" :description="'登录成功与失败记录；失败次数过多会触发账号锁定。'">
    <template #actions>
      <el-button v-perm="'monitor:logininfor:remove'" @click="clean.visible = true">
        清理
      </el-button>
      <el-button v-perm="'monitor:logininfor:export'" @click="exportAll">
        {{ t('common.export') }}
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="账号">
          <el-input v-model="query.user_name" clearable placeholder="登录名" />
        </el-form-item>
        <el-form-item label="IP">
          <el-input v-model="query.ipaddr" clearable placeholder="如 10.0.0." />
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
      row-key="info_id"
      empty-title="没有登录记录"
      empty-hint="调整筛选条件，或确认日志采集是否开启。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="user_name" label="账号" min-width="130" />
      <el-table-column prop="ipaddr" label="IP" min-width="130" />
      <el-table-column prop="login_location" label="归属地" min-width="150" show-overflow-tooltip />
      <el-table-column prop="browser" label="浏览器" min-width="140" show-overflow-tooltip />
      <el-table-column prop="os" label="操作系统" min-width="130" show-overflow-tooltip />
      <el-table-column label="结果" width="90">
        <template #default="{ row }">
          <AppStatusTag :value="row.status" variant="outcome" />
        </template>
      </el-table-column>
      <el-table-column prop="msg" label="消息" min-width="180" show-overflow-tooltip />
      <el-table-column label="登录时间" width="170">
        <template #default="{ row }">{{ formatDateTime(row.login_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="180" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'monitor:logininfor:unlock'" link type="warning" @click="unlock(row)">
            解锁
          </el-button>
          <el-button v-perm="'monitor:logininfor:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>
    </AppTable>

    <AppDialog
      v-model="clean.visible"
      title="清理登录日志"
      width="440px"
      confirm-type="danger"
      @confirm="submitClean"
    >
      <el-form label-width="110px" @submit.prevent>
        <el-form-item label="仅清成功记录">
          <el-switch v-model="clean.onlySuccess" />
        </el-form-item>
      </el-form>
      <p class="logininfor__hint">
        失败记录是排查暴力破解与配置错误的依据，默认保留；只在存储告急时清空全部。
      </p>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.logininfor__hint {
  font-size: var(--ci-text-xs);
  line-height: var(--ci-leading-snug);
  color: var(--ci-color-text-tertiary);
}
</style>
