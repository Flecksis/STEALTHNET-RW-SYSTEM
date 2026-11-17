export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

type Options = {
  method?: Method;
  body?: unknown;
  signal?: AbortSignal;
  keepalive?: boolean;
};

function errorFrom(status: number, data: unknown): ApiError {
  if (data && typeof data === 'object' && 'error' in data) {
    const err = (data as { error?: { code?: unknown; message?: unknown } }).error;
    const code = typeof err?.code === 'string' ? err.code : `http.${status}`;
    const message = typeof err?.message === 'string' ? err.message : `Ошибка ${status}`;
    return new ApiError(status, code, message);
  }
  if (typeof data === 'string' && data.trim()) {
    return new ApiError(status, `http.${status}`, data.trim());
  }
  return new ApiError(status, `http.${status}`, `Ошибка ${status}`);
}

let bearer: string | null = null;

export function setBearer(token: string | null): void {
  bearer = token;
}

export async function api<T>(path: string, options: Options = {}): Promise<T> {
  const headers: Record<string, string> = { accept: 'application/json' };
  if (bearer && path.startsWith('/api/client')) {
    headers.authorization = `Bearer ${bearer}`;
  }
  let body: string | undefined;
  if (options.body !== undefined) {
    headers['content-type'] = 'application/json';
    body = JSON.stringify(options.body);
  }
  let response: Response;
  try {
    response = await fetch(path, {
      method: options.method ?? 'GET',
      headers,
      body,
      credentials: 'same-origin',
      signal: options.signal,
      keepalive: options.keepalive,
    });
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === 'AbortError') {
      throw cause;
    }
    throw new ApiError(0, 'network', 'Нет связи с сервером. Проверьте интернет и попробуйте ещё раз');
  }
  const text = await response.text();
  let data: unknown = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
  }
  if (!response.ok) {
    throw errorFrom(response.status, data);
  }
  return data as T;
}
