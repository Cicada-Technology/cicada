/** 通知公告 API。`notice_type` 1 通知 / 2 公告；`status` 0 正常 1 关闭。 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export const NOTICE_TYPE_NOTIFICATION = 1;
export const NOTICE_TYPE_ANNOUNCEMENT = 2;

export const NOTICE_TYPE_OPTIONS = [
  { value: NOTICE_TYPE_NOTIFICATION, label: '通知' },
  { value: NOTICE_TYPE_ANNOUNCEMENT, label: '公告' },
] as const;

export interface NoticeVo {
  notice_id: number;
  notice_title: string;
  notice_type: number;
  notice_content?: string | null;
  status: number;
  remark?: string | null;
  create_time: string;
}

export interface NoticeListQuery extends PageQuery {
  notice_title?: string;
  notice_type?: number;
  status?: number;
}

export interface NoticeUpsert {
  notice_title: string;
  notice_type: number;
  notice_content?: string | null;
  status: number;
  remark?: string | null;
}

export function listNotices(params: NoticeListQuery): Promise<PageResult<NoticeVo>> {
  return request.get<PageResult<NoticeVo>>('/api/v1/system/notices', { params });
}

export function getNotice(noticeId: number): Promise<NoticeVo> {
  return request.get<NoticeVo>(`/api/v1/system/notices/${noticeId}`);
}

export function createNotice(data: NoticeUpsert): Promise<NoticeVo> {
  return request.post<NoticeVo>('/api/v1/system/notices', data);
}

export function updateNotice(noticeId: number, data: NoticeUpsert): Promise<NoticeVo> {
  return request.put<NoticeVo>(`/api/v1/system/notices/${noticeId}`, data);
}

export function deleteNotice(noticeId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/notices/${noticeId}`);
}

/** 标记已读：个人维度，不需要 `system:notice:edit`。 */
export function markNoticeRead(noticeId: number): Promise<void> {
  return request.post<void>(`/api/v1/system/notices/${noticeId}/read`);
}
