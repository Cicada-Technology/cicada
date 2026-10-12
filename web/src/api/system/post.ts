/** 岗位管理 API（最简单的分页 CRUD，可作为新增模块的模仿样板）。 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export interface PostVo {
  post_id: number;
  post_code: string;
  post_name: string;
  post_sort: number;
  status: number;
  remark?: string | null;
  create_time: string;
}

export interface PostListQuery extends PageQuery {
  post_code?: string;
  post_name?: string;
  status?: number;
}

export interface PostUpsert {
  post_code: string;
  post_name: string;
  post_sort: number;
  status: number;
  remark?: string | null;
}

export function listPosts(params: PostListQuery): Promise<PageResult<PostVo>> {
  return request.get<PageResult<PostVo>>('/api/v1/system/posts', { params });
}

export function createPost(data: PostUpsert): Promise<PostVo> {
  return request.post<PostVo>('/api/v1/system/posts', data);
}

export function updatePost(postId: number, data: PostUpsert): Promise<PostVo> {
  return request.put<PostVo>(`/api/v1/system/posts/${postId}`, data);
}

export function deletePost(postId: number): Promise<void> {
  return request.delete<void>(`/api/v1/system/posts/${postId}`);
}

export function exportPosts(params: PostListQuery): Promise<void> {
  return request.download('/api/v1/system/posts/export', { config: { params } });
}
