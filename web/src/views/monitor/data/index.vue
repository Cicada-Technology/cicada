<script setup lang="ts">
/**
 * 数据监控（`monitor:data:*`）。
 *
 * 只读 PostgreSQL 系统目录与统计视图：库大小、连接数、连接池、表行数与扫描次数。
 * 刻意不做任意 SQL 控制台，也不把连接串、查询文本或凭据带到前端。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import { getDataMonitor, type DataMonitorVo } from '@/api/monitor/data';
import AppPage from '@/components/AppPage.vue';
import AppTable from '@/components/AppTable.vue';
import {
  formatBytes,
  formatDateTime,
  formatNumber,
  formatSeconds,
  orPlaceholder,
} from '@/utils/format';

const { t } = useI18n();
const route = useRoute();

const title = computed(() => route.meta.title ?? '数据监控');
const loading = ref(false);
const data = ref<DataMonitorVo | null>(null);
const collectedAt = ref('');
const autoRefresh = ref(false);

const REFRESH_MS = 10_000;
let timer: ReturnType<typeof setInterval> | null = null;

async function load(): Promise<void> {
  loading.value = true;
  try {
    data.value = await getDataMonitor();
    collectedAt.value = new Date().toLocaleTimeString('zh-CN');
  } finally {
    loading.value = false;
  }
}

function watchAutoRefresh(enabled: boolean): void {
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
  if (enabled) timer = setInterval(load, REFRESH_MS);
}

const database = computed(() => data.value?.database);
const pool = computed(() => data.value?.pool);

const facts = computed(() => {
  const db = database.value;
  if (!db) return [];
  return [
    { label: '数据库', value: db.name },
    { label: '当前 schema', value: db.schema },
    { label: '数据库大小', value: formatBytes(db.size_bytes) },
    { label: '用户表数量', value: formatNumber(db.table_count) },
    { label: 'PostgreSQL 启动于', value: formatDateTime(db.started_at) },
    { label: '已运行', value: formatSeconds(db.uptime_seconds) },
    { label: '版本', value: db.version },
  ];
});

const poolPercent = computed(() => {
  const p = pool.value;
  if (!p || p.max <= 0) return 0;
  return Math.min(100, Number(((p.total / p.max) * 100).toFixed(1)));
});

const connectionPercent = computed(() => {
  const db = database.value;
  if (!db || db.max_connections <= 0) return 0;
  return Math.min(100, Number(((db.connections / db.max_connections) * 100).toFixed(1)));
});

const rows = computed(() =>
  (data.value?.tables ?? []).map((item) => ({
    ...item,
    key: `${item.schema}.${item.table}`,
  })),
);

onMounted(() => {
  void load();
});

onUnmounted(() => {
  if (timer !== null) clearInterval(timer);
});
</script>

<template>
  <AppPage
    :title="title"
    :description="'只读的 PostgreSQL 库与表统计：库大小、连接池、行数、扫描次数与清理时间。'"
  >
    <template #actions>
      <el-switch v-model="autoRefresh" :active-text="'自动刷新'" @change="watchAutoRefresh" />
      <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
    </template>

    <section v-loading="loading" class="data__summary">
      <div class="data__card ci-card">
        <h2 class="data__section-title">数据库</h2>
        <dl class="data__facts">
          <template v-for="row in facts" :key="row.label">
            <dt>{{ row.label }}</dt>
            <dd :class="{ data__mono: row.label === '版本' }">{{ orPlaceholder(row.value) }}</dd>
          </template>
        </dl>
        <p v-if="collectedAt" class="data__stamp">采集于 {{ collectedAt }}</p>
      </div>

      <div class="data__card ci-card">
        <h2 class="data__section-title">连接</h2>
        <div class="data__metric">
          <div class="data__metric-head">
            <span>应用连接池</span>
            <span>{{ pool?.total ?? 0 }} / {{ pool?.max ?? 0 }}</span>
          </div>
          <el-progress
            :percentage="poolPercent"
            :stroke-width="10"
            :show-text="false"
            :color="poolPercent >= 85 ? 'var(--ci-color-danger)' : undefined"
          />
          <p class="data__hint">空闲 {{ pool?.idle ?? 0 }} 条</p>
        </div>
        <div class="data__metric">
          <div class="data__metric-head">
            <span>数据库会话</span>
            <span>{{ database?.connections ?? 0 }} / {{ database?.max_connections ?? 0 }}</span>
          </div>
          <el-progress
            :percentage="connectionPercent"
            :stroke-width="10"
            :show-text="false"
            :color="connectionPercent >= 85 ? 'var(--ci-color-danger)' : undefined"
          />
          <p class="data__hint">
            <code class="data__mono">pg_stat_activity</code> 中当前数据库的全部会话。
          </p>
        </div>
      </div>
    </section>

    <section class="data__tables">
      <header class="data__section-head">
        <h2 class="data__section-title">表统计</h2>
        <span class="data__hint">按总大小倒序，最多 100 张；不含系统表与分区子表。</span>
      </header>

      <AppTable
        :rows="rows"
        :loading="loading"
        :total="null"
        row-key="key"
        empty-title="没有可统计的表"
        empty-hint="当前 schema 下还没有用户表，或当前账号没有读取系统目录的权限。"
      >
        <el-table-column label="表" min-width="220" show-overflow-tooltip>
          <template #default="{ row }">
            <code class="data__mono">{{ row.schema }}.{{ row.table }}</code>
          </template>
        </el-table-column>
        <el-table-column label="行数（估算）" width="130" align="right">
          <template #default="{ row }">{{ formatNumber(row.row_count) }}</template>
        </el-table-column>
        <el-table-column label="表大小" width="110" align="right">
          <template #default="{ row }">{{ formatBytes(row.table_bytes) }}</template>
        </el-table-column>
        <el-table-column label="索引" width="110" align="right">
          <template #default="{ row }">{{ formatBytes(row.index_bytes) }}</template>
        </el-table-column>
        <el-table-column label="总计" width="110" align="right">
          <template #default="{ row }">{{ formatBytes(row.total_bytes) }}</template>
        </el-table-column>
        <el-table-column label="顺序扫描" width="120" align="right">
          <template #default="{ row }">{{ formatNumber(row.seq_scan) }}</template>
        </el-table-column>
        <el-table-column label="索引扫描" width="120" align="right">
          <template #default="{ row }">{{ formatNumber(row.idx_scan) }}</template>
        </el-table-column>
        <el-table-column label="死行" width="100" align="right">
          <template #default="{ row }">{{ formatNumber(row.dead_rows) }}</template>
        </el-table-column>
        <el-table-column label="最后 vacuum" width="170">
          <template #default="{ row }">{{ formatDateTime(row.last_vacuum) }}</template>
        </el-table-column>
        <el-table-column label="最后 analyze" width="170">
          <template #default="{ row }">{{ formatDateTime(row.last_analyze) }}</template>
        </el-table-column>
      </AppTable>
    </section>
  </AppPage>
</template>

<style scoped>
.data__summary {
  display: grid;
  grid-template-columns: minmax(0, 1.5fr) minmax(280px, 1fr);
  gap: var(--ci-space-4);
}

@media (max-width: 1100px) {
  .data__summary {
    grid-template-columns: 1fr;
  }
}

.data__card {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  padding: var(--ci-space-5);
}

.data__section-title {
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-semibold);
  color: var(--ci-color-text);
}

.data__section-head {
  display: flex;
  align-items: baseline;
  gap: var(--ci-space-3);
  margin-bottom: var(--ci-space-3);
}

.data__facts {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: var(--ci-space-2) var(--ci-space-5);
  font-size: var(--ci-text-sm);
}

.data__facts dt {
  color: var(--ci-color-text-tertiary);
}

.data__facts dd {
  margin: 0;
  color: var(--ci-color-text);
  overflow-wrap: anywhere;
}

.data__metric {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-2);
}

.data__metric-head {
  display: flex;
  justify-content: space-between;
  font-size: var(--ci-text-sm);
  color: var(--ci-color-text-secondary);
}

.data__hint,
.data__stamp {
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.data__mono {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
}

.data__tables {
  margin-top: var(--ci-space-6);
}
</style>
