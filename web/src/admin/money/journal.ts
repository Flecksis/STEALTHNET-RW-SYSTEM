import { ApiError } from '../../lib/api';
import { count, plural } from '../../lib/format';
import type { Amount } from '../client/types';
import type { Person } from '../home/model';

export const WEBHOOKS = '/api/admin/webhooks';
export const JOURNAL = '/admin/money/webhooks';

export type Tone = 'ok' | 'muted' | 'warn' | 'bad';

export type Fact = { label: string; value: string };

export type JournalItem = {
  id: number;
  provider: string;
  provider_title: string;
  received_at: string;
  processed_at: string | null;
  event: string | null;
  outcome: string;
  tone: Tone;
  title: string;
  note: string | null;
  amount: Amount | null;
  amount_text: string | null;
  txid: string | null;
  payment: { id: number; public_id: string; amount: Amount; amount_text: string; status: string; order_public_id: string | null } | null;
  client_id: number | null;
  client: Person | null;
  deliveries: number;
  retry: { available: boolean; note: string | null };
};

export type Retention = { days: number; rejected_days: number };

export type JournalPage = {
  items: JournalItem[];
  next: number | null;
  providers: Array<{ id: string; title: string }>;
  retention: Retention;
};

export type BodyView =
  | { kind: 'json'; text: string }
  | { kind: 'form'; fields: Fact[] }
  | { kind: 'text'; text: string }
  | { kind: 'binary'; size: number }
  | { kind: 'missing'; reason: string };

export type Step = { at: string; text: string; tone: Tone };

export type JournalCard = JournalItem & {
  remote_ip: string | null;
  raw_event: string | null;
  advice: string | null;
  facts: Fact[];
  headers: Fact[];
  body: BodyView;
  masked: boolean;
  steps: Step[];
  retention: Retention;
};

export type Attention = { rejected: number; failed: number; providers: string[]; hours: number };

export type Period = 'day' | 'week' | 'all' | 'custom';

export const OUTCOMES: Array<[string, string]> = [
  ['', 'Все итоги'],
  ['attention', 'Требуют внимания'],
  ['credited', 'Засчитано'],
  ['repeat', 'Повтор, уже было'],
  ['skipped', 'Пропущено'],
  ['rejected', 'Отклонено'],
  ['mismatch', 'Сумма не сошлась'],
  ['failed', 'Ошибка обработки'],
];

export const PERIODS: Array<[Period, string]> = [
  ['day', 'За сутки'],
  ['week', 'За неделю'],
  ['all', 'За всё время'],
  ['custom', 'Свои даты'],
];

export type Filters = { provider: string; outcome: string; period: Period; since: string; until: string; q: string };

export const EMPTY: Filters = { provider: '', outcome: '', period: 'all', since: '', until: '', q: '' };

const PERIOD_KEYS = new Set<string>(PERIODS.map(([key]) => key));
const OUTCOME_KEYS = new Set<string>(OUTCOMES.map(([key]) => key));

export function filtersOf(params: URLSearchParams): Filters {
  const period = params.get('period') ?? 'all';
  const outcome = params.get('outcome') ?? '';
  return {
    provider: params.get('provider') ?? '',
    outcome: OUTCOME_KEYS.has(outcome) ? outcome : '',
    period: PERIOD_KEYS.has(period) ? (period as Period) : 'all',
    since: params.get('since') ?? '',
    until: params.get('until') ?? '',
    q: params.get('q') ?? '',
  };
}

export function paramsOf(filters: Filters): URLSearchParams {
  const params = new URLSearchParams();
  if (filters.provider) {
    params.set('provider', filters.provider);
  }
  if (filters.outcome) {
    params.set('outcome', filters.outcome);
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

function localDay(value: string, shift = 0): string | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) {
    return null;
  }
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]) + shift);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

export function queryOf(filters: Filters, after: number | null, now = Date.now()): string {
  const params = new URLSearchParams({ limit: '30' });
  if (filters.provider) {
    params.set('provider', filters.provider);
  }
  if (filters.outcome) {
    params.set('outcome', filters.outcome);
  }
  if (filters.period === 'day') {
    params.set('from', new Date(now - 86_400_000).toISOString());
  }
  if (filters.period === 'week') {
    params.set('from', new Date(now - 7 * 86_400_000).toISOString());
  }
  if (filters.period === 'custom') {
    const from = localDay(filters.since);
    const to = localDay(filters.until, 1);
    if (from) {
      params.set('from', from);
    }
    if (to) {
      params.set('to', to);
    }
  }
  if (filters.q.trim()) {
    params.set('q', filters.q.trim());
  }
  if (after !== null) {
    params.set('after', String(after));
  }
  return `${WEBHOOKS}?${params.toString()}`;
}

export function toneColor(tone: Tone): string {
  switch (tone) {
    case 'bad':
      return 'var(--bad)';
    case 'warn':
      return 'var(--warn)';
    case 'muted':
      return 'var(--muted)';
    default:
      return 'var(--ink)';
  }
}

export function eventText(event: string | null): string {
  return event ? event.replace(/\./g, '.\u200b') : 'не разобрано';
}

export function verdictText(item: Pick<JournalItem, 'title' | 'note'>): string {
  return item.note ? `${item.title}: ${item.note}` : item.title;
}

export function retentionText(retention: Retention): string {
  return `Уведомления хранятся ${count(retention.days, 'день', 'дня', 'дней')}, не прошедшие проверку подписи или разбор — ${count(retention.rejected_days, 'день', 'дня', 'дней')}.`;
}

export function deliveriesText(n: number): string | null {
  return n > 1 ? `пришло ${count(n, 'раз', 'раза', 'раз')}` : null;
}

export function attentionCount(attention: Attention | null | undefined): number {
  return attention ? attention.rejected + attention.failed : 0;
}

export function attentionTitle(attention: Attention): string {
  const total = attentionCount(attention);
  return `${count(total, 'уведомление', 'уведомления', 'уведомлений')} платёжек ${plural(total, 'не прошло', 'не прошли', 'не прошло')}`;
}

export function attentionNote(attention: Attention): string {
  const parts: string[] = [];
  if (attention.rejected) {
    parts.push(`отклонено ${attention.rejected}`);
  }
  if (attention.failed) {
    parts.push(`с ошибкой ${attention.failed}`);
  }
  const who = attention.providers.length ? ` · ${attention.providers.join(', ')}` : '';
  return `За сутки ${parts.join(', ')}${who}`;
}

export const ATTENTION_LINK = `${JOURNAL}?outcome=attention&period=day`;

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}
