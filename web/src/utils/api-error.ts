/**
 * API 错误的统一形状。
 *
 * 后端所有失败响应都是 RFC 9457 的 `application/problem+json`：`type` / `title` /
 * `status` / `detail` / `code` 必有，`instance` / `request_id` 可选，
 * `errors` 只在字段级校验（`CICADA_VALIDATION_4001`）时出现。
 */

export interface ProblemFieldError {
  field: string;
  message: string;
}

export interface ProblemDetails {
  type?: string;
  title?: string;
  status?: number;
  detail?: string;
  instance?: string;
  /** 机器可读业务码，如 `CICADA_RBAC_2001`。 */
  code?: string;
  request_id?: string;
  /** 字段级校验错误。 */
  errors?: ProblemFieldError[];
}

export class ApiError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly requestId?: string;
  readonly fields: ProblemFieldError[];
  /** 原始错误体，调试时才需要。 */
  readonly problem: ProblemDetails | null;
  /** 原始异常（axios 的 `AxiosError`）。ES2022 的 `Error.cause` 已经存在，这里是覆写。 */
  override readonly cause?: unknown;

  constructor(
    message: string,
    options: {
      status?: number;
      code?: string;
      requestId?: string;
      fields?: ProblemFieldError[];
      problem?: ProblemDetails | null;
      cause?: unknown;
    } = {},
  ) {
    super(message);
    this.name = 'ApiError';
    this.status = options.status ?? 0;
    this.code = options.code;
    this.requestId = options.requestId;
    this.fields = options.fields ?? [];
    this.problem = options.problem ?? null;
    this.cause = options.cause;
  }

  get isUnauthorized(): boolean {
    return this.status === 401;
  }

  get isForbidden(): boolean {
    return this.status === 403;
  }

  get isNotFound(): boolean {
    return this.status === 404;
  }

  /** 字段级校验错误：业务码目录的 `CICADA_VALIDATION_4001` 会带 `errors[]`。 */
  get isValidation(): boolean {
    return this.code === 'CICADA_VALIDATION_4001' || this.fields.length > 0;
  }

  /** 表单用来把服务端校验结果贴到对应字段上。 */
  fieldMessage(field: string): string | undefined {
    return this.fields.find((item) => item.field === field)?.message;
  }

  /** 字段名到消息的映射，直接喂给 `AppForm` 的 `serverErrors`。 */
  get fieldMap(): Record<string, string> {
    const map: Record<string, string> = {};
    for (const item of this.fields) {
      // 同一字段多条错误时保留第一条：表单只显示一行。
      if (!(item.field in map)) map[item.field] = item.message;
    }
    return map;
  }
}

/**
 * 从 problem+json 错误体里抽出人可读的文案。
 *
 * 优先级：`detail`（本次 occurrence 的细节，已按 `Accept-Language` 本地化）→
 * `errors[0].message`（字段级校验）→ `title`（业务码固定标题，未翻译）。
 */
export function readProblemMessage(body: unknown): string | undefined {
  if (!body || typeof body !== 'object') return undefined;
  const problem = body as ProblemDetails;
  const candidates = [problem.detail, problem.errors?.[0]?.message, problem.title];
  for (const candidate of candidates) {
    if (typeof candidate === 'string' && candidate.trim() !== '') return candidate;
  }
  return undefined;
}
