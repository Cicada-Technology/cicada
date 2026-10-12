/**
 * 列表页状态机：查询条件 + 分页 + 加载态 + 增删改后的重载。
 *
 * 抽它的原因很实际——每页都要写一遍「page 归 1、拼 params、try/finally 收 loading、
 * 改完再 load」，十页里会有三种写法，而其中一种一定忘了 `page = 1`。
 */

import { onMounted, ref, type Ref } from 'vue';

import type { PageQuery, PageResult } from '@/api/types';
import { notifySuccess } from '@/utils/feedback';

export interface UseTableOptions<TRow, TQuery extends PageQuery> {
  /** 分页查询函数，通常是 `api/system/xxx.ts` 里的 `listXxx`。 */
  fetcher: (query: TQuery) => Promise<PageResult<TRow>>;
  /** 初始筛选条件（不含 `page` / `page_size`）。 */
  initialQuery?: TQuery;
  pageSize?: number;
  /** 挂载时立即加载；默认 true。 */
  immediate?: boolean;
}

export interface MutateOptions {
  /** 成功提示文案；传空字符串表示不提示。 */
  successMessage?: string;
  /** 成功后是否重新加载列表；默认 true。 */
  reload?: boolean;
}

export function useTable<TRow, TQuery extends PageQuery = PageQuery>(
  options: UseTableOptions<TRow, TQuery>,
) {
  const loading = ref(false);
  const rows = ref([]) as Ref<TRow[]>;
  const total = ref(0);
  const page = ref(1);
  const pageSize = ref(options.pageSize ?? 20);
  const query = ref({ ...(options.initialQuery ?? {}) }) as Ref<TQuery>;

  function params(): TQuery {
    return { ...query.value, page: page.value, page_size: pageSize.value } as TQuery;
  }

  async function load(): Promise<void> {
    loading.value = true;
    try {
      const data = await options.fetcher(params());
      rows.value = data.items;
      total.value = data.total;
    } finally {
      // 失败时保留旧数据（比清空好：用户还能看到刚才在操作什么）。
      loading.value = false;
    }
  }

  /** 条件变化后从第一页开始查。 */
  function search(): Promise<void> {
    page.value = 1;
    return load();
  }

  function reset(): Promise<void> {
    query.value = { ...(options.initialQuery ?? {}) } as TQuery;
    return search();
  }

  function changePage(next: number): Promise<void> {
    page.value = next;
    return load();
  }

  function changePageSize(next: number): Promise<void> {
    pageSize.value = next;
    page.value = 1;
    return load();
  }

  /**
   * 执行一次写操作，成功后提示并重载列表。
   *
   * 不吞异常：失败时向上抛，让调用方决定是否要保留弹层（例如表单校验失败应保持打开）。
   */
  async function mutate<T>(
    action: () => Promise<T>,
    mutateOptions: MutateOptions = {},
  ): Promise<T> {
    const result = await action();
    if (mutateOptions.successMessage !== '') {
      notifySuccess(mutateOptions.successMessage);
    }
    if (mutateOptions.reload !== false) {
      await load();
    }
    return result;
  }

  if (options.immediate !== false) {
    onMounted(load);
  }

  return {
    loading,
    rows,
    total,
    page,
    pageSize,
    query,
    load,
    search,
    reset,
    changePage,
    changePageSize,
    mutate,
  };
}
