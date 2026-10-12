/**
 * 定时任务（`monitor:job:*`）。
 *
 * 集群下由分布式锁选主执行，因此「启动/暂停」与「立即执行一次」是两种
 * 不同语义：前者改状态位（持久），后者触发单次运行（不入状态）。
 */

import type { PageQuery, PageResult } from '@/api/types';
import { request } from '@/utils/request';

export const JOB_CONCURRENCY_OPTIONS = [
  { value: 0, label: '允许并发' },
  { value: 1, label: '禁止并发' },
] as const;

export interface JobVo {
  job_id: number;
  job_name: string;
  job_group: string;
  invoke_target: string;
  cron_expression: string;
  /** 0 允许并发 1 禁止并发。 */
  concurrency: number;
  timeout_secs: number;
  max_retry: number;
  /** 0 正常 1 暂停。 */
  status: number;
  remark?: string | null;
  create_time: string;
}

export interface JobListQuery extends PageQuery {
  job_name?: string;
  job_group?: string;
  status?: number;
}

export interface JobUpsert {
  job_name: string;
  job_group: string;
  invoke_target: string;
  cron_expression: string;
  concurrency: number;
  timeout_secs: number;
  max_retry: number;
  status: number;
  remark?: string | null;
}

export interface JobLogVo {
  job_log_id: number;
  job_id: number;
  job_name: string;
  job_group: string;
  invoke_target: string;
  instance_id: string;
  /** 触发方式：1 定时触发 2 手动触发。 */
  trigger_kind: number;
  start_time: string;
  end_time?: string | null;
  duration_ms?: number | null;
  /** 0 成功 1 失败。 */
  status: number;
  result?: string | null;
  trace_id?: string | null;
}

export interface JobLogListQuery extends PageQuery {
  job_name?: string;
  status?: number;
}

export function listJobs(params: JobListQuery): Promise<PageResult<JobVo>> {
  return request.get<PageResult<JobVo>>('/api/v1/monitor/jobs', { params });
}

export function createJob(data: JobUpsert): Promise<JobVo> {
  return request.post<JobVo>('/api/v1/monitor/jobs', data);
}

export function updateJob(jobId: number, data: JobUpsert): Promise<JobVo> {
  return request.put<JobVo>(`/api/v1/monitor/jobs/${jobId}`, data);
}

export function deleteJob(jobId: number): Promise<void> {
  return request.delete<void>(`/api/v1/monitor/jobs/${jobId}`);
}

export function changeJobStatus(jobId: number, status: number): Promise<void> {
  return request.put<void>(`/api/v1/monitor/jobs/${jobId}/status`, { status });
}

/** 立即执行一次（不影响调度状态）。 */
export function runJobOnce(jobId: number): Promise<void> {
  return request.put<void>(`/api/v1/monitor/jobs/${jobId}/run`);
}

export function listJobLogs(jobId: number, params: JobLogListQuery): Promise<PageResult<JobLogVo>> {
  return request.get<PageResult<JobLogVo>>(`/api/v1/monitor/jobs/${jobId}/logs`, { params });
}

export function exportJobs(params: JobListQuery): Promise<void> {
  return request.download('/api/v1/monitor/jobs/export', { config: { params } });
}
