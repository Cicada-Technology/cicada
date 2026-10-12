<script setup lang="ts">
/**
 * 在线用户（`monitor:online:list`）。
 *
 * 会话存在 Redis 里，所以「强退」是删会话而不是改用户状态：对端下一次请求就会 401
 * 并被弹出登录页。这里因此有两个必须做的界面义务：
 * - 强退前确认，并在文案里带上**账号 + IP**（同名账号可能多处登录，删错会话用户找不出原因）。
 * - 时间用 `formatUnixSeconds`：后端给的是 Unix 秒，不是 RFC3339。
 */
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  batchForceLogout,
  forceLogout,
  listOnlineUsers,
  type OnlineUserListQuery,
  type OnlineUserVo,
} from '@/api/monitor/online';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { notifySuccess } from '@/utils/feedback';
import { formatUnixSeconds } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirm } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const selection = ref<OnlineUserVo[]>([]);

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
} = useTable<OnlineUserVo, OnlineUserListQuery>({ fetcher: listOnlineUsers });

async function kickOne(row: OnlineUserVo): Promise<void> {
  const agreed = await confirm(`确定强退「${row.user_name}」（${row.ip}）吗？该会话会立即失效。`, {
    danger: true,
  });
  if (!agreed) return;
  try {
    await forceLogout(row.session_id);
    notifySuccess('已强退该会话');
    await load();
  } catch {
    // 会话可能已过期自行消失，提示由拦截器给出。
  }
}

async function kickSelected(): Promise<void> {
  if (selection.value.length === 0) return;
  const agreed = await confirm(
    `确定强退选中的 ${selection.value.length} 个会话吗？对应用户会立即掉线。`,
    { danger: true },
  );
  if (!agreed) return;
  try {
    await batchForceLogout(selection.value.map((item) => item.session_id));
    selection.value = [];
    notifySuccess('已批量强退');
    await load();
  } catch {
    // 同上。
  }
}
</script>

<template>
  <AppPage :title="title" :description="'当前活跃会话；强退后对端下一次请求即被登出。'">
    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="账号">
          <el-input v-model="query.user_name" clearable placeholder="登录名" />
        </el-form-item>
        <el-form-item label="IP">
          <el-input v-model="query.ip" clearable placeholder="如 10.0.0." />
        </el-form-item>
      </AppSearchForm>
    </template>

    <AppTable
      v-model:selection="selection"
      :rows="rows"
      :loading="loading"
      :total="total"
      :page="page"
      :page-size="pageSize"
      row-key="session_id"
      selectable
      empty-title="没有在线会话"
      empty-hint="没人登录，或者都被筛掉了。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="user_name" label="账号" min-width="140" />
      <el-table-column prop="ip" label="IP" min-width="140" />
      <el-table-column prop="user_agent" label="客户端" min-width="260" show-overflow-tooltip />
      <el-table-column label="登录时间" width="170">
        <template #default="{ row }">{{ formatUnixSeconds(row.login_time) }}</template>
      </el-table-column>
      <el-table-column label="最近活动" width="170">
        <template #default="{ row }">{{ formatUnixSeconds(row.last_seen) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="110" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'monitor:online:list'" link type="danger" @click="kickOne(row)">
            强退
          </el-button>
        </template>
      </el-table-column>

      <template #bulk="{ selection: selected }">
        <el-button v-perm="'monitor:online:list'" type="danger" @click="kickSelected">
          强退选中 {{ selected.length }} 个
        </el-button>
      </template>
    </AppTable>
  </AppPage>
</template>
