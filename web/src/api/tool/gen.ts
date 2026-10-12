/**
 * 代码生成器 API。
 *
 * 生成物分两半：Rust（`business/` + `migrations/`）与前端（`web/src/api/…`、`web/src/views/…`），
 * 下载得到的是 zip；预览接口返回逐文件文本，便于在弹层里直接读。
 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

/** 模板分类：单表 / 树表（主子表递延）。 */
export const TPL_CATEGORY_OPTIONS = [
  { value: 'crud', label: '单表（增删改查）' },
  { value: 'tree', label: '树表（树形结构）' },
] as const;

export const QUERY_TYPE_OPTIONS = [
  { value: 'EQ', label: '等于' },
  { value: 'LIKE', label: '模糊' },
  { value: 'BETWEEN', label: '范围' },
] as const;

export const HTML_TYPE_OPTIONS = [
  { value: 'input', label: '文本框' },
  { value: 'textarea', label: '文本域' },
  { value: 'select', label: '下拉框' },
  { value: 'radio', label: '单选框' },
  { value: 'checkbox', label: '复选框' },
  { value: 'datetime', label: '日期时间' },
  { value: 'date', label: '日期' },
  { value: 'switch', label: '开关' },
  { value: 'editor', label: '富文本' },
] as const;

export interface GenColumnVo {
  column_id: number;
  column_name: string;
  column_comment: string;
  column_type: string;
  rust_type: string;
  rust_field: string;
  is_pk: boolean;
  is_increment: boolean;
  is_required: boolean;
  is_insert: boolean;
  is_edit: boolean;
  is_list: boolean;
  is_query: boolean;
  query_type: string;
  html_type: string;
  dict_type?: string | null;
  sort: number;
}

export interface GenTableVo {
  table_id: number;
  table_name: string;
  table_comment: string;
  class_name: string;
  module_name: string;
  business_name: string;
  function_name: string;
  function_author: string;
  tpl_category: string;
  form_col_num: number;
  gen_type: string;
  remark?: string | null;
  create_time: string;
  columns: GenColumnVo[];
}

export interface GenTableListQuery extends PageQuery {
  table_name?: string;
}

export interface DbTableVo {
  table_name: string;
  table_comment: string;
  column_count: number;
  /** 已经导入过，界面据此禁用勾选。 */
  imported: boolean;
  /** 能否生成（框架表 / 缺列 / 复合主键会被拒）。 */
  generatable: boolean;
  reason?: string | null;
}

export interface DbTableList {
  items: DbTableVo[];
  /** 配置里的准入前缀（空表示不限制）。 */
  allowed_prefixes: string[];
}

export interface GenColumnPatch {
  column_name: string;
  column_comment?: string | null;
  is_required?: boolean;
  is_insert?: boolean;
  is_edit?: boolean;
  is_list?: boolean;
  is_query?: boolean;
  query_type?: string;
  html_type?: string;
  /** `null` 或空串表示清空字典绑定。 */
  dict_type?: string | null;
}

export interface GenTableUpdate {
  class_name?: string;
  module_name?: string;
  business_name?: string;
  function_name?: string;
  function_author?: string;
  tpl_category?: string;
  form_col_num?: number;
  remark?: string | null;
  columns: GenColumnPatch[];
}

export interface PreviewFile {
  name: string;
  path: string;
  content: string;
}

export interface PreviewResponse {
  files: PreviewFile[];
}

export function listGenTables(params: GenTableListQuery): Promise<PageResult<GenTableVo>> {
  return request.get<PageResult<GenTableVo>>('/api/v1/tool/gen/tables', { params });
}

export function listDbTables(): Promise<DbTableList> {
  return request.get<DbTableList>('/api/v1/tool/gen/db/tables');
}

export function importTables(tables: string[], tplCategory?: string): Promise<GenTableVo[]> {
  return request.post<GenTableVo[]>('/api/v1/tool/gen/import', {
    tables,
    tpl_category: tplCategory,
  });
}

export function updateGenTable(tableId: number, data: GenTableUpdate): Promise<GenTableVo> {
  return request.put<GenTableVo>(`/api/v1/tool/gen/tables/${tableId}`, data);
}

export function deleteGenTable(tableId: number): Promise<void> {
  return request.delete<void>(`/api/v1/tool/gen/tables/${tableId}`);
}

export function previewGenTable(tableId: number): Promise<PreviewResponse> {
  return request.get<PreviewResponse>(`/api/v1/tool/gen/tables/${tableId}/preview`);
}

/** 下载单表产物（zip）。 */
export function downloadGenTable(tableId: number, filename?: string): Promise<void> {
  return request.download(`/api/v1/tool/gen/tables/${tableId}/download`, { filename });
}

/** 按筛选条件批量下载（zip）。 */
export function downloadGenTables(params: GenTableListQuery, filename?: string): Promise<void> {
  return request.download('/api/v1/tool/gen/tables/download', {
    filename,
    config: { params },
  });
}
