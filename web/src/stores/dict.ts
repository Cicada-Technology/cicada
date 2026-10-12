/**
 * 字典缓存（`sys_dict_data`）。
 *
 * 字典是渲染表单用的参考数据，同一个类型会在多个页面/多行里被反复取用，
 * 因此这里做三件事：进程内缓存、并发去重（同一类型只发一次请求）、失败不缓存（可重试）。
 */

import { defineStore } from 'pinia';
import { ref } from 'vue';

import { dictDataByType, type DictDataVo } from '@/api/system/dict';

export interface DictOption {
  label: string;
  value: string;
  /** 列表里的标签样式（`success` / `warning` / …）。 */
  listClass?: string;
  /** 自定义 CSS 类，后端 `css_class`。 */
  cssClass?: string;
  isDefault: boolean;
  raw: DictDataVo;
}

function toOption(item: DictDataVo): DictOption {
  return {
    label: item.dict_label,
    value: item.dict_value,
    listClass: item.list_class ?? undefined,
    cssClass: item.css_class ?? undefined,
    isDefault: item.is_default === 'Y',
    raw: item,
  };
}

export const useDictStore = defineStore('dict', () => {
  const entries = ref<Record<string, DictOption[]>>({});
  /** 正在飞的请求：类型 → Promise，避免同一类型被并发拉取多次。 */
  const inflight = new Map<string, Promise<DictOption[]>>();

  async function load(dictType: string): Promise<DictOption[]> {
    const cached = entries.value[dictType];
    if (cached) return cached;
    const pending = inflight.get(dictType);
    if (pending) return pending;

    const task = dictDataByType(dictType)
      .then((rows) => {
        const options = rows
          .filter((row) => row.status === 0)
          .sort((a, b) => a.dict_sort - b.dict_sort)
          .map(toOption);
        entries.value = { ...entries.value, [dictType]: options };
        return options;
      })
      .catch(() => {
        // 拿不到字典不该让整个页面崩：降级为空选项，且**不写缓存**以便下次重试。
        return [] as DictOption[];
      })
      .finally(() => {
        inflight.delete(dictType);
      });

    inflight.set(dictType, task);
    return task;
  }

  function options(dictType: string): DictOption[] {
    return entries.value[dictType] ?? [];
  }

  function labelOf(dictType: string, value: unknown): string | undefined {
    const target = String(value ?? '');
    return options(dictType).find((option) => option.value === target)?.label;
  }

  function classOf(dictType: string, value: unknown): string | undefined {
    const target = String(value ?? '');
    return options(dictType).find((option) => option.value === target)?.listClass;
  }

  function clear(): void {
    entries.value = {};
  }

  return { entries, load, options, labelOf, classOf, clear };
});
