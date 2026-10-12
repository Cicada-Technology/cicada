/**
 * 接口层的公共类型（` 统一响应`）。
 *
 * 约定：字段名与后端 **逐字一致**（`snake_case`）。不做 camelCase 转换——多一层映射就多一处
 * 会漂移的地方，而代码生成器产出的 `api.ts` 本来就直接对着后端 DTO 写。
 */

/** 集合响应。后端一律返回带 `total` 的对象而不是裸数组，前端分页组件因此不需要适配层。 */
export interface PageResult<T> {
  items: T[];
  total: number;
  page: number;
  page_size: number;
}

/** 分页查询参数；后端各模块的 `*ListQuery` 都是它的扩展。 */
export interface PageQuery {
  page?: number;
  page_size?: number;
}

/** 状态位：后端是 `i16`，`0` 正常、`1` 停用（沿用参考实现语义）。 */
export const STATUS_ENABLED = 0;
export const STATUS_DISABLED = 1;

/** 数据范围：1 全部 2 自定义 3 本部门 4 本部门及以下 5 仅本人。 */
export const DATA_SCOPE_ALL = 1;
export const DATA_SCOPE_CUSTOM = 2;
export const DATA_SCOPE_DEPT = 3;
export const DATA_SCOPE_DEPT_AND_CHILD = 4;
export const DATA_SCOPE_SELF = 5;

export const DATA_SCOPE_OPTIONS = [
  { value: DATA_SCOPE_ALL, label: '全部数据' },
  { value: DATA_SCOPE_CUSTOM, label: '自定义数据' },
  { value: DATA_SCOPE_DEPT, label: '本部门数据' },
  { value: DATA_SCOPE_DEPT_AND_CHILD, label: '本部门及以下' },
  { value: DATA_SCOPE_SELF, label: '仅本人数据' },
] as const;

export const STATUS_OPTIONS = [
  { value: STATUS_ENABLED, label: '正常' },
  { value: STATUS_DISABLED, label: '停用' },
] as const;

/** 菜单类型：`M` 目录 / `C` 菜单 / `F` 按钮权限。 */
export type MenuType = 'M' | 'C' | 'F';

export const MENU_TYPE_OPTIONS = [
  { value: 'M', label: '目录' },
  { value: 'C', label: '菜单' },
  { value: 'F', label: '按钮' },
] as const;

export function isExternalLink(path?: string | null): boolean {
  return Boolean(path && /^https?:\/\//i.test(path));
}
