/** 参数设置 API（对应参考实现的 `sys_config`；`config_key` 是缓存键）。 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export interface ConfigVo {
  config_id: number;
  config_name: string;
  config_key: string;
  config_value: string;
  /** `Y` 内置 / `N` 自定义：内置参数不允许删除。 */
  config_type: string;
  remark?: string | null;
  create_time: string;
}

export interface ConfigListQuery extends PageQuery {
  config_name?: string;
  config_key?: string;
}

export interface ConfigUpsert {
  config_name: string;
  config_key: string;
  config_value: string;
  config_type: string;
  remark?: string | null;
}

export function listConfigs(params: ConfigListQuery): Promise<PageResult<ConfigVo>> {
  return request.get<PageResult<ConfigVo>>('/api/v1/system/configs', { params });
}

export function createConfig(data: ConfigUpsert): Promise<ConfigVo> {
  return request.post<ConfigVo>('/api/v1/system/configs', data);
}

export function updateConfig(configId: number, data: ConfigUpsert): Promise<ConfigVo> {
  return request.put<ConfigVo>(`/api/v1/system/configs/${configId}`, data);
}

export function deleteConfig(configId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/configs/${configId}`);
}

/** 手动刷新参数缓存（改完 `sys_config` 直连数据库的场景）。 */
export function refreshConfigCache(): Promise<void> {
  return request.post<void>('/api/v1/system/configs/refresh-cache');
}

export function exportConfigs(params: ConfigListQuery): Promise<void> {
  return request.download('/api/v1/system/configs/export', { config: { params } });
}
