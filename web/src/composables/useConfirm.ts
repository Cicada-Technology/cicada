/**
 * 破坏性操作的二次确认。
 *
 * 文案里带**对象名**（而不是「确定删除吗？」）：用户在列表里点删除时，最需要确认的
 * 是「删的到底是哪一条」（ux-admin-portal 的明确要求）。
 */

import { ElMessageBox } from 'element-plus';

import { translate } from '@/locales';

export interface ConfirmOptions {
  title?: string;
  confirmText?: string;
  cancelText?: string;
  /** 破坏性操作：主按钮变红。 */
  danger?: boolean;
}

export function useConfirm() {
  async function confirm(message: string, options: ConfirmOptions = {}): Promise<boolean> {
    try {
      await ElMessageBox.confirm(message, options.title ?? translate('common.tip'), {
        confirmButtonText: options.confirmText ?? translate('common.confirm'),
        cancelButtonText: options.cancelText ?? translate('common.cancel'),
        type: options.danger ? 'error' : 'warning',
        // 破坏性确认的默认焦点落在「取消」上：回车不会误删。
        autofocus: false,
        closeOnClickModal: false,
        distinguishCancelAndClose: true,
        customClass: 'ci-confirm',
      });
      return true;
    } catch {
      return false;
    }
  }

  /** 删除确认：文案里带对象名。 */
  function confirmDelete(name: string, options: ConfirmOptions = {}): Promise<boolean> {
    return confirm(translate('common.deleteConfirm', { name }), { danger: true, ...options });
  }

  function confirmBatchDelete(count: number): Promise<boolean> {
    return confirm(translate('common.batchDeleteConfirm', { count }), { danger: true });
  }

  return { confirm, confirmDelete, confirmBatchDelete };
}
