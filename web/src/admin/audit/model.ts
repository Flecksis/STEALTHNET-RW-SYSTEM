export const AUDIT = '/api/admin/audit';
export const PAGE = '/admin/audit';
export const PAGE_SIZE = 50;

export type Actor =
  | { kind: 'admin'; id: number; email: string | null; role: string | null }
  | { kind: 'api_key'; id: number; name: string | null; owner: string | null }
  | { kind: 'module'; id: string; name: string | null }
  | { kind: 'client'; id: number; name: string }
  | { kind: 'system' };

export type Target = { kind: string; label: string; note: string | null; link: string | null; client_id: number | null };
export type Change = { label: string; before: string; after: string };
export type Field = { key: string; label: string | null; value: string };

export type AuditItem = {
  id: number;
  at: string;
  action: string;
  title: string;
  area: string | null;
  known: boolean;
  actor: Actor;
  via_unknown: boolean;
  target: Target | null;
  changes: Change[];
  fields: Field[];
  ip: string | null;
};

export type AuditPage = { items: AuditItem[]; next: string | null; client: Target | null };

export type MetaArea = { id: string; title: string; actions: Array<{ code: string; title: string }> };

export type AuditMeta = {
  admins: Array<{ id: number; email: string; role: string; disabled: boolean }>;
  keys: Array<{ id: number; name: string; owner: string | null; revoked: boolean }>;
  modules: Array<{ id: string; name: string }>;
  areas: MetaArea[];
  search_min: number;
};

export type ClientChoice = { id: number; label: string; note: string | null };

export type Period = 'today' | 'week' | 'month' | 'all' | 'custom';

export const PERIODS: Array<[Period, string]> = [
  ['today', 'Сегодня'],
  ['week', '7 дней'],
  ['month', '30 дней'],
  ['all', 'Всё время'],
  ['custom', 'Свои даты'],
];

export type Filters = {
  actor: string;
  what: string;
  client: string;
  period: Period;
  since: string;
  until: string;
  q: string;
};

export const EMPTY: Filters = { actor: '', what: '', client: '', period: 'all', since: '', until: '', q: '' };

const PERIOD_KEYS = new Set<string>(PERIODS.map(([key]) => key));
const ACTOR_RE = /^(system|clients|admin:\d+|key:\d+|client:\d+|module:[a-z0-9-]{1,40})$/;
const WHAT_RE = /^(area:[a-z_]+|action:[a-z0-9_.]{1,80})$/;
const DAY_RE = /^\d{4}-\d{2}-\d{2}$/;

export function filtersOf(params: URLSearchParams): Filters {
  const actor = params.get('actor') ?? '';
  const what = params.get('what') ?? '';
  const client = params.get('client') ?? '';
  const period = params.get('period') ?? 'all';
  const since = params.get('since') ?? '';
  const until = params.get('until') ?? '';
  return {
    actor: ACTOR_RE.test(actor) ? actor : '',
    what: WHAT_RE.test(what) ? what : '',
    client: /^\d{1,18}$/.test(client) ? client : '',
    period: PERIOD_KEYS.has(period) ? (period as Period) : 'all',
    since: DAY_RE.test(since) ? since : '',
    until: DAY_RE.test(until) ? until : '',
    q: (params.get('q') ?? '').slice(0, 200),
  };
}

export function paramsOf(filters: Filters): URLSearchParams {
  const params = new URLSearchParams();
  if (filters.actor) {
    params.set('actor', filters.actor);
  }
  if (filters.what) {
    params.set('what', filters.what);
  }
  if (filters.client) {
    params.set('client', filters.client);
  }
  if (filters.period !== 'all') {
    params.set('period', filters.period);
  }
  if (filters.period === 'custom') {
    if (filters.since) {
      params.set('since', filters.since);
    }
    if (filters.until) {
      params.set('until', filters.until);
    }
  }
  if (filters.q.trim()) {
    params.set('q', filters.q.trim());
  }
  return params;
}

export function filterCount(filters: Filters): number {
  return [filters.actor, filters.what, filters.client, filters.period !== 'all' ? 'p' : '', filters.q.trim()].filter(Boolean).length;
}

function localDay(value: string, shift = 0): string | null {
  const match = DAY_RE.exec(value) ? value.split('-').map(Number) : null;
  if (!match) {
    return null;
  }
  const [year, month, day] = match as [number, number, number];
  const date = new Date(year, month - 1, day + shift);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

function startOfToday(now: number): Date {
  const date = new Date(now);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

export function rangeOf(filters: Filters, now = Date.now()): { since: string | null; until: string | null } {
  switch (filters.period) {
    case 'today':
      return { since: startOfToday(now).toISOString(), until: null };
    case 'week': {
      const start = startOfToday(now);
      start.setDate(start.getDate() - 6);
      return { since: start.toISOString(), until: null };
    }
    case 'month': {
      const start = startOfToday(now);
      start.setDate(start.getDate() - 29);
      return { since: start.toISOString(), until: null };
    }
    case 'custom':
      return { since: localDay(filters.since), until: localDay(filters.until, 1) };
    default:
      return { since: null, until: null };
  }
}

export function queryOf(filters: Filters, cursor: string | null, now = Date.now()): string {
  const params = new URLSearchParams({ limit: String(PAGE_SIZE) });
  if (filters.actor) {
    params.set('actor', filters.actor);
  }
  if (filters.what.startsWith('area:')) {
    params.set('area', filters.what.slice(5));
  }
  if (filters.what.startsWith('action:')) {
    params.set('action', filters.what.slice(7));
  }
  if (filters.client) {
    params.set('client', filters.client);
  }
  const range = rangeOf(filters, now);
  if (range.since) {
    params.set('since', range.since);
  }
  if (range.until) {
    params.set('until', range.until);
  }
  if (filters.q.trim()) {
    params.set('q', filters.q.trim());
  }
  if (cursor) {
    params.set('cursor', cursor);
  }
  return `${AUDIT}?${params.toString()}`;
}

export function actorName(actor: Actor): string {
  switch (actor.kind) {
    case 'admin':
      return actor.email ?? `Администратор №${actor.id}`;
    case 'api_key':
      return actor.name ? `Ключ API «${actor.name}»` : `Ключ API №${actor.id}`;
    case 'module':
      return `Модуль «${actor.name ?? actor.id}»`;
    case 'client':
      return actor.name;
    case 'system':
      return 'Панель, автоматически';
  }
}

export function actorShort(actor: Actor): string {
  switch (actor.kind) {
    case 'api_key':
      return actor.name ?? `№${actor.id}`;
    case 'module':
      return actor.name ?? actor.id;
    case 'system':
      return 'Панель';
    default:
      return actorName(actor);
  }
}

export function actorNote(actor: Actor): string {
  switch (actor.kind) {
    case 'admin':
      return actor.role ?? 'Администратор';
    case 'api_key':
      return actor.owner ? `ключ API, выпущен ${actor.owner}` : 'ключ API';
    case 'module':
      return 'внешний модуль';
    case 'client':
      return 'клиент';
    case 'system':
      return 'автоматически';
  }
}

export function actorKind(actor: Actor): string {
  return actor.kind === 'api_key' ? 'ключ API' : actorNote(actor);
}

export function actorKey(actor: Actor): string {
  switch (actor.kind) {
    case 'admin':
      return `admin:${actor.id}`;
    case 'api_key':
      return `key:${actor.id}`;
    case 'module':
      return `module:${actor.id}`;
    case 'client':
      return `client:${actor.id}`;
    case 'system':
      return 'system';
  }
}

export function actorFilterLabel(value: string, meta: AuditMeta | null, client?: Target | null): string {
  if (!value) {
    return 'Все участники';
  }
  if (value === 'system') {
    return 'Панель, автоматически';
  }
  if (value === 'clients') {
    return 'Сами клиенты';
  }
  const [kind, id] = value.split(':') as [string, string];
  if (kind === 'admin') {
    return meta?.admins.find((a) => String(a.id) === id)?.email ?? `Администратор №${id}`;
  }
  if (kind === 'key') {
    const name = meta?.keys.find((k) => String(k.id) === id)?.name;
    return name ? `Ключ «${name}»` : `Ключ №${id}`;
  }
  if (kind === 'module') {
    return `Модуль «${meta?.modules.find((m) => m.id === id)?.name ?? id}»`;
  }
  if (kind === 'client') {
    return client && String(client.client_id) === id ? client.label : `Клиент №${id}`;
  }
  return value;
}

export function whatFilterLabel(value: string, meta: AuditMeta | null): string {
  if (!value) {
    return 'Все действия';
  }
  if (value.startsWith('area:')) {
    const id = value.slice(5);
    return meta?.areas.find((a) => a.id === id)?.title ?? 'Раздел';
  }
  const code = value.slice(7);
  for (const area of meta?.areas ?? []) {
    const found = area.actions.find((a) => a.code === code);
    if (found) {
      return found.title;
    }
  }
  return code;
}

export function periodLabel(filters: Filters): string {
  if (filters.period === 'custom') {
    const fmt = (day: string) => (day ? day.split('-').reverse().join('.') : '…');
    return `${fmt(filters.since)} — ${fmt(filters.until)}`;
  }
  return PERIODS.find(([key]) => key === filters.period)?.[1] ?? 'Всё время';
}

const SUMMARY_KEYS = ['reason', 'text', 'comment', 'note', 'file', 'code', 'name', 'version', 'error'];

export function summaryOf(item: AuditItem): string | null {
  const change = item.changes[0];
  if (change) {
    return `${change.label}: ${change.before} → ${change.after}`;
  }
  for (const key of SUMMARY_KEYS) {
    const field = item.fields.find((f) => f.key === key);
    if (field && field.value !== '—') {
      return field.value;
    }
  }
  return null;
}

function two(n: number): string {
  return String(n).padStart(2, '0');
}

const ISO_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:?\d{2})$/;

export function valueText(value: string): string {
  if (!ISO_RE.test(value)) {
    return value;
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return `${two(date.getDate())}.${two(date.getMonth() + 1)}.${date.getFullYear()}, ${two(date.getHours())}:${two(date.getMinutes())}`;
}

export function whenShort(iso: string, now = Date.now()): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  const day = `${two(date.getDate())}.${two(date.getMonth() + 1)}`;
  const year = date.getFullYear() === new Date(now).getFullYear() ? '' : `.${date.getFullYear()}`;
  return `${day}${year}, ${two(date.getHours())}:${two(date.getMinutes())}`;
}

export type Chip = { key: keyof Filters | 'range'; label: string; clear: Partial<Filters> };

export function chipsOf(filters: Filters, meta: AuditMeta | null, client: Target | null): Chip[] {
  const chips: Chip[] = [];
  if (filters.actor) {
    chips.push({ key: 'actor', label: actorFilterLabel(filters.actor, meta, client), clear: { actor: '' } });
  }
  if (filters.what) {
    chips.push({ key: 'what', label: whatFilterLabel(filters.what, meta), clear: { what: '' } });
  }
  if (filters.client) {
    const label = client && String(client.client_id) === filters.client ? client.label : `Клиент №${filters.client}`;
    chips.push({ key: 'client', label, clear: { client: '' } });
  }
  if (filters.period !== 'all') {
    chips.push({ key: 'range', label: periodLabel(filters), clear: { period: 'all', since: '', until: '' } });
  }
  if (filters.q.trim()) {
    chips.push({ key: 'q', label: `«${filters.q.trim()}»`, clear: { q: '' } });
  }
  return chips;
}
