/** 部门管理 API。返回的是树（不分页），树形表格与部门选择器共用同一份数据。 */

import { request } from '@/utils/request';

export interface DeptVo {
  dept_id: number;
  parent_id: number;
  dept_name: string;
  /** 祖先链路，形如 `0,100,101`：用于「本部门及以下」的数据范围判断。 */
  ancestors: string;
  order_num: number;
  leader?: string | null;
  phone?: string | null;
  email?: string | null;
  status: number;
  children: DeptVo[];
}

export interface DeptUpsert {
  parent_id: number;
  dept_name: string;
  order_num: number;
  leader?: string | null;
  phone?: string | null;
  email?: string | null;
  status: number;
  remark?: string | null;
}

export function listDepts(): Promise<DeptVo[]> {
  return request.get<DeptVo[]>('/api/v1/system/depts');
}

export function createDept(data: DeptUpsert): Promise<DeptVo> {
  return request.post<DeptVo>('/api/v1/system/depts', data);
}

export function updateDept(deptId: number, data: DeptUpsert): Promise<DeptVo> {
  return request.put<DeptVo>(`/api/v1/system/depts/${deptId}`, data);
}

export function deleteDept(deptId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/depts/${deptId}`);
}
