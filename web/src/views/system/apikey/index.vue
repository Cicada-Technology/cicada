<script setup lang="ts">
/**
 * 开放接口凭证（`system:apikey:*`， / ADR 0004）。
 *
 * 三条界面上的硬约束来自后端语义：
 * - 明文密钥只在**签发响应**里出现一次，列表只有 `key_prefix`；所以签发成功后必须立刻
 *   用一次性弹层把它交给用户，关掉就找不回来了（只能重签）。
 * - 凭证主体是租户级的服务身份（`user_id = 0`，不是自然人），故停用/删除都是即时生效，
 *   没有会话可踢。
 * - 权限标识是精确匹配的 `模块:资源:操作`，写错的 key 不会报错，只会一直 403。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  createApiKey,
  deleteApiKey,
  listApiKeys,
  updateApiKeyStatus,
  type ApiKeyCreate,
  type ApiKeyIssuedVo,
  type ApiKeyListQuery,
  type ApiKeyVo,
} from '@/api/system/platform';
import { STATUS_DISABLED, STATUS_ENABLED, STATUS_OPTIONS } from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppStatusTag from '@/components/AppStatusTag.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { notifyError, notifySuccess, notifyWarning } from '@/utils/feedback';
import { formatDateTime, orPlaceholder } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirm, confirmDelete } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const formRef = ref<InstanceType<typeof AppForm>>();
const dialogVisible = ref(false);
const saving = ref(false);

const issuedVisible = ref(false);
const issued = ref<ApiKeyIssuedVo | null>(null);

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
} = useTable<ApiKeyVo, ApiKeyListQuery>({ fetcher: listApiKeys });

const form = reactive({
  name: '',
  scopes: [] as string[],
  expires_at: '' as string,
  remark: '',
});

/** 与后端 `valid_scope` 同形：小写字母/数字/下划线，至少两段，冒号分隔。 */
const SCOPE_PATTERN = /^[a-z0-9_]+(:[a-z0-9_]+)+$/;

const fields = computed<AppFormField[]>(() => [
  { prop: 'name', label: '凭证名称', required: true, span: 24 },
  { prop: 'scopes', label: '权限标识', type: 'slot', span: 24 },
  {
    prop: 'expires_at',
    label: '过期时间',
    type: 'datetime',
    clearable: true,
    tips: '留空表示不过期。',
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea', span: 24 },
]);

function resetForm(): void {
  Object.assign(form, { name: '', scopes: [], expires_at: '', remark: '' });
  formRef.value?.clearValidate();
}

function openCreate(): void {
  resetForm();
  dialogVisible.value = true;
}

async function submit(): Promise<void> {
  if (!(await formRef.value?.validate())) return;
  const bad = form.scopes.find((item) => !SCOPE_PATTERN.test(item));
  if (bad) {
    notifyError(`权限标识非法：${bad}（需形如 system:user:list，小写、冒号分段）`);
    return;
  }
  const payload: ApiKeyCreate = {
    name: form.name,
    scopes: form.scopes,
    expires_at: form.expires_at || null,
    remark: form.remark || null,
  };
  saving.value = true;
  try {
    const data = await createApiKey(payload);
    dialogVisible.value = false;
    issued.value = data;
    issuedVisible.value = true;
    await load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    saving.value = false;
  }
}

async function copySecret(): Promise<void> {
  const secret = issued.value?.secret;
  if (!secret) return;
  try {
    await navigator.clipboard.writeText(secret);
    notifySuccess('已复制到剪贴板');
  } catch {
    // 非安全上下文（http 直连）下剪贴板不可用，选中文本让用户手动复制。
    notifyWarning('浏览器拒绝了剪贴板访问，请手动选中密钥复制');
  }
}

async function toggleStatus(row: ApiKeyVo): Promise<void> {
  const next = row.status === STATUS_ENABLED ? STATUS_DISABLED : STATUS_ENABLED;
  const agreed = await (next === STATUS_DISABLED
    ? confirm(`确定停用「${row.name}」吗？该密钥立即失效，已接入的外部系统会立刻收到 401。`, {
        danger: true,
      })
    : Promise.resolve(true));
  if (!agreed) return;
  try {
    await updateApiKeyStatus(row.id, next);
    notifySuccess(next === STATUS_ENABLED ? '已启用' : '已停用');
    await load();
  } catch {
    // 同上。
  }
}

async function remove(row: ApiKeyVo): Promise<void> {
  if (!(await confirmDelete(row.name))) return;
  try {
    await deleteApiKey(row.id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}
</script>

<template>
  <AppPage
    :title="title"
    :description="'给外部系统调用的凭证：明文密钥只在签发时返回一次，权限标识精确匹配接口权限。'"
  >
    <template #actions>
      <el-button v-perm="'system:apikey:add'" type="primary" @click="openCreate">
        签发凭证
      </el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="凭证名称">
          <el-input v-model="query.name" clearable placeholder="凭证名称" />
        </el-form-item>
        <el-form-item :label="t('common.status')">
          <el-select v-model="query.status" clearable placeholder="全部" style="width: 120px">
            <el-option
              v-for="item in STATUS_OPTIONS"
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
      empty-title="还没有开放接口凭证"
      empty-hint="凭证代表租户级的服务身份，外部系统带它调用 REST 接口。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="name" label="凭证名称" min-width="160" />
      <el-table-column label="密钥前缀" width="140">
        <template #default="{ row }">
          <code class="apikey__code">{{ row.key_prefix }}</code>
        </template>
      </el-table-column>
      <el-table-column label="权限标识" min-width="260">
        <template #default="{ row }">
          <el-tag
            v-for="scope in row.scopes"
            :key="scope"
            class="apikey__scope"
            type="info"
            effect="plain"
            size="small"
            round
          >
            {{ scope }}
          </el-tag>
          <span v-if="row.scopes.length === 0" class="ci-tertiary">仅认证，无接口权限</span>
        </template>
      </el-table-column>
      <el-table-column :label="t('common.status')" width="90">
        <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
      </el-table-column>
      <el-table-column label="过期时间" width="170">
        <template #default="{ row }">{{ formatDateTime(row.expires_at) }}</template>
      </el-table-column>
      <el-table-column label="最近使用" width="170">
        <template #default="{ row }">{{ orPlaceholder(row.last_used_at) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="180" fixed="right">
        <template #default="{ row }">
          <el-button
            v-perm="'system:apikey:edit'"
            link
            :type="row.status === 0 ? 'danger' : 'primary'"
            @click="toggleStatus(row)"
          >
            {{ row.status === 0 ? '停用' : '启用' }}
          </el-button>
          <el-button v-perm="'system:apikey:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>

      <template #empty-action>
        <el-button v-perm="'system:apikey:add'" type="primary" @click="openCreate">
          签发凭证
        </el-button>
      </template>
    </AppTable>

    <AppDialog v-model="dialogVisible" title="签发凭证" :confirm-loading="saving" @confirm="submit">
      <AppForm ref="formRef" :model="form" :fields="fields">
        <template #field:scopes>
          <el-select
            v-model="form.scopes"
            multiple
            filterable
            allow-create
            default-first-option
            :reserve-keyword="false"
            placeholder="输入权限标识后回车，如 system:user:list"
            style="width: 100%"
          />
          <p class="apikey__tips">
            精确匹配接口权限（小写、冒号分段）。留空表示该凭证只有认证、不能调任何业务接口。
          </p>
        </template>
      </AppForm>
    </AppDialog>

    <AppDialog
      v-model="issuedVisible"
      title="凭证已签发"
      confirm-text="我已保存"
      width="560px"
      @confirm="issuedVisible = false"
    >
      <div class="apikey__issued">
        <p class="apikey__issued-name">{{ issued?.name }}</p>
        <div class="apikey__secret-row">
          <code class="apikey__secret">{{ issued?.secret }}</code>
          <el-button link type="primary" @click="copySecret">复制</el-button>
        </div>
        <p class="apikey__warning">
          明文密钥只显示这一次。离开本弹层后无法再取回，只能重新签发或轮换。
        </p>
        <dl class="apikey__meta">
          <dt>前缀</dt>
          <dd>
            <code class="apikey__code">{{ issued?.key_prefix }}</code>
          </dd>
          <dt>权限标识</dt>
          <dd>{{ issued?.scopes.join('、') || '（无）' }}</dd>
          <dt>过期时间</dt>
          <dd>{{ formatDateTime(issued?.expires_at) }}</dd>
        </dl>
      </div>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.apikey__code {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}

.apikey__scope {
  margin: 0 var(--ci-space-1) var(--ci-space-1) 0;
}

.apikey__tips {
  width: 100%;
  margin-top: var(--ci-space-1);
  font-size: var(--ci-text-xs);
  line-height: var(--ci-leading-snug);
  color: var(--ci-color-text-tertiary);
}

.apikey__issued {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
}

.apikey__issued-name {
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-semibold);
  color: var(--ci-color-text);
}

.apikey__secret-row {
  display: flex;
  align-items: center;
  gap: var(--ci-space-2);
  padding: var(--ci-space-3);
  background: var(--ci-color-surface-sunken);
  border-radius: var(--ci-radius-md);
}

.apikey__secret {
  flex: 1 1 auto;
  min-width: 0;
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-sm);
  color: var(--ci-color-text);
  overflow-wrap: anywhere;
  user-select: all;
}

.apikey__warning {
  font-size: var(--ci-text-xs);
  color: var(--ci-color-danger);
}

.apikey__meta {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: var(--ci-space-2) var(--ci-space-5);
  font-size: var(--ci-text-sm);
}

.apikey__meta dt {
  color: var(--ci-color-text-tertiary);
}

.apikey__meta dd {
  margin: 0 0 var(--ci-space-1);
  color: var(--ci-color-text);
}
</style>
