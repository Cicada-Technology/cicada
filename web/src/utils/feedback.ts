/**
 * 统一的用户反馈入口。
 *
 * 为什么要包一层：错误提示在拦截器、指令、页面三处都会用到，
 * 直接散落 `ElMessage.error(...)` 会让文案风格与 i18n 覆盖度逐渐漂移。
 */

import { ElMessage } from 'element-plus';

import { translate } from '@/locales';

import { ApiError } from './api-error';

export function notifySuccess(message?: string): void {
  ElMessage.success({ message: message ?? translate('common.success'), grouping: true });
}

export function notifyError(message: string): void {
  ElMessage.error({ message, grouping: true });
}

export function notifyWarning(message: string): void {
  ElMessage.warning({ message, grouping: true });
}

export function notifyInfo(message: string): void {
  ElMessage.info({ message, grouping: true });
}

/** 把一次请求失败翻译成一句人话；422 的字段级错误留给表单，不在这里堆砌。 */
export function describeApiError(error: ApiError): string {
  if (error.status === 0) return translate('common.networkError');
  if (error.status === 403) return error.message || translate('common.forbidden');
  if (error.status === 429) return error.message || '请求过于频繁，请稍后再试';
  return error.message || translate('common.failed');
}

/** 请求失败时的兜底提示：只在调用方没有显式声明 `silent` 时触发。 */
export function notifyApiError(error: ApiError): void {
  notifyError(describeApiError(error));
}
