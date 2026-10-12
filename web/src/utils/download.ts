/**
 * 浏览器下载、文件选择与剪贴板的小工具。
 *
 * 导出走 `Content-Disposition`（见 `crates/web/src/export.rs`），文件名可能是
 * RFC 5987 的 `filename*=UTF-8''…`（中文名），解析时必须优先它、再退回 `filename=`。
 */

const DEFAULT_FILENAME = 'download';

export function filenameFromDisposition(disposition?: string | null): string | null {
  if (!disposition) return null;
  const extended = /filename\*=UTF-8''([^;]+)/i.exec(disposition);
  if (extended?.[1]) {
    try {
      return decodeURIComponent(extended[1].trim());
    } catch {
      return extended[1].trim();
    }
  }
  const plain = /filename="?([^";]+)"?/i.exec(disposition);
  return plain?.[1] ? plain[1].trim() : null;
}

export function saveBlob(blob: Blob, filename: string = DEFAULT_FILENAME): void {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.rel = 'noopener';
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  // 立刻释放会让部分浏览器取消下载，放到下一轮任务里再回收。
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

/** 触发隐藏的 input[type=file]，返回用户选中的文件（取消则返回 null）。 */
export function pickFile(options: { accept?: string } = {}): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    if (options.accept) input.accept = options.accept;
    input.style.display = 'none';
    document.body.appendChild(input);
    let settled = false;
    const finish = (file: File | null) => {
      if (settled) return;
      settled = true;
      input.remove();
      resolve(file);
    };
    input.addEventListener('change', () => finish(input.files?.[0] ?? null));
    // 用户取消时不会触发 change：窗口重新获得焦点即视为已关闭选择框。
    window.addEventListener(
      'focus',
      () => {
        setTimeout(() => {
          if (!settled) finish(input.files?.[0] ?? null);
        }, 500);
      },
      { once: true },
    );
    input.click();
  });
}

/** 单文件包装成 multipart：后端统一从名为 `file` 的字段读取。 */
export function fileFormData(file: File, field = 'file'): FormData {
  const form = new FormData();
  form.append(field, file, file.name);
  return form;
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
