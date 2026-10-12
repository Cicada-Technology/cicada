/**
 * 展示层格式化。
 *
 * 约定：只做展示，不做业务判断。后端时间字段是 RFC3339 字符串（`to_rfc3339()`），
 * 空值与无效值一律显示占位符，不显示 `Invalid Date`。
 */

import dayjs from 'dayjs';
import 'dayjs/locale/zh-cn';
import relativeTime from 'dayjs/plugin/relativeTime';

dayjs.extend(relativeTime);
dayjs.locale('zh-cn');

export const EMPTY_PLACEHOLDER = '—';

export function formatDateTime(value?: string | number | null): string {
  if (value === null || value === undefined || value === '') return EMPTY_PLACEHOLDER;
  const parsed = dayjs(value);
  return parsed.isValid() ? parsed.format('YYYY-MM-DD HH:mm:ss') : EMPTY_PLACEHOLDER;
}

export function formatDate(value?: string | number | null): string {
  if (value === null || value === undefined || value === '') return EMPTY_PLACEHOLDER;
  const parsed = dayjs(value);
  return parsed.isValid() ? parsed.format('YYYY-MM-DD') : EMPTY_PLACEHOLDER;
}

/** 在线用户接口给的是 Unix 秒而非 RFC3339，单独一个入口避免用错。 */
export function formatUnixSeconds(value?: number | null): string {
  if (!value) return EMPTY_PLACEHOLDER;
  return formatDateTime(value * 1000);
}

export function formatRelative(value?: string | number | null): string {
  if (value === null || value === undefined || value === '') return EMPTY_PLACEHOLDER;
  const parsed = dayjs(value);
  return parsed.isValid() ? parsed.fromNow() : EMPTY_PLACEHOLDER;
}

export function formatBytes(bytes?: number | null): string {
  if (!bytes || bytes <= 0) return '0 B';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value >= 100 || exponent === 0 ? Math.round(value) : value.toFixed(1)} ${units[exponent]}`;
}

/** 操作日志耗时：毫秒到人读单位。 */
export function formatDuration(ms?: number | null): string {
  if (ms === null || ms === undefined) return EMPTY_PLACEHOLDER;
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(2)} s`;
  return `${(ms / 60_000).toFixed(1)} min`;
}

/** 秒到「3 天 4 小时」；缓存的 TTL、任务超时常见。 */
export function formatSeconds(seconds?: number | null): string {
  if (!seconds || seconds <= 0) return EMPTY_PLACEHOLDER;
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days} 天 ${hours} 小时`;
  if (hours > 0) return `${hours} 小时 ${minutes} 分`;
  if (minutes > 0) return `${minutes} 分 ${seconds % 60} 秒`;
  return `${seconds} 秒`;
}

export function formatNumber(value?: number | null): string {
  if (value === null || value === undefined) return EMPTY_PLACEHOLDER;
  return value.toLocaleString('zh-CN');
}

/** 语言无关的空值处理，表格里大量使用。 */
export function orPlaceholder(value?: string | number | null): string {
  if (value === null || value === undefined || value === '') return EMPTY_PLACEHOLDER;
  return String(value);
}
