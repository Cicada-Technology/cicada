/** 在线用户（`monitor:online:*`）。会话在 Redis 里，踢人＝删会话，对端下一次请求即 401。 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export interface OnlineUserVo {
  session_id: string;
  user_id: number;
  tenant_id: number;
  user_name: string;
  ip: string;
  user_agent: string;
  /** Unix 秒（不是 RFC3339），展示时用 `formatUnixSeconds`。 */
  login_time: number;
  last_seen: number;
}

export interface OnlineUserListQuery extends PageQuery {
  user_name?: string;
  ip?: string;
}

export function listOnlineUsers(params: OnlineUserListQuery): Promise<PageResult<OnlineUserVo>> {
  return request.get<PageResult<OnlineUserVo>>('/api/v1/monitor/online-users', { params });
}

export function forceLogout(sessionId: string): Promise<void> {
  return request.delete<void>(`/api/v1/monitor/online-users/${encodeURIComponent(sessionId)}`);
}

/** 批量强退：请求体是 `{ session_ids: [...] }`（`DELETE` 带 body）。 */
export function batchForceLogout(sessionIds: string[]): Promise<void> {
  return request.delete<void>('/api/v1/monitor/online-users', {
    data: { session_ids: sessionIds },
  });
}
