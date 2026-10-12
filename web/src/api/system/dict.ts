/**
 * 字典管理 API（类型 + 数据）。
 *
 * 路径与接口清单的写法不同：实现里是 `/system/dict/types` 与 `/system/dict/data`
 * （接口清单写的是 `/dict-types`、`/dict-data`）。以实现为准，附录待同步。
 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export interface DictTypeVo {
  dict_id: number;
  dict_name: string;
  dict_type: string;
  status: number;
  remark?: string | null;
  create_time: string;
}

export interface DictTypeListQuery extends PageQuery {
  dict_name?: string;
  dict_type?: string;
  status?: number;
}

export interface DictTypeUpsert {
  dict_name: string;
  dict_type: string;
  status: number;
  remark?: string | null;
}

export interface DictDataVo {
  dict_code: number;
  dict_sort: number;
  dict_label: string;
  dict_value: string;
  dict_type: string;
  css_class?: string | null;
  /** Element Plus 标签样式（`success` / `info` / `warning` / `danger`）。 */
  list_class?: string | null;
  /** `Y` / `N`。 */
  is_default: string;
  status: number;
  remark?: string | null;
  create_time: string;
}

export interface DictDataListQuery extends PageQuery {
  dict_type?: string;
  dict_label?: string;
  status?: number;
}

export interface DictDataUpsert {
  dict_label: string;
  dict_value: string;
  dict_type: string;
  dict_sort: number;
  css_class?: string | null;
  list_class?: string | null;
  is_default: string;
  status: number;
  remark?: string | null;
}

export function listDictTypes(params: DictTypeListQuery): Promise<PageResult<DictTypeVo>> {
  return request.get<PageResult<DictTypeVo>>('/api/v1/system/dict/types', { params });
}

export function createDictType(data: DictTypeUpsert): Promise<DictTypeVo> {
  return request.post<DictTypeVo>('/api/v1/system/dict/types', data);
}

export function updateDictType(dictId: number, data: DictTypeUpsert): Promise<DictTypeVo> {
  return request.put<DictTypeVo>(`/api/v1/system/dict/types/${dictId}`, data);
}

export function deleteDictType(dictId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/dict/types/${dictId}`);
}

export function listDictData(params: DictDataListQuery): Promise<PageResult<DictDataVo>> {
  return request.get<PageResult<DictDataVo>>('/api/v1/system/dict/data', { params });
}

export function createDictData(data: DictDataUpsert): Promise<DictDataVo> {
  return request.post<DictDataVo>('/api/v1/system/dict/data', data);
}

export function updateDictData(dictCode: number, data: DictDataUpsert): Promise<DictDataVo> {
  return request.put<DictDataVo>(`/api/v1/system/dict/data/${dictCode}`, data);
}

export function deleteDictData(dictCode: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/dict/data/${dictCode}`);
}

/**
 * 按字典类型取全部启用项——**只要求登录**，不要求 `system:dict:list`。
 *
 * 字典是渲染表单用的参考数据（如「性别」「状态」），任何能打开该页面的角色都需要它；
 * 复用分页接口时，没有字典管理权限的用户会看到空下拉框，故后端单独开了一条读路径
 * （`crates/web/src/system/dict.rs::list_dict_data_by_type`）。
 */
export function dictDataByType(dictType: string): Promise<DictDataVo[]> {
  return request.get<DictDataVo[]>(`/api/v1/system/dict/data/by-type/${dictType}`, {
    silent: true,
  });
}
