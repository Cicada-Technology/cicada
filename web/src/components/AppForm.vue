<script setup lang="ts">
/**
 * 表单：**结构由数据描述**（`fields`），而不是每个页面重复写一遍 `el-form-item`。
 *
 * 这样做的收益不是少打字，而是把「必填校验、字典选项、禁用态、条件显示」这四件
 * 每个页面都会写错一次的事固定下来：规则由 `required` 推导，字典选项由 `useDict` 拉，
 * 条件显示统一走 `when`。
 *
 * 需要控件级定制时用插槽 `#field:<prop>` 覆盖单个字段，或 `#after` 追加自定义内容，
 * 不需要为此放弃整个组件。
 */
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import type { FormInstance, FormItemRule } from 'element-plus';

import AppDictSelect from './AppDictSelect.vue';
import AppTreeSelect, { type TreeSelectNode } from './AppTreeSelect.vue';

export type AppFormFieldType =
  | 'input'
  | 'password'
  | 'textarea'
  | 'number'
  | 'select'
  | 'switch'
  | 'date'
  | 'datetime'
  | 'time'
  | 'tree-select'
  | 'radio'
  | 'slot';

export interface AppFormOption {
  label: string;
  value: string | number | boolean;
}

export interface AppFormField {
  prop: string;
  label: string;
  type?: AppFormFieldType;
  required?: boolean;
  disabled?: boolean;
  placeholder?: string;
  /** 静态选项（`select` / `radio`）。 */
  options?: AppFormOption[];
  /** 字典类型：优先于 `options`，选项由 `useDict` 拉取。 */
  dictType?: string;
  multiple?: boolean;
  clearable?: boolean;
  rows?: number;
  /** 栅格占比（24 栅格制）；缺省按 `columns` 均分。 */
  span?: number;
  /** 字段下方的补充说明。 */
  tips?: string;
  /** 条件显示：返回 false 时既不渲染也不校验。 */
  when?: (model: Record<string, unknown>) => boolean;
  /** `tree-select` 的数据与键名。 */
  treeData?: TreeSelectNode[];
  treeLabelKey?: string;
  treeValueKey?: string;
  /** 追加校验规则（在 `required` 之外）。 */
  rules?: FormItemRule[];
  /** 透传给底层控件（`maxlength`、`step` 之类）。 */
  controlProps?: Record<string, unknown>;
}

const props = withDefaults(
  defineProps<{
    /** 表单数据对象（`reactive` 或 `ref` 解出来的都行）。 */
    model: Record<string, unknown>;
    fields: AppFormField[];
    labelWidth?: string;
    labelPosition?: 'left' | 'right' | 'top';
    /** 一行几列；弹层表单一般是 1，详情页可以是 2。 */
    columns?: number;
    /** 服务端字段级错误（`ApiError.fieldMap`）：直接显示到对应字段下。 */
    serverErrors?: Record<string, string>;
    disabled?: boolean;
  }>(),
  {
    labelWidth: '104px',
    labelPosition: 'right',
    columns: 1,
    serverErrors: () => ({}),
    disabled: false,
  },
);

const { t } = useI18n();

const formRef = ref<FormInstance>();

/** 条件显示：`when` 缺省即显示。 */
const visibleFields = computed(() =>
  props.fields.filter((field) => (field.when ? field.when(props.model) : true)),
);

function spanOf(field: AppFormField): number {
  if (field.span) return Math.min(24, Math.max(1, field.span));
  return Math.max(1, Math.round(24 / props.columns));
}

function rulesOf(field: AppFormField): FormItemRule[] {
  const rules: FormItemRule[] = [];
  if (field.required) {
    rules.push({
      required: true,
      message:
        field.type === 'select' || field.type === 'radio' || field.type === 'tree-select'
          ? t('common.pleaseSelect', { field: field.label })
          : t('common.pleaseInput', { field: field.label }),
      // 数字 0 与布尔 false 是合法值，交给自定义判定，不能让 Element 的空值检查误杀。
      validator: (_rule, value: unknown, callback) => {
        const empty =
          value === undefined ||
          value === null ||
          value === '' ||
          (Array.isArray(value) && value.length === 0);
        callback(empty ? new Error(rules[0]?.message as string) : undefined);
      },
      trigger: 'change',
    });
  }
  if (field.rules) rules.push(...field.rules);
  return rules;
}

/** 服务端错误清空：用户一改就该重新校验，不然错误提示会粘住。 */
watch(
  () => props.serverErrors,
  () => void 0,
);

async function validate(): Promise<boolean> {
  if (!formRef.value) return true;
  try {
    await formRef.value.validate();
    return true;
  } catch {
    return false;
  }
}

function resetFields(): void {
  formRef.value?.resetFields();
}

function clearValidate(): void {
  formRef.value?.clearValidate();
}

defineExpose({ validate, resetFields, clearValidate, formRef });
</script>

<template>
  <el-form
    ref="formRef"
    class="app-form"
    :model="model"
    :label-width="labelPosition === 'top' ? undefined : labelWidth"
    :label-position="labelPosition"
    :disabled="disabled"
    @submit.prevent
  >
    <el-row :gutter="16">
      <el-col v-for="field in visibleFields" :key="field.prop" :span="spanOf(field)">
        <el-form-item
          :label="field.label"
          :prop="field.prop"
          :rules="rulesOf(field)"
          :error="serverErrors[field.prop]"
        >
          <slot :name="`field:${field.prop}`" :field="field" :model="model">
            <AppDictSelect
              v-if="field.dictType"
              v-model="model[field.prop] as string"
              :dict-type="field.dictType"
              :disabled="field.disabled"
              :multiple="field.multiple"
              :clearable="field.clearable ?? true"
            />

            <AppTreeSelect
              v-else-if="field.type === 'tree-select'"
              v-model="model[field.prop] as number | string | null"
              :data="field.treeData ?? []"
              :label-key="field.treeLabelKey ?? 'label'"
              :value-key="field.treeValueKey ?? 'value'"
              :disabled="field.disabled"
              :placeholder="field.placeholder"
            />

            <el-select
              v-else-if="field.type === 'select'"
              v-model="model[field.prop]"
              :placeholder="field.placeholder || t('common.pleaseChoose')"
              :disabled="field.disabled"
              :multiple="field.multiple"
              :clearable="field.clearable ?? true"
              style="width: 100%"
            >
              <el-option
                v-for="option in field.options ?? []"
                :key="String(option.value)"
                :label="option.label"
                :value="option.value"
              />
            </el-select>

            <el-radio-group
              v-else-if="field.type === 'radio'"
              v-model="model[field.prop]"
              :disabled="field.disabled"
            >
              <el-radio
                v-for="option in field.options ?? []"
                :key="String(option.value)"
                :value="option.value"
              >
                {{ option.label }}
              </el-radio>
            </el-radio-group>

            <el-switch
              v-else-if="field.type === 'switch'"
              v-model="model[field.prop]"
              :disabled="field.disabled"
            />

            <el-input
              v-else-if="field.type === 'textarea'"
              v-model="model[field.prop] as string"
              type="textarea"
              :rows="field.rows ?? 3"
              :placeholder="field.placeholder || t('common.pleaseInput', { field: field.label })"
              :disabled="field.disabled"
              v-bind="field.controlProps"
            />

            <el-input-number
              v-else-if="field.type === 'number'"
              v-model="model[field.prop] as number"
              :disabled="field.disabled"
              controls-position="right"
              style="width: 100%"
              v-bind="field.controlProps"
            />

            <el-date-picker
              v-else-if="field.type === 'date' || field.type === 'datetime'"
              v-model="model[field.prop] as string"
              :type="field.type === 'date' ? 'date' : 'datetime'"
              value-format="YYYY-MM-DDTHH:mm:ssZ"
              :placeholder="field.placeholder || t('common.pleaseChoose')"
              :disabled="field.disabled"
              style="width: 100%"
            />

            <el-time-picker
              v-else-if="field.type === 'time'"
              v-model="model[field.prop] as string"
              value-format="HH:mm:ss"
              :disabled="field.disabled"
              style="width: 100%"
            />

            <el-input
              v-else
              v-model="model[field.prop] as string"
              :type="field.type === 'password' ? 'password' : 'text'"
              :placeholder="field.placeholder || t('common.pleaseInput', { field: field.label })"
              :autocomplete="field.type === 'password' ? 'new-password' : 'off'"
              :disabled="field.disabled"
              :show-password="field.type === 'password'"
              :clearable="field.clearable ?? true"
              v-bind="field.controlProps"
            />
          </slot>
          <p v-if="field.tips" class="app-form__tips">{{ field.tips }}</p>
        </el-form-item>
      </el-col>
    </el-row>
    <slot name="after" />
  </el-form>
</template>

<style scoped>
.app-form__tips {
  width: 100%;
  margin-top: var(--ci-space-1);
  font-size: var(--ci-text-xs);
  line-height: var(--ci-leading-snug);
  color: var(--ci-color-text-tertiary);
}
</style>
