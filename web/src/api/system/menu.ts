/**
 * 菜单管理 API。
 *
 * 菜单是**全局字典**（`sys_menu` 无 `tenant_id`），所以这里没有租户过滤；
 * `GET /menus` 返回包含 `F` 按钮节点的完整树——它同时是角色授权界面的数据源。
 */

import type { MenuType } from '@/api/types';
import { request } from '@/utils/request';

export interface MenuVo {
  menu_id: number;
  menu_name: string;
  parent_id: number;
  order_num: number;
  menu_type: string;
  path?: string | null;
  component?: string | null;
  query?: string | null;
  perms?: string | null;
  icon?: string | null;
  /** 0 显示 1 隐藏。 */
  visible: number;
  status: number;
  /** 1 非外链 0 外链（沿用参考实现的取反语义，与 `isExternalLink` 配合使用）。 */
  is_frame?: number;
  /** 1 缓存 0 不缓存。 */
  is_cache?: number;
  remark?: string | null;
  children: MenuVo[];
}

export interface MenuUpsert {
  menu_name: string;
  parent_id: number;
  order_num: number;
  menu_type: MenuType;
  path?: string | null;
  component?: string | null;
  query?: string | null;
  perms?: string | null;
  icon?: string | null;
  is_frame: number;
  is_cache: number;
  visible: number;
  status: number;
  remark?: string | null;
}

export function listMenus(): Promise<MenuVo[]> {
  return request.get<MenuVo[]>('/api/v1/system/menus');
}

export function createMenu(data: MenuUpsert): Promise<MenuVo> {
  return request.post<MenuVo>('/api/v1/system/menus', data);
}

export function updateMenu(menuId: number, data: MenuUpsert): Promise<MenuVo> {
  return request.put<MenuVo>(`/api/v1/system/menus/${menuId}`, data);
}

export function deleteMenu(menuId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/menus/${menuId}`);
}
