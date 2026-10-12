/** 服务监控（`monitor:server:*`，：只出指标，不做内嵌控制台）。 */

import { request } from '@/utils/request';

export interface DiskVo {
  name: string;
  mount_point: string;
  fs_type: string;
  total_bytes: number;
  used_bytes: number;
}

export interface ServerInfoVo {
  os_name: string;
  os_version: string;
  hostname: string;
  arch: string;
  cpu_brand: string;
  cpu_cores: number;
  cpu_usage: number;
  mem_total_bytes: number;
  mem_used_bytes: number;
  swap_total_bytes: number;
  swap_used_bytes: number;
  load_avg_1m: number;
  load_avg_5m: number;
  load_avg_15m: number;
  system_uptime_secs: number;
  process_start_unix_secs: number;
  process_uptime_secs: number;
  pid: number;
  version: string;
  build_commit: string;
  disks: DiskVo[];
}

export function getServerInfo(): Promise<ServerInfoVo> {
  return request.get<ServerInfoVo>('/api/v1/monitor/server');
}
