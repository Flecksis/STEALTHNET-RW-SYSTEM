export type DocKind = 'link' | 'page';

export type DocRow = {
  id: string;
  kind: DocKind;
  title: string;
  title_en: string | null;
  url: string | null;
  path: string | null;
  english: boolean;
  updated_at: string | null;
};

export type DocsView = {
  consent: boolean;
  items: DocRow[];
  max: number;
  title_max: number;
  url_max: number;
  path_max: number;
  body_max: number;
  base_url: string | null;
};

export type DocFull = {
  id: string;
  kind: DocKind;
  title: string;
  title_en: string | null;
  url: string | null;
  path: string | null;
  link: string | null;
  body_ru: string | null;
  body_en: string | null;
  updated_at: string | null;
};

export const DOCS_API = '/api/admin/settings/documents';
export const DOCS_PAGE = '/admin/settings/documents';

export function hostOf(base: string | null): string | null {
  if (!base) {
    return null;
  }
  return base.replace(/^https?:\/\//, '').replace(/\/+$/, '');
}

export function listItem(row: DocRow): Record<string, string | null> {
  return { id: row.id, title: row.title, title_en: row.title_en, url: row.url, path: row.path };
}

const RESERVED = ['admin', 'api', 'app', 'assets', 'cabinet', 'm', 'pay', 'sub', 'verify', 'sw.js', 'service-worker.js', 'robots.txt', 'index.html', 'favicon.ico'];

export function normalizePath(raw: string): string {
  return raw.trim().replace(/^\/+/, '').replace(/\/+$/, '').toLowerCase();
}

export function pathProblem(raw: string, max: number): string | null {
  const value = normalizePath(raw);
  if (!value) {
    return 'нужен адрес, например privacy-policy';
  }
  const first = value.split('/')[0] ?? '';
  if (RESERVED.includes(first)) {
    return `адрес /${first} занят панелью`;
  }
  if (!/^[a-z0-9_/-]+$/.test(value)) {
    return 'только латинские буквы, цифры, дефис, подчёркивание и /';
  }
  if (!value.split('/').every((part) => /^[a-z0-9](?:[a-z0-9_-]*[a-z0-9])?$/.test(part))) {
    return 'части адреса начинаются и заканчиваются буквой или цифрой';
  }
  if (value.split('/').length > 3) {
    return 'не больше трёх частей через /';
  }
  if (value.length + 1 > max) {
    return `не длиннее ${max} символов`;
  }
  return null;
}

export function urlProblem(raw: string): string | null {
  const value = raw.trim();
  if (!value) {
    return 'нужна ссылка';
  }
  if (!/^https:\/\/[^\s"'<>]+$/.test(value) || value.length <= 'https://'.length) {
    return 'ссылка вида https://…';
  }
  return null;
}

export function titleProblem(raw: string, max: number, required: boolean): string | null {
  const value = raw.trim();
  if (!value) {
    return required ? 'нужно название' : null;
  }
  return value.length > max ? `не длиннее ${max} символов` : null;
}
