/**
 * HTTP 通道（「API 层按模块生成」的前置基础设施）。
 *
 * 这里是**唯一**碰 axios 的地方，向上只暴露 `request.get/post/put/delete/upload/download`。
 * 代码生成器产出的 `web/src/api/<module>/<name>.ts` 与手写 API 模块都按同一套约定调用：
 *
 * ```ts
 * export function listGoods(params: GoodsQuery): Promise<PageResult<GoodsVo>> {
 *   return request.get<PageResult<GoodsVo>>('/business/goods', { params });
 * }
 * ```
 *
 * 职责边界：
 * - 令牌注入与 401 刷新重放：放在拦截器里，业务代码永远不写「取 token、拼 header」。
 * - 错误归一：所有失败都收敛成 `ApiError`，调用方只需要 `catch` 一次。
 * - 默认弹一次错误提示（`silent: true` 可关），因为生成器产出的页面不写 try/catch，
 *   没有全局兜底就会出现「点了没反应」。
 */

import axios, {
  AxiosError,
  type AxiosInstance,
  type AxiosRequestConfig,
  type AxiosResponse,
  type InternalAxiosRequestConfig,
} from 'axios';

import { currentLocale } from '@/locales';
import { ApiError, readProblemMessage, type ProblemDetails } from './api-error';
import { notifyApiError } from './feedback';
import { fileFormData, filenameFromDisposition, saveBlob } from './download';
import { clearSession, getAccessToken, getRefreshToken, setTokens } from './session';

declare module 'axios' {
  /* eslint-disable @typescript-eslint/no-explicit-any, @typescript-eslint/no-unused-vars --
     声明合并要求类型参数名与默认值逐字一致：`D` 在本接口里用不到也必须保留。 */
  interface AxiosRequestConfig<D = any> {
    /** 静默失败：不弹全局错误提示，由调用方自己处理（如登录页显示内联错误）。 */
    silent?: boolean;
  }
  interface InternalAxiosRequestConfig<D = any> {
    silent?: boolean;
  }
  /* eslint-enable @typescript-eslint/no-explicit-any, @typescript-eslint/no-unused-vars */
}

/** 与前端同源时留空（生产由 Nginx 反代 /api 与 /auth）。 */
const BASE_URL = import.meta.env.VITE_API_BASE_URL ?? '';
const TIMEOUT_MS = 30_000;

/** 这些路径 401 是「凭据不对」而不是「令牌过期」，不能触发刷新重放。 */
const UNAUTHENTICATED_PATHS = ['/auth/login', '/auth/refresh'];

interface RefreshResponse {
  access_token: string;
  refresh_token: string;
}

type RetryableConfig = InternalAxiosRequestConfig & { __cicadaRetried?: boolean };

function toApiError(error: AxiosError): ApiError {
  if (error.code === 'ECONNABORTED' || error.code === 'ETIMEDOUT') {
    return new ApiError('请求超时，请稍后重试', { status: 0, cause: error });
  }
  const response = error.response;
  if (!response) {
    return new ApiError('网络异常，请检查连接后重试', { status: 0, cause: error });
  }
  const problem = (response.data ?? null) as ProblemDetails | null;
  return new ApiError(readProblemMessage(problem) ?? response.statusText ?? '请求失败', {
    status: response.status,
    code: problem?.code,
    requestId: problem?.request_id,
    fields: problem?.errors ?? [],
    problem,
    cause: error,
  });
}

const http: AxiosInstance = axios.create({
  baseURL: BASE_URL,
  timeout: TIMEOUT_MS,
  headers: { Accept: 'application/json' },
});

http.interceptors.request.use((config) => {
  const token = getAccessToken();
  if (token) {
    config.headers.set('Authorization', `Bearer ${token}`);
  }
  // 后端 rust-i18n 按此头协商错误消息语言，与 UI 语言保持一致。
  config.headers.set('Accept-Language', currentLocale());
  // 不发送租户头：租户由令牌推导，前端不参与租户判定。
  return config;
});

/**
 * 刷新令牌。并发请求只发一次：`refreshInFlight` 让后来者复用同一个 Promise。
 * 刷新令牌是**轮换**的，重复消费会触发 RefreshReplay 而被判为异常。
 */
let refreshInFlight: Promise<string> | null = null;

async function requestNewToken(): Promise<string> {
  const refreshToken = getRefreshToken();
  if (!refreshToken) {
    throw new ApiError('会话已过期', { status: 401 });
  }
  const response = await axios.post<RefreshResponse>(
    `${BASE_URL}/auth/refresh`,
    { refresh_token: refreshToken },
    {
      timeout: TIMEOUT_MS,
      headers: { Accept: 'application/json', 'Accept-Language': currentLocale() },
    },
  );
  setTokens({
    accessToken: response.data.access_token,
    refreshToken: response.data.refresh_token,
  });
  return response.data.access_token;
}

function ensureRefreshed(): Promise<string> {
  refreshInFlight ??= requestNewToken().finally(() => {
    refreshInFlight = null;
  });
  return refreshInFlight;
}

function isUnauthenticatedPath(url: string | undefined): boolean {
  if (!url) return false;
  return UNAUTHENTICATED_PATHS.some((path) => url.startsWith(path));
}

/**
 * 会话被动终止（刷新令牌过期、被强退、改密）时的唯一出口。
 *
 * 这里只做「清会话 + 广播」，提示与跳转交给应用层（`main.ts::handleSessionLost`）——
 * 拦截器里拿不到路由与 store，硬跳 `location.href` 会丢掉 SPA 状态。
 */
function terminateSession(): void {
  clearSession('expired');
}

http.interceptors.response.use(
  (response) => response,
  async (error: AxiosError) => {
    const config = error.config as RetryableConfig | undefined;
    const status = error.response?.status ?? 0;
    /** 这次失败是否已经走到「会话终止」：那种情况下提示由上层的会话收尾负责，不在这里重复。 */
    let sessionTerminated = false;

    // 令牌过期：刷新一次并重放原请求。登录/刷新接口自身不参与，否则会自锁。
    if (status === 401 && config && !config.__cicadaRetried && !isUnauthenticatedPath(config.url)) {
      config.__cicadaRetried = true;
      try {
        const token = await ensureRefreshed();
        config.headers.set('Authorization', `Bearer ${token}`);
        return await http.request(config);
      } catch {
        terminateSession();
        sessionTerminated = true;
      }
    } else if (status === 401 && !isUnauthenticatedPath(config?.url)) {
      terminateSession();
      sessionTerminated = true;
    }

    const apiError = toApiError(error);
    // 会话刚失效时不再叠一条「请求失败」：用户接下来要做的是重新登录，
    // 两条提示并排出现只会让人以为系统坏了。
    if (!sessionTerminated && !config?.silent) {
      notifyApiError(apiError);
    }
    return Promise.reject(apiError);
  },
);

async function unwrap<T>(pending: Promise<AxiosResponse<T>>): Promise<T> {
  const response = await pending;
  return response.data;
}

export interface DownloadOptions {
  /** 不传则用响应头里的文件名，再退回 `download`。 */
  filename?: string;
  method?: 'GET' | 'POST';
  config?: AxiosRequestConfig;
}

export interface RequestClient {
  /** 少数需要读响应头/状态的场景才用它（常规调用请用下面几个方法）。 */
  readonly raw: AxiosInstance;
  request<T = void>(config: AxiosRequestConfig): Promise<T>;
  get<T = void>(url: string, config?: AxiosRequestConfig): Promise<T>;
  post<T = void>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<T>;
  put<T = void>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<T>;
  patch<T = void>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<T>;
  delete<T = void>(url: string, config?: AxiosRequestConfig): Promise<T>;
  upload<T = void>(url: string, file: File, config?: AxiosRequestConfig): Promise<T>;
  /** 触发浏览器下载；导出接口返回 attachment，文件名从响应头读取。 */
  download(url: string, options?: DownloadOptions): Promise<void>;
  /** 取二进制内容而不落盘（预览、图片直出）。 */
  blob(url: string, config?: AxiosRequestConfig): Promise<Blob>;
}

export const request: RequestClient = {
  raw: http,
  request: <T = void>(config: AxiosRequestConfig) => unwrap<T>(http.request<T>(config)),
  get: <T = void>(url: string, config?: AxiosRequestConfig) => unwrap<T>(http.get<T>(url, config)),
  post: <T = void>(url: string, data?: unknown, config?: AxiosRequestConfig) =>
    unwrap<T>(http.post<T>(url, data, config)),
  put: <T = void>(url: string, data?: unknown, config?: AxiosRequestConfig) =>
    unwrap<T>(http.put<T>(url, data, config)),
  patch: <T = void>(url: string, data?: unknown, config?: AxiosRequestConfig) =>
    unwrap<T>(http.patch<T>(url, data, config)),
  delete: <T = void>(url: string, config?: AxiosRequestConfig) =>
    unwrap<T>(http.delete<T>(url, config)),
  upload: <T = void>(url: string, file: File, config?: AxiosRequestConfig) =>
    unwrap<T>(http.post<T>(url, fileFormData(file), config)),
  async download(url: string, options: DownloadOptions = {}) {
    const method = options.method ?? 'GET';
    const response = await http.request<Blob>({
      ...options.config,
      url,
      method,
      responseType: 'blob',
    });
    const header = response.headers['content-disposition'];
    const filename =
      options.filename ??
      filenameFromDisposition(typeof header === 'string' ? header : undefined) ??
      'download';
    saveBlob(response.data, filename);
  },
  async blob(url: string, config?: AxiosRequestConfig) {
    const response = await http.get<Blob>(url, { ...config, responseType: 'blob' });
    return response.data;
  },
};

/** 组装带 query 的路径；列表接口的 `undefined`/空字符串一律丢掉，避免后端收到空过滤条件。 */
export function withQuery(
  url: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): string {
  if (!params) return url;
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === '') continue;
    search.append(key, String(value));
  }
  const query = search.toString();
  return query ? `${url}?${query}` : url;
}
