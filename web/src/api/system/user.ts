/**
 * 用户管理 API。
 *
 * 权限点由后端 `#[require_perm]` 声明，前端只负责在界面上隐藏没权限的入口，
 * 真正的判定永远在后端。
 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export interface UserVo {
  user_id: number;
  user_name: string;
  nick_name: string;
  dept_id?: number | null;
  email?: string | null;
  phonenumber?: string | null;
  /** 0 正常 1 停用。 */
  status: number;
  create_time: string;
  role_ids: number[];
}

export interface UserListQuery extends PageQuery {
  user_name?: string;
  status?: number;
  dept_id?: number;
}

export interface UserCreate {
  user_name: string;
  nick_name: string;
  password: string;
  dept_id?: number | null;
  email?: string | null;
  phonenumber?: string | null;
  status?: number;
  remark?: string | null;
  role_ids?: number[];
}

export interface UserUpdate {
  nick_name: string;
  dept_id?: number | null;
  email?: string | null;
  phonenumber?: string | null;
  status?: number;
  remark?: string | null;
  role_ids?: number[];
}

/** 导入结果报告（`POST /api/v1/system/users/import`）。 */
export interface ImportReport {
  total: number;
  inserted: number;
  updated: number;
  skipped: number;
  failures: { row: number; message: string }[];
}

export function listUsers(params: UserListQuery): Promise<PageResult<UserVo>> {
  return request.get<PageResult<UserVo>>('/api/v1/system/users', { params });
}

export function getUser(userId: number): Promise<UserVo> {
  return request.get<UserVo>(`/api/v1/system/users/${userId}`);
}

export function createUser(data: UserCreate): Promise<UserVo> {
  return request.post<UserVo>('/api/v1/system/users', data);
}

export function updateUser(userId: number, data: UserUpdate): Promise<UserVo> {
  return request.put<UserVo>(`/api/v1/system/users/${userId}`, data);
}

export function deleteUser(userId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/users/${userId}`);
}

export function resetUserPassword(userId: number, password: string): Promise<void> {
  return request.put<void>(`/api/v1/system/users/${userId}/password`, { password });
}

export function exportUsers(params: UserListQuery): Promise<void> {
  return request.download('/api/v1/system/users/export', { config: { params } });
}

export function downloadImportTemplate(): Promise<void> {
  return request.download('/api/v1/system/users/import-template', {
    filename: '用户导入模板.xlsx',
  });
}

/** `mode: 'partial'` 时单行失败不中断，返回逐行报告；缺省是失败即停。 */
export function importUsers(file: File, mode?: 'partial'): Promise<ImportReport> {
  return request.upload<ImportReport>('/api/v1/system/users/import', file, {
    params: mode ? { mode } : undefined,
  });
}
