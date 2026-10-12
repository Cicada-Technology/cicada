/**
 * 平台管理：租户、租户套餐、开放接口凭证（M4 扩展， / ADR 0003 / ADR 0004）。
 *
 * 三者在界面上都属于「平台管理员」范畴：租户与配额、套餐限额、API Key 的签发与吊销。
 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export interface TenantVo {
  tenant_id: number;
  tenant_name: string;
  status: number;
  package_id?: number | null;
  package_name?: string | null;
  remark?: string | null;
  create_time: string;
}

export interface TenantListQuery extends PageQuery {
  tenant_name?: string;
  status?: number;
}

export interface TenantUpsert {
  tenant_name: string;
  status: number;
  package_id?: number | null;
  remark?: string | null;
}

/** 租户已用与上限（`GET /system/tenants/{id}/quota`）。 */
export interface QuotaVo {
  tenant_id: number;
  package_code?: string | null;
  user_limit: number;
  storage_limit_mb: number;
  api_qps_limit: number;
  users: number;
  storage_mb: number;
  features: Record<string, unknown>;
}

export interface PackageVo {
  id: number;
  code: string;
  name: string;
  user_limit: number;
  storage_limit_mb: number;
  api_qps_limit: number;
  features: Record<string, unknown>;
  status: number;
  remark?: string | null;
  create_time: string;
}

export interface PackageListQuery extends PageQuery {
  code?: string;
  name?: string;
}

export interface PackageUpsert {
  code: string;
  name: string;
  user_limit: number;
  storage_limit_mb: number;
  api_qps_limit: number;
  features: Record<string, unknown>;
  status: number;
  remark?: string | null;
}

export interface ApiKeyVo {
  id: number;
  tenant_id: number;
  name: string;
  /** 键前缀（用于在日志里定位是哪把钥匙），明文只签发时返回一次。 */
  key_prefix: string;
  scopes: string[];
  status: number;
  expires_at?: string | null;
  last_used_at?: string | null;
  create_by?: string | null;
  create_time: string;
}

/**
 * 签发响应。后端 `ApiKeyIssuedVo` 用 `#[serde(flatten)]` 内嵌 `ApiKeyVo`，
 * 所以线上 JSON 是**平铺**的：凭证字段与 `secret` 同级，没有 `key` 这一层。
 */
export interface ApiKeyIssuedVo extends ApiKeyVo {
  /** 明文密钥，只在签发响应里出现一次。 */
  secret: string;
}

export interface ApiKeyListQuery extends PageQuery {
  name?: string;
  status?: number;
}

export interface ApiKeyCreate {
  name: string;
  scopes: string[];
  expires_at?: string | null;
  remark?: string | null;
}

export function listTenants(params: TenantListQuery): Promise<PageResult<TenantVo>> {
  return request.get<PageResult<TenantVo>>('/api/v1/system/tenants', { params });
}

export function createTenant(data: TenantUpsert): Promise<TenantVo> {
  return request.post<TenantVo>('/api/v1/system/tenants', data);
}

export function updateTenant(tenantId: number, data: TenantUpsert): Promise<TenantVo> {
  return request.put<TenantVo>(`/api/v1/system/tenants/${tenantId}`, data);
}

export function deleteTenant(tenantId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/tenants/${tenantId}`);
}

export function getTenantQuota(tenantId: number): Promise<QuotaVo> {
  return request.get<QuotaVo>(`/api/v1/system/tenants/${tenantId}/quota`);
}

export function listPackages(params: PackageListQuery): Promise<PageResult<PackageVo>> {
  return request.get<PageResult<PackageVo>>('/api/v1/system/tenant-packages', { params });
}

export function createPackage(data: PackageUpsert): Promise<PackageVo> {
  return request.post<PackageVo>('/api/v1/system/tenant-packages', data);
}

export function updatePackage(id: number, data: PackageUpsert): Promise<PackageVo> {
  return request.put<PackageVo>(`/api/v1/system/tenant-packages/${id}`, data);
}

export function deletePackage(id: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/tenant-packages/${id}`);
}

export function listApiKeys(params: ApiKeyListQuery): Promise<PageResult<ApiKeyVo>> {
  return request.get<PageResult<ApiKeyVo>>('/api/v1/system/api-keys', { params });
}

export function createApiKey(data: ApiKeyCreate): Promise<ApiKeyIssuedVo> {
  return request.post<ApiKeyIssuedVo>('/api/v1/system/api-keys', data);
}

export function updateApiKeyStatus(id: number, status: number): Promise<void> {
  return request.put<void>(`/api/v1/system/api-keys/${id}/status`, { status });
}

export function deleteApiKey(id: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/api-keys/${id}`);
}
