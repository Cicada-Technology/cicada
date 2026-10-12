/** 操作日志与登录日志（只读视图，属主是 system 域）。 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

/** 业务类型，沿用参考实现的枚举（1 新增 … 9 强退）。 */
export const BUSINESS_TYPE_OPTIONS = [
  { value: 0, label: '其它' },
  { value: 1, label: '新增' },
  { value: 2, label: '修改' },
  { value: 3, label: '删除' },
  { value: 4, label: '授权' },
  { value: 5, label: '导出' },
  { value: 6, label: '导入' },
  { value: 7, label: '强退' },
  { value: 8, label: '生成代码' },
  { value: 9, label: '清空数据' },
] as const;

export interface OperLogVo {
  oper_id: number;
  title: string;
  business_type: number;
  method: string;
  request_method: string;
  oper_name?: string | null;
  oper_url: string;
  oper_ip?: string | null;
  oper_param?: string | null;
  json_result?: string | null;
  /** 0 成功 1 失败。 */
  status: number;
  error_msg?: string | null;
  oper_time: string;
  cost_ms: number;
}

export interface OperLogListQuery extends PageQuery {
  title?: string;
  oper_name?: string;
  business_type?: number;
  status?: number;
  begin_time?: string;
  end_time?: string;
}

export interface LoginLogVo {
  info_id: number;
  user_name: string;
  ipaddr?: string | null;
  login_location?: string | null;
  browser?: string | null;
  os?: string | null;
  /** 0 成功 1 失败。 */
  status: number;
  msg?: string | null;
  login_time: string;
}

export interface LoginLogListQuery extends PageQuery {
  user_name?: string;
  ipaddr?: string;
  status?: number;
  begin_time?: string;
  end_time?: string;
}

export function listOperLogs(params: OperLogListQuery): Promise<PageResult<OperLogVo>> {
  return request.get<PageResult<OperLogVo>>('/api/v1/monitor/oper-logs', { params });
}

export function getOperLog(operId: number): Promise<OperLogVo> {
  return request.get<OperLogVo>(`/api/v1/monitor/oper-logs/${operId}`);
}

export function deleteOperLog(operId: number): Promise<void> {
  return request.delete<void>(`/api/v1/monitor/oper-logs/${operId}`);
}

/** 清空（按当前筛选条件）；传空对象即清空全部——页面必须二次确认。 */
export function cleanOperLogs(params: OperLogListQuery = {}): Promise<void> {
  return request.delete<void>('/api/v1/monitor/oper-logs', { data: params });
}

export function exportOperLogs(params: OperLogListQuery): Promise<void> {
  return request.download('/api/v1/monitor/oper-logs/export', { config: { params } });
}

export function listLoginLogs(params: LoginLogListQuery): Promise<PageResult<LoginLogVo>> {
  return request.get<PageResult<LoginLogVo>>('/api/v1/monitor/login-logs', { params });
}

export function deleteLoginLog(infoId: number): Promise<void> {
  return request.delete<void>(`/api/v1/monitor/login-logs/${infoId}`);
}

export function cleanLoginLogs(params: LoginLogListQuery = {}): Promise<void> {
  return request.delete<void>('/api/v1/monitor/login-logs', { data: params });
}

/** 解锁被密码错误锁定的账号（锁定计数在 Redis 里）。 */
export function unlockLogin(userName: string): Promise<void> {
  return request.put<void>(`/api/v1/monitor/login-logs/unlock/${encodeURIComponent(userName)}`);
}

export function exportLoginLogs(params: LoginLogListQuery): Promise<void> {
  return request.download('/api/v1/monitor/login-logs/export', { config: { params } });
}
