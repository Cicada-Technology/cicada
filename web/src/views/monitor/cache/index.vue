<script setup lang="ts">
/**
 * 缓存监控（`monitor:cache:*`）。
 *
 * 这一版的取向是「先看得懂，再动手」：
 * - 概览区把 Redis 的自身指标与**域**（key 前缀，由 infra 层定义）分开展示。清缓存必须
 *   按域来，直接给一个「清空所有」按钮在共享 Redis 上是灾难。
 * - 敏感域（会话、令牌）在界面上单独标红，并在清空前二次确认里点明影响面。
 * - 键列表是 `SCAN` 游标语义，不是页码：所以用「加载更多」而不是分页器，避免用户
 *   误以为能跳页。
 */
import { computed, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  cacheOverview,
  clearCacheKeys,
  listCacheKeys,
  type CacheInfoVo,
  type CacheKeyPage,
  type DomainStat,
} from '@/api/monitor/cache';
import AppEmpty from '@/components/AppEmpty.vue';
import AppPage from '@/components/AppPage.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { notifySuccess, notifyWarning } from '@/utils/feedback';
import { formatNumber, formatSeconds } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirm } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const loading = ref(false);
const info = ref<CacheInfoVo | null>(null);
const domains = ref<DomainStat[]>([]);
const scannedAt = ref('');
const selectedDomain = ref<DomainStat | null>(null);

const keys = ref<string[]>([]);
const cursor = ref(0);
const hasMore = ref(false);
const keysLoading = ref(false);

async function loadOverview(): Promise<void> {
  loading.value = true;
  try {
    const data = await cacheOverview();
    info.value = data.info;
    domains.value = data.domains;
    scannedAt.value = data.scanned_at;
  } finally {
    loading.value = false;
  }
}

/** 打开某个域：先取第一页键。 */
async function openDomain(domain: DomainStat): Promise<void> {
  selectedDomain.value = domain;
  keys.value = [];
  cursor.value = 0;
  await loadMoreKeys();
}

async function loadMoreKeys(): Promise<void> {
  if (!selectedDomain.value) return;
  keysLoading.value = true;
  try {
    const page: CacheKeyPage = await listCacheKeys(selectedDomain.value.name, cursor.value);
    // 游标式分页：续页要追加，不是替换。
    keys.value = [...keys.value, ...page.keys];
    cursor.value = page.cursor;
    hasMore.value = page.has_more;
  } finally {
    keysLoading.value = false;
  }
}

/** 清空前把影响面写进确认文案：敏感域额外提示。 */
async function clearDomain(domain: DomainStat): Promise<void> {
  const warning = domain.sensitive
    ? `「${domain.description}」是敏感域：清空会让对应用户立即掉线或需要重新获取令牌。`
    : `「${domain.description}」清空后会按需从数据库重建。`;
  const agreed = await confirm(
    `确定清空域「${domain.name}」（${domain.keys} 个键）吗？${warning}`,
    { danger: true },
  );
  if (!agreed) return;
  try {
    const result = await clearCacheKeys(domain.name);
    notifySuccess(`已清除 ${result.removed} 个键`);
    if (selectedDomain.value?.name === domain.name) {
      keys.value = [];
      cursor.value = 0;
      hasMore.value = false;
    }
    await loadOverview();
  } catch {
    // 提示由拦截器给出。
  }
}

async function removeKey(key: string): Promise<void> {
  if (!selectedDomain.value) return;
  const agreed = await confirm(`确定删除键「${key}」吗？`, { danger: true });
  if (!agreed) return;
  try {
    const result = await clearCacheKeys(selectedDomain.value.name, key);
    if (result.removed === 0) {
      notifyWarning('该键已不存在（可能刚过期）');
    } else {
      notifySuccess('已删除该键');
    }
    keys.value = keys.value.filter((item) => item !== key);
    await loadOverview();
  } catch {
    // 同上。
  }
}

const infoRows = computed(() => {
  const data = info.value;
  if (!data) return [];
  return [
    { label: 'Redis 版本', value: data.version },
    { label: '角色', value: data.role },
    { label: '运行时长', value: formatSeconds(data.uptime_secs) },
    { label: '连接数', value: formatNumber(data.connected_clients) },
    { label: '已用内存', value: `${data.used_memory_human} / ${formatNumber(data.maxmemory)}` },
    { label: '键总数', value: formatNumber(data.db_keys) },
    { label: '命中 / 未命中', value: `${formatNumber(data.hits)} / ${formatNumber(data.misses)}` },
    { label: '命中率', value: `${(data.hit_rate * 100).toFixed(1)}%` },
    { label: '过期键', value: formatNumber(data.expired_keys) },
    { label: '淘汰键', value: formatNumber(data.evicted_keys) },
  ];
});

onMounted(loadOverview);
</script>

<template>
  <AppPage :title="title" :description="'Redis 自身指标与按域组织的缓存键；清缓存只能按域进行。'">
    <template #actions>
      <el-button :loading="loading" @click="loadOverview">{{ t('common.refresh') }}</el-button>
    </template>

    <section v-loading="loading" class="cache__info ci-card">
      <h2 class="cache__section-title">实例信息</h2>
      <dl class="cache__facts">
        <template v-for="row in infoRows" :key="row.label">
          <dt>{{ row.label }}</dt>
          <dd>{{ row.value }}</dd>
        </template>
      </dl>
      <p v-if="scannedAt" class="cache__stamp">采集于 {{ scannedAt }}</p>
    </section>

    <section class="cache__domains">
      <header class="cache__section-head">
        <h2 class="cache__section-title">域</h2>
        <span class="cache__hint">点域标识查看键；清空按域生效。</span>
      </header>

      <AppTable
        :rows="domains"
        :loading="loading"
        :total="null"
        row-key="name"
        empty-title="没有可统计的域"
        empty-hint="缓存域由 infra 层注册；没有注册说明该模块还没接入缓存。"
      >
        <el-table-column label="域" min-width="160">
          <template #default="{ row }">
            <el-button link type="primary" @click="openDomain(row as DomainStat)">
              {{ row.name }}
            </el-button>
          </template>
        </el-table-column>
        <el-table-column prop="prefix" label="前缀" min-width="180">
          <template #default="{ row }">
            <code class="cache__code">{{ row.prefix }}</code>
          </template>
        </el-table-column>
        <el-table-column prop="description" label="说明" min-width="200" show-overflow-tooltip />
        <el-table-column label="键数" width="110">
          <template #default="{ row }"> {{ row.keys }}{{ row.truncated ? '+' : '' }} </template>
        </el-table-column>
        <el-table-column label="敏感" width="90">
          <template #default="{ row }">
            <el-tag v-if="row.sensitive" type="danger" effect="light" round size="small"
              >敏感</el-tag
            >
            <span v-else class="ci-tertiary">—</span>
          </template>
        </el-table-column>
        <el-table-column :label="t('common.actions')" width="110" fixed="right">
          <template #default="{ row }">
            <el-button v-perm="'monitor:cache:clear'" link type="danger" @click="clearDomain(row)">
              清空
            </el-button>
          </template>
        </el-table-column>
      </AppTable>
    </section>

    <section class="cache__keys ci-card">
      <header class="cache__section-head">
        <h2 class="cache__section-title">
          键
          <span v-if="selectedDomain" class="cache__hint">域 {{ selectedDomain.name }}</span>
        </h2>
        <el-button
          v-if="selectedDomain"
          v-perm="'monitor:cache:clear'"
          type="danger"
          plain
          @click="clearDomain(selectedDomain)"
        >
          清空该域
        </el-button>
      </header>

      <AppEmpty
        v-if="!selectedDomain"
        icon="Key"
        title="先选一个域"
        hint="键数量可能很大，只有选中域之后才会按游标分批扫描。"
      />

      <template v-else>
        <div v-loading="keysLoading" class="cache__key-list">
          <div v-for="key in keys" :key="key" class="cache__key">
            <code class="cache__code">{{ key }}</code>
            <el-button v-perm="'monitor:cache:clear'" link type="danger" @click="removeKey(key)">
              删除
            </el-button>
          </div>
          <AppEmpty v-if="keys.length === 0 && !keysLoading" title="该域下没有键" />
        </div>

        <div class="cache__key-footer">
          <span class="cache__hint">已加载 {{ keys.length }} 个键（游标分页，非页码）</span>
          <el-button v-if="hasMore" :loading="keysLoading" @click="loadMoreKeys">
            加载更多
          </el-button>
          <span v-else class="cache__hint">已到底</span>
        </div>
      </template>
    </section>
  </AppPage>
</template>

<style scoped>
.cache__info {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  padding: var(--ci-space-5);
}

.cache__section-title {
  display: flex;
  align-items: baseline;
  gap: var(--ci-space-2);
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-semibold);
  color: var(--ci-color-text);
}

.cache__section-head {
  display: flex;
  align-items: center;
  gap: var(--ci-space-3);
  margin-bottom: var(--ci-space-3);
}

.cache__section-head > .el-button {
  margin-left: auto;
}

.cache__hint {
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.cache__facts {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: var(--ci-space-2) var(--ci-space-5);
  font-size: var(--ci-text-sm);
}

.cache__facts dt {
  color: var(--ci-color-text-tertiary);
}

.cache__facts dd {
  margin: 0 0 var(--ci-space-1);
  color: var(--ci-color-text);
}

.cache__stamp {
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.cache__keys {
  padding: var(--ci-space-5);
}

.cache__key-list {
  display: flex;
  flex-direction: column;
  max-height: 320px;
  overflow: auto;
}

.cache__key {
  display: flex;
  align-items: center;
  gap: var(--ci-space-3);
  padding: var(--ci-space-1) 0;
  border-bottom: 1px solid var(--ci-color-separator);
}

.cache__key:last-child {
  border-bottom: none;
}

.cache__key .cache__code {
  flex: 1 1 auto;
  min-width: 0;
  overflow-wrap: anywhere;
}

.cache__key-footer {
  display: flex;
  align-items: center;
  gap: var(--ci-space-3);
  margin-top: var(--ci-space-3);
}

.cache__code {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}
</style>
