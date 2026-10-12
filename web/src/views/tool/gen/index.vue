<script setup lang="ts">
/**
 * 代码生成器。
 *
 * 这条链路是「数据库表 → 生成元数据 → Rust 业务代码 + Vue 页面」，所以页面按同一步骤分三段：
 * 1. **导入**：从库里挑表（框架表、缺主键、复合主键的会被后端标成不可生成，界面直接禁用）。
 * 2. **配置**：类名/模块名/功能名与每个字段的「列表显示、查询方式、表单控件、字典绑定」。
 *    这一步决定了生成出来的页面上有什么，最值得花时间。
 * 3. **预览/下载**：预览是逐文件文本（在弹层里直接读），下载得到 zip（Rust + 前端两半）。
 *
 * 字段配置用行内控件而不是逐个弹层：配置一列要改六七个开关，弹层会把这件事变成长流程。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  deleteGenTable,
  downloadGenTable,
  downloadGenTables,
  HTML_TYPE_OPTIONS,
  importTables,
  listDbTables,
  listGenTables,
  previewGenTable,
  QUERY_TYPE_OPTIONS,
  TPL_CATEGORY_OPTIONS,
  updateGenTable,
  type DbTableVo,
  type GenColumnPatch,
  type GenColumnVo,
  type GenTableListQuery,
  type GenTableUpdate,
  type GenTableVo,
  type PreviewFile,
} from '@/api/tool/gen';
import AppDialog from '@/components/AppDialog.vue';
import AppDrawer from '@/components/AppDrawer.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { notifySuccess, notifyWarning } from '@/utils/feedback';
import { formatDateTime } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirmDelete, confirm } = useConfirm();

const title = computed(() => route.meta.title ?? '');
const selected = ref<GenTableVo[]>([]);

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
} = useTable<GenTableVo, GenTableListQuery>({ fetcher: listGenTables });

const tplLabels = new Map(TPL_CATEGORY_OPTIONS.map((item) => [item.value as string, item.label]));

// ------------------------------------------------------------------ 导入
const importer = reactive({
  visible: false,
  loading: false,
  importing: false,
  tplCategory: 'crud' as string,
});

const dbTables = ref<DbTableVo[]>([]);
const allowedPrefixes = ref<string[]>([]);
const chosenTables = ref<string[]>([]);

async function openImport(): Promise<void> {
  importer.visible = true;
  importer.loading = true;
  chosenTables.value = [];
  try {
    const data = await listDbTables();
    dbTables.value = data.items;
    allowedPrefixes.value = data.allowed_prefixes;
  } catch {
    // 提示由拦截器给出（没有库连接权限时会在这一步失败）。
  } finally {
    importer.loading = false;
  }
}

/** 勾选由表格自己维护，这里只接受「可生成且未导入」的行。 */
function onDbSelection(selection: DbTableVo[]): void {
  chosenTables.value = selection.map((item) => item.table_name);
}

async function submitImport(): Promise<void> {
  if (chosenTables.value.length === 0) {
    notifyWarning('先勾选要导入的表');
    return;
  }
  importer.importing = true;
  try {
    const imported = await importTables(chosenTables.value, importer.tplCategory);
    importer.visible = false;
    notifySuccess(`已导入 ${imported.length} 张表，接着配置字段再生成`);
    await load();
  } catch {
    // 同上。
  } finally {
    importer.importing = false;
  }
}

// ------------------------------------------------------------------ 配置
const editor = reactive({ visible: false, saving: false, tableId: 0 });
const config = reactive({
  class_name: '',
  module_name: '',
  business_name: '',
  function_name: '',
  function_author: '',
  tpl_category: 'crud',
  form_col_num: 1,
  remark: '',
});
const editColumns = ref<GenColumnVo[]>([]);

function openConfig(row: GenTableVo): void {
  editor.tableId = row.table_id;
  Object.assign(config, {
    class_name: row.class_name,
    module_name: row.module_name,
    business_name: row.business_name,
    function_name: row.function_name,
    function_author: row.function_author,
    tpl_category: row.tpl_category,
    form_col_num: row.form_col_num,
    remark: row.remark ?? '',
  });
  // 深拷贝：取消时不能污染列表里的那份数据。
  editColumns.value = row.columns.map((column) => ({ ...column }));
  editor.visible = true;
}

function toPatch(column: GenColumnVo): GenColumnPatch {
  return {
    column_name: column.column_name,
    column_comment: column.column_comment,
    is_required: column.is_required,
    is_insert: column.is_insert,
    is_edit: column.is_edit,
    is_list: column.is_list,
    is_query: column.is_query,
    query_type: column.query_type,
    html_type: column.html_type,
    dict_type: column.dict_type?.trim() ? column.dict_type : null,
  };
}

async function submitConfig(): Promise<void> {
  for (const [label, value] of [
    ['实体类名', config.class_name],
    ['功能名称', config.function_name],
    ['模块名', config.module_name],
    ['业务名', config.business_name],
  ] as const) {
    if (value.trim() === '') {
      notifyWarning(`${label}不能为空`);
      return;
    }
  }
  const payload: GenTableUpdate = {
    class_name: config.class_name,
    module_name: config.module_name,
    business_name: config.business_name,
    function_name: config.function_name,
    function_author: config.function_author,
    tpl_category: config.tpl_category,
    form_col_num: config.form_col_num,
    remark: config.remark || null,
    columns: editColumns.value.map(toPatch),
  };
  editor.saving = true;
  try {
    await updateGenTable(editor.tableId, payload);
    editor.visible = false;
    notifySuccess('生成配置已保存');
    await load();
  } catch {
    // 同上（模块名非法、表名带前缀等会被后端拒绝）。
  } finally {
    editor.saving = false;
  }
}

// ------------------------------------------------------------------ 预览 / 下载
const preview = reactive({ visible: false, loading: false, index: 0 });
const previewFiles = ref<PreviewFile[]>([]);
const activeFile = computed(() => previewFiles.value[preview.index] ?? null);

async function openPreview(row: GenTableVo): Promise<void> {
  preview.visible = true;
  preview.loading = true;
  preview.index = 0;
  previewFiles.value = [];
  try {
    const data = await previewGenTable(row.table_id);
    previewFiles.value = data.files;
  } catch {
    // 同上。
  } finally {
    preview.loading = false;
  }
}

async function remove(row: GenTableVo): Promise<void> {
  if (!(await confirmDelete(row.table_name))) return;
  try {
    await deleteGenTable(row.table_id);
    notifySuccess();
    await load();
  } catch {
    // 同上。
  }
}

async function downloadOne(row: GenTableVo): Promise<void> {
  await downloadGenTable(row.table_id, `${row.class_name}.zip`);
}

async function downloadSelected(): Promise<void> {
  if (selected.value.length === 0) return;
  const agreed = await confirm(`下载选中的 ${selected.value.length} 张表的代码包？`);
  if (!agreed) return;
  await downloadGenTables({ page: 1, page_size: selected.value.length });
}
</script>

<template>
  <AppPage
    :title="title"
    :description="'导入表 → 配置字段 → 预览/下载；产出物同时包含 Rust 业务代码与 Vue 页面。'"
  >
    <template #actions>
      <el-button
        v-perm="'tool:gen:code'"
        :disabled="selected.length === 0"
        @click="downloadSelected"
      >
        批量下载
      </el-button>
      <el-button v-perm="'tool:gen:edit'" type="primary" @click="openImport">导入表</el-button>
    </template>

    <template #search>
      <AppSearchForm :model="query" :loading="loading" @search="search" @reset="reset">
        <el-form-item label="表名">
          <el-input v-model="query.table_name" clearable placeholder="如 biz_goods" />
        </el-form-item>
      </AppSearchForm>
    </template>

    <AppTable
      v-model:selection="selected"
      :rows="rows"
      :loading="loading"
      :total="total"
      :page="page"
      :page-size="pageSize"
      row-key="table_id"
      selectable
      empty-title="还没有导入任何表"
      empty-hint="从「导入表」里挑一张有自增主键的业务表开始。"
      @update:page="changePage"
      @update:page-size="changePageSize"
    >
      <el-table-column prop="table_name" label="表名" min-width="170" />
      <el-table-column prop="table_comment" label="表说明" min-width="160" show-overflow-tooltip />
      <el-table-column prop="class_name" label="实体类" min-width="130" />
      <el-table-column label="模块 / 业务" min-width="170">
        <template #default="{ row }">
          <code class="gen__code">{{ row.module_name }} / {{ row.business_name }}</code>
        </template>
      </el-table-column>
      <el-table-column label="模板" width="120">
        <template #default="{ row }">{{
          tplLabels.get(row.tpl_category) ?? row.tpl_category
        }}</template>
      </el-table-column>
      <el-table-column :label="t('common.createTime')" width="170">
        <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
      </el-table-column>

      <el-table-column :label="t('common.actions')" width="250" fixed="right">
        <template #default="{ row }">
          <el-button v-perm="'tool:gen:preview'" link type="primary" @click="openPreview(row)">
            预览
          </el-button>
          <el-button v-perm="'tool:gen:edit'" link type="primary" @click="openConfig(row)">
            配置
          </el-button>
          <el-button v-perm="'tool:gen:code'" link @click="downloadOne(row)">下载</el-button>
          <el-button v-perm="'tool:gen:remove'" link type="danger" @click="remove(row)">
            {{ t('common.delete') }}
          </el-button>
        </template>
      </el-table-column>
    </AppTable>

    <!-- 导入 -->
    <AppDialog
      v-model="importer.visible"
      title="导入数据库表"
      width="760px"
      :confirm-loading="importer.importing"
      confirm-text="导入"
      @confirm="submitImport"
    >
      <el-form label-width="88px" class="gen__import-form" @submit.prevent>
        <el-form-item label="模板">
          <el-radio-group v-model="importer.tplCategory">
            <el-radio v-for="item in TPL_CATEGORY_OPTIONS" :key="item.value" :value="item.value">
              {{ item.label }}
            </el-radio>
          </el-radio-group>
        </el-form-item>
      </el-form>

      <p v-if="allowedPrefixes.length > 0" class="gen__hint">
        只接受这些前缀的表：{{ allowedPrefixes.join('、') }}
      </p>
      <p v-else class="gen__hint">配置未限制表名前缀，但仍会拒绝框架表与没有自增主键的表。</p>

      <el-table
        v-loading="importer.loading"
        :data="dbTables"
        height="360px"
        @selection-change="onDbSelection($event as DbTableVo[])"
      >
        <el-table-column
          type="selection"
          width="46"
          :selectable="(row: DbTableVo) => row.generatable && !row.imported"
        />
        <el-table-column prop="table_name" label="表名" min-width="180" />
        <el-table-column prop="table_comment" label="说明" min-width="160" show-overflow-tooltip />
        <el-table-column prop="column_count" label="字段数" width="90" />
        <el-table-column label="状态" min-width="160">
          <template #default="{ row }">
            <el-tag v-if="row.imported" type="info" effect="light" round size="small"
              >已导入</el-tag
            >
            <el-tag v-else-if="!row.generatable" type="warning" effect="light" round size="small">
              {{ row.reason || '不可生成' }}
            </el-tag>
            <el-tag v-else type="success" effect="light" round size="small">可导入</el-tag>
          </template>
        </el-table-column>
      </el-table>
    </AppDialog>

    <!-- 配置 -->
    <AppDrawer
      v-model="editor.visible"
      title="生成配置"
      size="900px"
      :hide-footer="false"
      :confirm-loading="editor.saving"
      @confirm="submitConfig"
    >
      <div class="gen__config">
        <el-form label-width="96px" :model="config" @submit.prevent>
          <el-row :gutter="16">
            <el-col :span="12">
              <el-form-item label="实体类名" required>
                <el-input v-model="config.class_name" placeholder="如 Goods" />
              </el-form-item>
            </el-col>
            <el-col :span="12">
              <el-form-item label="功能名称" required>
                <el-input v-model="config.function_name" placeholder="如 商品" />
              </el-form-item>
            </el-col>
            <el-col :span="12">
              <el-form-item label="模块名" required>
                <el-input v-model="config.module_name" placeholder="如 business" />
              </el-form-item>
            </el-col>
            <el-col :span="12">
              <el-form-item label="业务名" required>
                <el-input v-model="config.business_name" placeholder="如 goods" />
              </el-form-item>
            </el-col>
            <el-col :span="12">
              <el-form-item label="作者">
                <el-input v-model="config.function_author" />
              </el-form-item>
            </el-col>
            <el-col :span="12">
              <el-form-item label="表单列数">
                <el-input-number
                  v-model="config.form_col_num"
                  :min="1"
                  :max="4"
                  controls-position="right"
                />
              </el-form-item>
            </el-col>
            <el-col :span="12">
              <el-form-item label="模板">
                <el-select v-model="config.tpl_category" style="width: 100%">
                  <el-option
                    v-for="item in TPL_CATEGORY_OPTIONS"
                    :key="item.value"
                    :label="item.label"
                    :value="item.value"
                  />
                </el-select>
              </el-form-item>
            </el-col>
            <el-col :span="12">
              <el-form-item :label="t('common.remark')">
                <el-input v-model="config.remark" />
              </el-form-item>
            </el-col>
          </el-row>
        </el-form>

        <h3 class="gen__section">字段配置</h3>
        <p class="gen__hint">
          勾「列表」出现在表格列，勾「查询」出现在查询条，勾「必填」生成表单校验；
          绑定字典类型后下拉框会自动取字典项。
        </p>

        <el-table :data="editColumns" border height="420px">
          <el-table-column prop="column_name" label="字段" width="150" fixed />
          <el-table-column label="说明" width="150">
            <template #default="{ row }">
              <el-input v-model="row.column_comment" size="small" />
            </template>
          </el-table-column>
          <el-table-column prop="rust_type" label="Rust 类型" width="110" />
          <el-table-column label="列表" width="70" align="center">
            <template #default="{ row }">
              <el-checkbox v-model="row.is_list" :disabled="row.is_pk" />
            </template>
          </el-table-column>
          <el-table-column label="查询" width="70" align="center">
            <template #default="{ row }">
              <el-checkbox v-model="row.is_query" />
            </template>
          </el-table-column>
          <el-table-column label="查询方式" width="120">
            <template #default="{ row }">
              <el-select v-model="row.query_type" size="small" :disabled="!row.is_query">
                <el-option
                  v-for="item in QUERY_TYPE_OPTIONS"
                  :key="item.value"
                  :label="item.label"
                  :value="item.value"
                />
              </el-select>
            </template>
          </el-table-column>
          <el-table-column label="插入" width="70" align="center">
            <template #default="{ row }">
              <el-checkbox v-model="row.is_insert" :disabled="row.is_pk && row.is_increment" />
            </template>
          </el-table-column>
          <el-table-column label="编辑" width="70" align="center">
            <template #default="{ row }">
              <el-checkbox v-model="row.is_edit" :disabled="row.is_pk" />
            </template>
          </el-table-column>
          <el-table-column label="必填" width="70" align="center">
            <template #default="{ row }">
              <el-checkbox v-model="row.is_required" />
            </template>
          </el-table-column>
          <el-table-column label="控件" width="140">
            <template #default="{ row }">
              <el-select v-model="row.html_type" size="small">
                <el-option
                  v-for="item in HTML_TYPE_OPTIONS"
                  :key="item.value"
                  :label="item.label"
                  :value="item.value"
                />
              </el-select>
            </template>
          </el-table-column>
          <el-table-column label="字典类型" width="180">
            <template #default="{ row }">
              <el-input v-model="row.dict_type" size="small" placeholder="留空则不绑定" />
            </template>
          </el-table-column>
        </el-table>
      </div>
    </AppDrawer>

    <!-- 预览 -->
    <AppDialog v-model="preview.visible" title="生成预览" width="960px" hide-footer>
      <div v-loading="preview.loading" class="gen__preview">
        <nav class="gen__files">
          <button
            v-for="(file, index) in previewFiles"
            :key="file.path"
            type="button"
            class="gen__file"
            :class="{ 'is-active': index === preview.index }"
            @click="preview.index = index"
          >
            {{ file.path }}
          </button>
        </nav>
        <div class="gen__content">
          <p class="gen__content-title">{{ activeFile?.path }}</p>
          <pre class="gen__pre">{{ activeFile?.content }}</pre>
        </div>
      </div>
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.gen__code {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}

.gen__hint {
  font-size: var(--ci-text-xs);
  line-height: var(--ci-leading-snug);
  color: var(--ci-color-text-tertiary);
  margin-bottom: var(--ci-space-2);
}

.gen__import-form {
  margin-bottom: var(--ci-space-2);
}

.gen__config {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-2);
}

.gen__section {
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-semibold);
  color: var(--ci-color-text);
}

.gen__preview {
  display: grid;
  grid-template-columns: 260px minmax(0, 1fr);
  gap: var(--ci-space-3);
  min-height: 420px;
}

.gen__files {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-1);
  max-height: 480px;
  overflow: auto;
  padding-right: var(--ci-space-1);
}

.gen__file {
  padding: var(--ci-space-2) var(--ci-space-3);
  border: none;
  border-radius: var(--ci-radius-sm);
  background: transparent;
  color: var(--ci-color-text-secondary);
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  text-align: left;
  overflow-wrap: anywhere;
  cursor: pointer;
  transition: var(--ci-transition-control);
}

.gen__file:hover {
  background: var(--ci-color-surface-sunken);
}

.gen__file.is-active {
  background: var(--ci-color-accent-soft);
  color: var(--ci-color-accent);
}

.gen__content {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-2);
  min-width: 0;
}

.gen__content-title {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-tertiary);
}

.gen__pre {
  flex: 1 1 auto;
  max-height: 440px;
  overflow: auto;
  margin: 0;
  padding: var(--ci-space-3);
  border-radius: var(--ci-radius-md);
  background: var(--ci-color-surface-sunken);
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  line-height: var(--ci-leading-normal);
  white-space: pre;
}
</style>
