<script setup lang="ts">
/**
 * 字典管理（`system:dict:*`）：类型 + 数据两段式。
 *
 * 为什么是两栏而不是两个菜单项：字典数据**必须**依附于某个类型才有意义（`dict_type`
 * 是它的外键），分成两个页面会让用户在两页之间来回记类型名。左选类型、右改数据，
 * 是这类「一对多字典表」唯一不别扭的排法。
 *
 * 保存后要清掉 `stores/dict` 的缓存：同一会话里其他页面的下拉框都读那份缓存，
 * 不清就会出现「这里改了、表单里还是旧选项」。
 */
import { computed, reactive, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute } from 'vue-router';

import {
  createDictData,
  createDictType,
  deleteDictData,
  deleteDictType,
  listDictData,
  listDictTypes,
  updateDictData,
  updateDictType,
  type DictDataListQuery,
  type DictDataUpsert,
  type DictDataVo,
  type DictTypeListQuery,
  type DictTypeUpsert,
  type DictTypeVo,
} from '@/api/system/dict';
import { STATUS_ENABLED, STATUS_OPTIONS } from '@/api/types';
import AppDialog from '@/components/AppDialog.vue';
import AppEmpty from '@/components/AppEmpty.vue';
import AppForm, { type AppFormField } from '@/components/AppForm.vue';
import AppPage from '@/components/AppPage.vue';
import AppSearchForm from '@/components/AppSearchForm.vue';
import AppStatusTag from '@/components/AppStatusTag.vue';
import AppTable from '@/components/AppTable.vue';
import { useConfirm } from '@/composables/useConfirm';
import { useTable } from '@/composables/useTable';
import { useDictStore } from '@/stores/dict';
import { notifySuccess } from '@/utils/feedback';
import { formatDateTime } from '@/utils/format';

const { t } = useI18n();
const route = useRoute();
const { confirmDelete } = useConfirm();
const dictStore = useDictStore();

const title = computed(() => route.meta.title ?? '');

// ---------------------------------------------------------------- 字典类型
const types = useTable<DictTypeVo, DictTypeListQuery>({ fetcher: listDictTypes });
const selected = ref<DictTypeVo | null>(null);
const typeFormRef = ref<InstanceType<typeof AppForm>>();
const typeDialog = reactive({ visible: false, id: null as number | null });
const typeSaving = ref(false);

const typeForm = reactive({ dict_name: '', dict_type: '', status: STATUS_ENABLED, remark: '' });

const typeFields = computed<AppFormField[]>(() => [
  { prop: 'dict_name', label: '字典名称', required: true },
  {
    prop: 'dict_type',
    label: '字典类型',
    required: true,
    tips: '表单里 `dict_type` 引用的就是这个字符串，例如 sys_normal_disable。',
  },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

// ---------------------------------------------------------------- 字典数据
const dataTable = useTable<DictDataVo, DictDataListQuery>({
  fetcher: listDictData,
  // 未选类型时不打接口：`dict_type` 是必填过滤条件，先拉一页全量没有意义。
  immediate: false,
});
const dataFormRef = ref<InstanceType<typeof AppForm>>();
const dataDialog = reactive({ visible: false, id: null as number | null });
const dataSaving = ref(false);

const dataForm = reactive({
  dict_label: '',
  dict_value: '',
  dict_sort: 0,
  list_class: 'primary',
  css_class: '',
  is_default: 'N',
  status: STATUS_ENABLED,
  remark: '',
});

const dataFields = computed<AppFormField[]>(() => [
  { prop: 'dict_label', label: '标签', required: true },
  { prop: 'dict_value', label: '键值', required: true },
  { prop: 'dict_sort', label: t('common.sort'), type: 'number' },
  {
    prop: 'list_class',
    label: '标签样式',
    type: 'select',
    options: [
      { value: 'primary', label: 'primary' },
      { value: 'success', label: 'success' },
      { value: 'info', label: 'info' },
      { value: 'warning', label: 'warning' },
      { value: 'danger', label: 'danger' },
    ],
    tips: '表格里该值显示成标签时的颜色。',
  },
  { prop: 'css_class', label: 'CSS 类' },
  {
    prop: 'is_default',
    label: t('dict.isDefault'),
    type: 'select',
    options: [
      { value: 'Y', label: '是' },
      { value: 'N', label: '否' },
    ],
  },
  {
    prop: 'status',
    label: t('common.status'),
    type: 'select',
    options: STATUS_OPTIONS.map((item) => ({ value: item.value, label: item.label })),
  },
  { prop: 'remark', label: t('common.remark'), type: 'textarea' },
]);

function resetTypeForm(): void {
  Object.assign(typeForm, { dict_name: '', dict_type: '', status: STATUS_ENABLED, remark: '' });
  typeFormRef.value?.clearValidate();
}

function openTypeCreate(): void {
  typeDialog.id = null;
  resetTypeForm();
  typeDialog.visible = true;
}

function openTypeEdit(row: DictTypeVo): void {
  typeDialog.id = row.dict_id;
  resetTypeForm();
  Object.assign(typeForm, {
    dict_name: row.dict_name,
    dict_type: row.dict_type,
    status: row.status,
    remark: row.remark ?? '',
  });
  typeDialog.visible = true;
}

async function submitType(): Promise<void> {
  if (!(await typeFormRef.value?.validate())) return;
  const payload: DictTypeUpsert = {
    dict_name: typeForm.dict_name,
    dict_type: typeForm.dict_type,
    status: typeForm.status,
    remark: typeForm.remark || null,
  };
  typeSaving.value = true;
  try {
    if (typeDialog.id === null) await createDictType(payload);
    else await updateDictType(typeDialog.id, payload);
    typeDialog.visible = false;
    invalidateDict();
    notifySuccess();
    await types.load();
  } catch {
    // 提示由拦截器给出，弹层保持打开。
  } finally {
    typeSaving.value = false;
  }
}

async function removeType(row: DictTypeVo): Promise<void> {
  if (!(await confirmDelete(row.dict_name))) return;
  try {
    await deleteDictType(row.dict_id);
    if (selected.value?.dict_id === row.dict_id) {
      selected.value = null;
      dataTable.rows.value = [];
      dataTable.total.value = 0;
    }
    invalidateDict();
    notifySuccess();
    await types.load();
  } catch {
    // 有字典数据时后端会拒绝，提示由拦截器给出。
  }
}

/** 选中类型 → 右侧加载该类型的数据。 */
function selectType(row: DictTypeVo): void {
  selected.value = row;
  dataTable.query.value = { dict_type: row.dict_type };
  void dataTable.search();
}

function resetDataForm(): void {
  Object.assign(dataForm, {
    dict_label: '',
    dict_value: '',
    dict_sort: 0,
    list_class: 'primary',
    css_class: '',
    is_default: 'N',
    status: STATUS_ENABLED,
    remark: '',
  });
  dataFormRef.value?.clearValidate();
}

function openDataCreate(): void {
  dataDialog.id = null;
  resetDataForm();
  dataDialog.visible = true;
}

function openDataEdit(row: DictDataVo): void {
  dataDialog.id = row.dict_code;
  resetDataForm();
  Object.assign(dataForm, {
    dict_label: row.dict_label,
    dict_value: row.dict_value,
    dict_sort: row.dict_sort,
    list_class: row.list_class ?? 'primary',
    css_class: row.css_class ?? '',
    is_default: row.is_default,
    status: row.status,
    remark: row.remark ?? '',
  });
  dataDialog.visible = true;
}

async function submitData(): Promise<void> {
  if (!(await dataFormRef.value?.validate())) return;
  const dictType = selected.value?.dict_type ?? '';
  const payload: DictDataUpsert = {
    dict_label: dataForm.dict_label,
    dict_value: dataForm.dict_value,
    dict_type: dictType,
    dict_sort: dataForm.dict_sort,
    list_class: dataForm.list_class || null,
    css_class: dataForm.css_class || null,
    is_default: dataForm.is_default,
    status: dataForm.status,
    remark: dataForm.remark || null,
  };
  dataSaving.value = true;
  try {
    if (dataDialog.id === null) await createDictData(payload);
    else await updateDictData(dataDialog.id, payload);
    dataDialog.visible = false;
    invalidateDict();
    notifySuccess();
    await dataTable.load();
  } catch {
    // 同上。
  } finally {
    dataSaving.value = false;
  }
}

async function removeData(row: DictDataVo): Promise<void> {
  if (!(await confirmDelete(row.dict_label))) return;
  try {
    await deleteDictData(row.dict_code);
    invalidateDict();
    notifySuccess();
    await dataTable.load();
  } catch {
    // 同上。
  }
}

/** 字典变了：清掉其他页面下拉框用的那份缓存（同会话内立即生效）。 */
function invalidateDict(): void {
  dictStore.clear();
}
</script>

<template>
  <AppPage
    :title="title"
    :description="'字典是渲染表单与表格标签的参考数据：左选类型，右改该类型的取值。'"
  >
    <div class="dict">
      <section class="dict__pane ci-card">
        <header class="dict__pane-head">
          <h2 class="dict__pane-title">字典类型</h2>
          <el-button v-perm="'system:dict:add'" type="primary" @click="openTypeCreate">
            {{ t('common.add') }}
          </el-button>
        </header>

        <AppSearchForm
          :model="types.query.value"
          :loading="types.loading.value"
          @search="types.search"
          @reset="types.reset"
        >
          <el-form-item label="名称">
            <el-input v-model="types.query.value.dict_name" clearable placeholder="字典名称" />
          </el-form-item>
          <el-form-item label="类型">
            <el-input v-model="types.query.value.dict_type" clearable placeholder="字典类型" />
          </el-form-item>
        </AppSearchForm>

        <AppTable
          :rows="types.rows.value"
          :loading="types.loading.value"
          :total="types.total.value"
          :page="types.page.value"
          :page-size="types.pageSize.value"
          row-key="dict_id"
          :page-sizes="[10, 20, 50]"
          empty-title="还没有字典类型"
          empty-hint="先建一个类型（如 sys_normal_disable），再往里加取值。"
          @update:page="types.changePage"
          @update:page-size="types.changePageSize"
        >
          <el-table-column label="名称" min-width="160">
            <template #default="{ row }">
              <el-button
                link
                :type="selected?.dict_id === row.dict_id ? 'primary' : 'default'"
                @click="selectType(row as DictTypeVo)"
              >
                {{ row.dict_name }}
              </el-button>
            </template>
          </el-table-column>
          <el-table-column label="类型" min-width="180">
            <template #default="{ row }">
              <code class="dict__type">{{ row.dict_type }}</code>
            </template>
          </el-table-column>
          <el-table-column :label="t('common.status')" width="90">
            <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
          </el-table-column>
          <el-table-column :label="t('common.actions')" width="150">
            <template #default="{ row }">
              <el-button v-perm="'system:dict:edit'" link type="primary" @click="openTypeEdit(row)">
                {{ t('common.edit') }}
              </el-button>
              <el-button v-perm="'system:dict:remove'" link type="danger" @click="removeType(row)">
                {{ t('common.delete') }}
              </el-button>
            </template>
          </el-table-column>
        </AppTable>
      </section>

      <section class="dict__pane ci-card">
        <header class="dict__pane-head">
          <h2 class="dict__pane-title">
            字典数据
            <span v-if="selected" class="dict__selected">{{ selected.dict_name }}</span>
          </h2>
          <el-button
            v-perm="'system:dict:add'"
            type="primary"
            :disabled="!selected"
            @click="openDataCreate"
          >
            {{ t('common.add') }}
          </el-button>
        </header>

        <AppEmpty
          v-if="!selected"
          icon="Pointer"
          title="先选一个字典类型"
          hint="左侧点字典名称即可在右侧维护它的取值。"
        />

        <template v-else>
          <AppSearchForm
            :model="dataTable.query.value"
            :loading="dataTable.loading.value"
            @search="dataTable.search"
            @reset="dataTable.reset"
          >
            <el-form-item label="标签">
              <el-input v-model="dataTable.query.value.dict_label" clearable placeholder="标签" />
            </el-form-item>
          </AppSearchForm>

          <AppTable
            :rows="dataTable.rows.value"
            :loading="dataTable.loading.value"
            :total="dataTable.total.value"
            :page="dataTable.page.value"
            :page-size="dataTable.pageSize.value"
            row-key="dict_code"
            :page-sizes="[10, 20, 50]"
            empty-title="这个类型还没有取值"
            empty-hint="表单里引用该类型时会出现空下拉框，先加几条取值。"
            @update:page="dataTable.changePage"
            @update:page-size="dataTable.changePageSize"
          >
            <el-table-column prop="dict_label" label="标签" min-width="140" />
            <el-table-column prop="dict_value" label="键值" min-width="120" />
            <el-table-column :label="t('common.sort')" width="80" prop="dict_sort" />
            <el-table-column label="样式" width="100">
              <template #default="{ row }">
                <el-tag :type="row.list_class || 'info'" effect="light" round size="small">
                  {{ row.list_class || '—' }}
                </el-tag>
              </template>
            </el-table-column>
            <el-table-column :label="t('dict.isDefault')" width="90">
              <template #default="{ row }">{{ row.is_default === 'Y' ? '是' : '否' }}</template>
            </el-table-column>
            <el-table-column :label="t('common.status')" width="90">
              <template #default="{ row }"><AppStatusTag :value="row.status" /></template>
            </el-table-column>
            <el-table-column :label="t('common.createTime')" width="170">
              <template #default="{ row }">{{ formatDateTime(row.create_time) }}</template>
            </el-table-column>
            <el-table-column :label="t('common.actions')" width="150">
              <template #default="{ row }">
                <el-button
                  v-perm="'system:dict:edit'"
                  link
                  type="primary"
                  @click="openDataEdit(row)"
                >
                  {{ t('common.edit') }}
                </el-button>
                <el-button
                  v-perm="'system:dict:remove'"
                  link
                  type="danger"
                  @click="removeData(row)"
                >
                  {{ t('common.delete') }}
                </el-button>
              </template>
            </el-table-column>
          </AppTable>
        </template>
      </section>
    </div>

    <AppDialog
      v-model="typeDialog.visible"
      :title="typeDialog.id === null ? '新增字典类型' : '修改字典类型'"
      :confirm-loading="typeSaving"
      @confirm="submitType"
    >
      <AppForm ref="typeFormRef" :model="typeForm" :fields="typeFields" />
    </AppDialog>

    <AppDialog
      v-model="dataDialog.visible"
      :title="dataDialog.id === null ? '新增字典数据' : '修改字典数据'"
      :confirm-loading="dataSaving"
      @confirm="submitData"
    >
      <AppForm ref="dataFormRef" :model="dataForm" :fields="dataFields" :columns="2" />
    </AppDialog>
  </AppPage>
</template>

<style scoped>
.dict {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: var(--ci-space-4);
  align-items: start;
}

/* 窄屏堆叠：两栏各带完整表格，并排会把列挤成省略号。 */
@media (max-width: 1200px) {
  .dict {
    grid-template-columns: minmax(0, 1fr);
  }
}

.dict__pane {
  display: flex;
  flex-direction: column;
  gap: var(--ci-space-3);
  padding: var(--ci-space-4);
  min-width: 0;
}

.dict__pane-head {
  display: flex;
  align-items: center;
  gap: var(--ci-space-3);
}

.dict__pane-title {
  display: flex;
  align-items: baseline;
  gap: var(--ci-space-2);
  font-size: var(--ci-text-md);
  font-weight: var(--ci-weight-semibold);
  color: var(--ci-color-text);
}

.dict__pane-head > .el-button {
  margin-left: auto;
}

.dict__selected {
  font-size: var(--ci-text-xs);
  font-weight: var(--ci-weight-regular);
  color: var(--ci-color-text-tertiary);
}

.dict__type {
  font-family: var(--ci-font-mono);
  font-size: var(--ci-text-xs);
  color: var(--ci-color-text-secondary);
}
</style>
