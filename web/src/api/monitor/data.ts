/** 数据监控：PostgreSQL 数据库与表的只读统计。 */

import { request } from '@/utils/request';

export interface DataDatabaseVo {
  name: string;
  version: string;
  schema: string;
  size_bytes: number;
  table_count: number;
  connections: number;
  max_connections: number;
  started_at: string;
  uptime_seconds: number;
}

export interface DataPoolVo {
  total: number;
  idle: number;
  max: number;
}

export interface TableStatVo {
  schema: string;
  table: string;
  row_count: number;
  total_bytes: number;
  table_bytes: number;
  index_bytes: number;
  seq_scan: number;
  idx_scan: number;
  dead_rows: number;
  last_vacuum?: string | null;
  last_analyze?: string | null;
}

export interface DataMonitorVo {
  database: DataDatabaseVo;
  pool: DataPoolVo;
  tables: TableStatVo[];
  collected_at: string;
}

export function getDataMonitor(): Promise<DataMonitorVo> {
  return request.get<DataMonitorVo>('/api/v1/monitor/data');
}
