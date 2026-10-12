<script setup lang="ts">
/**
 * 服务监控（`monitor:server:*`）。
 *
 * 取向与缓存监控一致：只呈现后端**专用指标接口**给的事实，不做内嵌控制台，也不在前端
 * 二次采样估算——`cpu_usage` / `mem_used_bytes` 都来自后端一次采集，刷新即重新采集。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import { getServerInfo, type ServerInfoVo } from '@/api/monitor/server';
import AppPage from '@/components/AppPage.vue';
import AppTable from '@/components/AppTable.vue';
import {
  formatBytes,
  formatNumber,
  formatSeconds,
  formatUnixSeconds,
  orPlaceholder,
} from '@/utils/format';

const { t } = useI18n();
const route = useRoute();

const title = computed(() => route.meta.title ?? '');
const loading = ref(false);
const info = ref<ServerInfoVo | null>(null);
const collectedAt = ref('');
const autoRefresh = ref(false);

/** 采集间隔：后台页常见的 5s，够看清趋势又不至于把指标接口打成热点。 */
const REFRESH_MS = 5000;
let timer: ReturnType<typeof setInterval> | null = null;

async function load(): Promise<void> {
  loading.value = true;
  try {
    info.value = await getServerInfo();
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

const facts = computed(() => {
  const data = info.value;
  if (!data) return [];
  return [
    { label: '主机名', value: data.hostname },
    { label: '操作系统', value: `${data.os_name} ${data.os_version}`.trim() },
    { label: '架构', value: data.arch },
    { label: '应用版本', value: data.version },
    { label: '构建 commit', value: data.build_commit },
    { label: '进程 PID', value: formatNumber(data.pid) },
    { label: '进程启动', value: formatUnixSeconds(data.process_start_unix_secs) },
    { label: '进程运行时长', value: formatSeconds(data.process_uptime_secs) },
    { label: '系统运行时长', value: formatSeconds(data.system_uptime_secs) },
  ];
});

/** 用量比率（0–1）→ 百分比；后端字段是 0–100 的（`cpu_usage`）不要再乘一次。 */
function toPercent(ratio: number): number {
  return Math.min(100, Math.max(0, Number((ratio * 100).toFixed(1))));
}

const cpuPercent = computed(() =>
  Math.min(100, Math.max(0, Number((info.value?.cpu_usage ?? 0).toFixed(1)))),
);
const memPercent = computed(() => {
  const data = info.value;
  if (!data || data.mem_total_bytes === 0) return 0;
  return toPercent(data.mem_used_bytes / data.mem_total_bytes);
});
const swapPercent = computed(() => {
  const data = info.value;
  if (!data || data.swap_total_bytes === 0) return 0;
  return toPercent(data.swap_used_bytes / data.swap_total_bytes);
});

function diskPercent(disk: { total_bytes: number; used_bytes: number }): number {
  if (disk.total_bytes === 0) return 0;
  return toPercent(disk.used_bytes / disk.total_bytes);
}

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
    :description="'后端一次采集的机器与进程指标：CPU、内存、磁盘、运行时长与构建信息。'"
  >
    <template #actions>
      <el-switch v-model="autoRefresh" :active-text="'自动刷新'" @change="watchAutoRefresh" />
      <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
    </template>

    <section v-loading="loading" class="server__card ci-card">
      <h2 class="server__section-title">运行环境</h2>
      <dl class="server__facts">
        <template v-for="row in facts" :key="row.label">
          <dt>{{ row.label }}</dt>
          <dd>
            <code v-if="row.label === '构建 commit'" class="server__code">{{ row.value }}</code>
            <span v-else>{{ orPlaceholder(row.value) }}</span>
          </dd>
        </template>
      </dl>
      <p v-if="collectedAt" class="server__stamp">采集于 {{ collectedAt }}</p>
    </section>

    <section v-loading="loading" class="server__gauges">
      <div class="server__card ci-card">
        <h2 class="server__section-title">CPU</h2>
        <el-progress
          type="dashboard"
          :percentage="cpuPercent"
          :color="cpuPercent >= 85 ? 'var(--ci-color-danger)' : undefined"
        />
        <dl class="server__facts server__facts--single">
          <dt>型号</dt>
          <dd>{{ orPlaceholder(info?.cpu_brand) }}</dd>
          <dt>核心数</dt>
          <dd>{{ orPlaceholder(info?.cpu_cores) }}</dd>
          <dt>负载 1 / 5 / 15 分钟</dt>
          <dd>
            {{ info?.load_avg_1m.toFixed(2) }} / {{ info?.load_avg_5m.toFixed(2) }} /
            {{ info?.load_avg_15m.toFixed(2) }}
          </dd>
        </dl>
      </div>

      <div class="server__card ci-card">
        <h2 class="server__section-title">内存</h2>
        <div class="server__bar">
          <el-progress :percentage="memPercent" :stroke-width="10" :show-text="false" />
          <p class="server__hint">
            {{ formatBytes(info?.mem_used_bytes) }} / {{ formatBytes(info?.mem_total_bytes) }}
          </p>
        </div>
        <h3 class="server__sub-title">Swap</h3>
        <div class="server__bar">
          <el-progress
            :percentage="swapPercent"
            :stroke-width="10"
            :show-text="false"
            :color="'var(--ci-gray-400)'"
          />
          <p class="server__hint">
            {{ formatBytes(info?.swap_used_bytes) }} /
            {{ info?.swap_total_bytes ? formatBytes(info.swap_total_bytes) : '未启用' }}
          </p>
        </div>
      </div>
    </section>

    <section class="server__disks">
      <header class="server__section-head">
        <h2 class="server__section-title">磁盘</h2>
        <span class="server__hint">按挂载点列举，使用率超过 85% 标红。</span>
      </header>

      <AppTable
        :rows="info?.disks ?? []"
        :loading="loading"
        :total="null"
        row-key="mount_point"
        empty-title="没有磁盘采样"
        empty-hint="后端读不到 /proc/mounts 时磁盘区会为空。"
      >
        <el-table-column prop="name" label="设备" min-width="180" show-overflow-tooltip>
          <template #default="{ row }">
            <code class="server__code">{{ row.name }}</code>
          </template>
        </el-table-column>
        <el-table-column prop="mount_point" label="挂载点" min-width="160" />
        <el-table-column prop="fs_type" label="文件系统" width="120" />
        <el-table-column label="总量" width="120">
          <template #default="{ row }">{{ formatBytes(row.total_bytes) }}</template>
        </el-table-column>
        <el-table-column label="已用" width="120">
          <template #default="{ row }">{{ formatBytes(row.used_bytes) }}</template>
        </el-table-column>
        <el-table-column label="使用率" min-width="180">
          <template #default="{ row }">
            <el-progress
              :percentage="diskPercent(row)"
              :stroke-width="10"
              :color="diskPercent(row) >= 85 ? 'var(--ci-color-danger)' : undefined"
            />
          </template>
        </el-table-column>
      </AppTable>
    </section>
  </AppPage>
</template>

<style scoped>
.server__card {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  padding: var(--ci-space-5);
}

.server__gauges {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
  gap: var(--ci-space-4);
}

.server__section-title {
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-semibold);
  color: var(--ci-color-text);
}

.server__sub-title {
  font-size: var(--ci-text-sm);
  font-weight: var(--ci-weight-medium);
  color: var(--ci-color-text-secondary);
}

.server__section-head {
  display: flex;
  align-items: baseline;
  gap: var(--ci-space-3);
  margin-bottom: var(--ci-space-3);
}

.server__hint,
.server__stamp {
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.server__facts {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: var(--ci-space-2) var(--ci-space-5);
  font-size: var(--ci-text-sm);
}

.server__facts--single {
  grid-template-columns: 1fr;
}

.server__facts dt {
  color: var(--ci-color-text-tertiary);
}

.server__facts dd {
  margin: 0 0 var(--ci-space-1);
  color: var(--ci-color-text);
}

.server__bar {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-2);
}

.server__code {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}
</style>
