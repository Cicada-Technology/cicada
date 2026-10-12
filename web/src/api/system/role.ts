/** 角色管理 API。角色的菜单/部门授权通过 `RoleUpsert.menu_ids` / `dept_ids` 提交。 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export interface RoleVo {
  role_id: number;
  role_name: string;
  role_key: string;
  role_sort: number;
  /** 数据范围，见 `DATA_SCOPE_*`。 */
  data_scope: number;
  status: number;
  remark?: string | null;
  create_time: string;
}

export interface RoleListQuery extends PageQuery {
  role_name?: string;
  role_key?: string;
  status?: number;
}

export interface RoleUpsert {
  role_name: string;
  role_key: string;
  role_sort?: number;
  data_scope?: number;
  status?: number;
  remark?: string | null;
  /** 菜单授权：提交即整体替换（后端在事务里先删后插）。 */
  menu_ids?: number[];
  /** 数据范围为「自定义」时的部门集合。 */
  dept_ids?: number[];
}

export function listRoles(params: RoleListQuery): Promise<PageResult<RoleVo>> {
  return request.get<PageResult<RoleVo>>('/api/v1/system/roles', { params });
}

export function getRole(roleId: number): Promise<RoleVo> {
  return request.get<RoleVo>(`/api/v1/system/roles/${roleId}`);
}

export function createRole(data: RoleUpsert): Promise<RoleVo> {
  return request.post<RoleVo>('/api/v1/system/roles', data);
}

export function updateRole(roleId: number, data: RoleUpsert): Promise<RoleVo> {
  return request.put<RoleVo>(`/api/v1/system/roles/${roleId}`, data);
}

export function deleteRole(roleId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/roles/${roleId}`);
}

export function exportRoles(params: RoleListQuery): Promise<void> {
  return request.download('/api/v1/system/roles/export', { config: { params } });
}

/**
 * 角色已授权的菜单 id 集合。
 *
 * `RoleVo` 不含 `menu_ids`，而保存是**整体替换**（后端先删后插），所以拿不到原授权就
 * 会把已有授权清空——这个读接口是权限分配界面的必要前提（`views/system/role/index.vue`）。
 * 后端实现见 `crates/web/src/system/role.rs::list_role_menus`。
 */
export function getRoleMenus(roleId: number): Promise<number[]> {
  return request.get<number[]>(`/api/v1/system/roles/${roleId}/menus`);
}
