/**
 * 通知中心 API（ADR 0002：三通道经 HTTP 网关下发，渠道/模板/记录三类资源）。
 *
 * 「测试发送」走 `POST /notify/send`：它同步渲染并投递一次，返回记录 id 与结果，
 * 便于在渠道配置页当场验证连通性。
 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

/** 渠道类型：与 `crates/notify` 的通道实现一一对应。 */
export const CHANNEL_KIND_OPTIONS = [
  { value: 'webhook', label: 'Webhook' },
  { value: 'email', label: '邮件' },
  { value: 'sms', label: '短信' },
] as const;

export const RECORD_STATUS_OPTIONS = [
  { value: 1, label: '已发送' },
  { value: 2, label: '失败' },
] as const;

export interface ChannelVo {
  id: number;
  code: string;
  name: string;
  kind: string;
  /** 通道私有配置（webhook 地址、SMTP 账号等），按 `kind` 结构不同。 */
  config: Record<string, unknown>;
  enabled: boolean;
  remark?: string | null;
  create_time: string;
}

export interface ChannelListQuery extends PageQuery {
  code?: string;
  kind?: string;
  enabled?: boolean;
}

export interface ChannelUpsert {
  code: string;
  name: string;
  kind: string;
  config: Record<string, unknown>;
  enabled: boolean;
  remark?: string | null;
}

export interface TemplateVo {
  id: number;
  code: string;
  name: string;
  channel_kind: string;
  /** 语言标签，如 `zh-CN`；同 code 可按语言多份。 */
  locale: string;
  title: string;
  body: string;
  enabled: boolean;
  remark?: string | null;
  create_time: string;
}

export interface TemplateListQuery extends PageQuery {
  code?: string;
  channel_kind?: string;
}

export interface TemplateUpsert {
  code: string;
  name: string;
  channel_kind: string;
  locale: string;
  title: string;
  body: string;
  enabled: boolean;
  remark?: string | null;
}

export interface RecordVo {
  id: number;
  channel_id: number;
  template_code: string;
  recipient: string;
  payload: Record<string, unknown>;
  /** 1 已发送 2 失败。 */
  status: number;
  error?: string | null;
  create_by?: string | null;
  create_time: string;
  sent_at?: string | null;
}

export interface RecordListQuery extends PageQuery {
  template_code?: string;
  recipient?: string;
  status?: number;
}

export interface SendBody {
  channel_code: string;
  template_code: string;
  recipient: string;
  locale?: string;
  payload?: Record<string, unknown>;
}

export interface SendVo {
  record_id: number;
  /** 1 已发送 2 失败。 */
  status: number;
  error?: string | null;
  rendered_body: string;
}

export function listChannels(params: ChannelListQuery): Promise<PageResult<ChannelVo>> {
  return request.get<PageResult<ChannelVo>>('/api/v1/notify/channels', { params });
}

export function createChannel(data: ChannelUpsert): Promise<ChannelVo> {
  return request.post<ChannelVo>('/api/v1/notify/channels', data);
}

export function updateChannel(id: number, data: ChannelUpsert): Promise<ChannelVo> {
  return request.put<ChannelVo>(`/api/v1/notify/channels/${id}`, data);
}

export function deleteChannel(id: number): Promise<void> {
  return request.delete<void>(`/api/v1/notify/channels/${id}`);
}

export function listTemplates(params: TemplateListQuery): Promise<PageResult<TemplateVo>> {
  return request.get<PageResult<TemplateVo>>('/api/v1/notify/templates', { params });
}

export function createTemplate(data: TemplateUpsert): Promise<TemplateVo> {
  return request.post<TemplateVo>('/api/v1/notify/templates', data);
}

export function updateTemplate(id: number, data: TemplateUpsert): Promise<TemplateVo> {
  return request.put<TemplateVo>(`/api/v1/notify/templates/${id}`, data);
}

export function deleteTemplate(id: number): Promise<void> {
  return request.delete<void>(`/api/v1/notify/templates/${id}`);
}

export function listRecords(params: RecordListQuery): Promise<PageResult<RecordVo>> {
  return request.get<PageResult<RecordVo>>('/api/v1/notify/records', { params });
}

export function sendNotification(data: SendBody): Promise<SendVo> {
  return request.post<SendVo>('/api/v1/notify/send', data, { silent: true });
}
