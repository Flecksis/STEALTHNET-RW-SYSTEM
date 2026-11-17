import { ApiError } from '../../lib/api';
import { dateLong, groupDigits, plural, timeHM } from '../../lib/format';

export const BASE = '/api/admin/m/broadcast';
export const ROOT = '/admin/m/broadcast';

export type Status = 'draft' | 'scheduled' | 'sending' | 'done' | 'canceled';

export type Section =
  | 'menu'
  | 'subscriptions'
  | 'buy'
  | 'trial'
  | 'balance'
  | 'support'
  | 'promo'
  | 'referral'
  | 'options'
  | 'documents';

export type Audience =
  | { kind: 'all' }
  | { kind: 'active' }
  | { kind: 'expiring'; days: number }
  | { kind: 'expired' }
  | { kind: 'never_connected' }
  | { kind: 'trial' }
  | { kind: 'no_subscription' }
  | { kind: 'tariff'; tariff_id: number };

export type AudienceKind = Audience['kind'];

export type ButtonSpec = { text: string; url?: string; section?: Section };

export type Image = { id: number; content_type: string; width: number; height: number; size: number };

export type Progress = { total: number; sent: number; failed: number; blocked: number; skipped: number; pending: number };

export type Problem = { error: string; count: number };

export type Broadcast = {
  id: number;
  status: Status;
  text: string;
  image: Image | null;
  buttons: ButtonSpec[];
  audience: Audience;
  audience_label: string;
  send_at: string | null;
  started_at: string | null;
  finished_at: string | null;
  created_at: string;
  updated_at: string;
  notice: string | null;
  progress: Progress;
  problems: Problem[];
  queued_behind: number | null;
};

export type ListItem = {
  id: number;
  status: Status;
  preview: string;
  has_image: boolean;
  buttons: number;
  audience: Audience;
  audience_label: string;
  send_at: string | null;
  started_at: string | null;
  finished_at: string | null;
  created_at: string;
  notice: string | null;
  progress: Progress;
};

export type Limits = {
  text: number;
  caption: number;
  raw: number;
  buttons: number;
  button_text: number;
  url: number;
  days: number;
  schedule_days: number;
  image_bytes: number;
};

export type Options = {
  bot: boolean;
  bot_username: string | null;
  sections: Array<{ key: Section; title: string; available: boolean }>;
  tariffs: Array<{ id: number; name: string }>;
  limits: Limits;
};

export type Reach = { recipients: number; stopped: number };

export type Draft = {
  text: string;
  image: Image | null;
  buttons: ButtonSpec[];
  audience: Audience;
};

export const EMPTY: Draft = { text: '', image: null, buttons: [], audience: { kind: 'all' } };

export const AUDIENCES: Array<{ kind: AudienceKind; title: string; hint: string }> = [
  { kind: 'all', title: 'Все клиенты с Telegram', hint: 'Каждый, кто запускал бота' },
  { kind: 'active', title: 'С активной подпиской', hint: 'Подписка действует, пробные не входят' },
  { kind: 'expiring', title: 'Подписка скоро закончится', hint: 'Действует ещё не больше указанного числа дней' },
  { kind: 'expired', title: 'Подписка закончилась', hint: 'Была подписка, сейчас не продлена' },
  { kind: 'never_connected', title: 'Не подключались', hint: 'Подписка есть, но ни одного подключения' },
  { kind: 'trial', title: 'На пробном периоде', hint: 'Сейчас действует пробная подписка' },
  { kind: 'no_subscription', title: 'Ни разу не было подписки', hint: 'Запускали бота, но ничего не оформили' },
  { kind: 'tariff', title: 'По тарифу', hint: 'Действующая подписка на выбранном тарифе' },
];

export function audienceOf(kind: AudienceKind, days: number, tariff: number | null): Audience | null {
  switch (kind) {
    case 'expiring':
      return { kind, days };
    case 'tariff':
      return tariff === null ? null : { kind, tariff_id: tariff };
    default:
      return { kind };
  }
}

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function count(n: number): string {
  return groupDigits(n);
}

export function clients(n: number): string {
  return `${groupDigits(n)}${' '}${plural(n, 'клиенту', 'клиентам', 'клиентам')}`;
}

export function at(iso: string | null): string {
  if (!iso) {
    return '';
  }
  return `${dateLong(iso)} в${' '}${timeHM(iso)}`;
}

export function eta(pending: number): string {
  const seconds = Math.ceil(pending / 25);
  if (seconds < 60) {
    return 'осталось меньше минуты';
  }
  const minutes = Math.round(seconds / 60);
  return `осталось около ${minutes}${'\u00a0'}мин`;
}

export function done(p: Progress): number {
  return p.sent + p.failed + p.blocked;
}

export function percent(p: Progress): number {
  if (p.total <= 0) {
    return 0;
  }
  return Math.min(100, Math.round((done(p) / p.total) * 100));
}

export type StatusLine = { text: string; tone: 'ink' | 'muted' | 'warn' | 'ok' };

export function statusLine(b: { status: Status; send_at: string | null; finished_at: string | null; started_at: string | null; progress: Progress; notice: string | null }): StatusLine {
  switch (b.status) {
    case 'draft':
      return { text: 'Черновик', tone: 'muted' };
    case 'scheduled':
      return { text: `Запланирована на ${at(b.send_at)}`, tone: 'ink' };
    case 'sending':
      return {
        text: b.progress.total > 0 ? `Идёт отправка: ${count(done(b.progress))} из ${count(b.progress.total)}` : 'Готовится к отправке',
        tone: b.notice ? 'warn' : 'ink',
      };
    case 'done':
      return { text: b.progress.total > 0 ? `Отправлена ${at(b.finished_at)}` : 'Получателей не нашлось', tone: b.progress.total > 0 ? 'ok' : 'muted' };
    case 'canceled':
      return { text: `Остановлена ${at(b.finished_at)}`, tone: 'warn' };
  }
}

export function outcome(p: Progress): string {
  const parts = [`доставлено ${count(p.sent)}`];
  if (p.failed > 0) {
    parts.push(`не${' '}доставлено ${count(p.failed)}`);
  }
  if (p.blocked > 0) {
    parts.push(`бот остановлен у${' '}${count(p.blocked)}`);
  }
  if (p.skipped > 0) {
    parts.push(`не${' '}отправлено ${count(p.skipped)}`);
  }
  return parts.join(' · ');
}

export function draftBody(draft: Draft) {
  return {
    text: draft.text,
    image_id: draft.image?.id ?? null,
    buttons: draft.buttons.map((b) => (b.section ? { text: b.text, section: b.section } : { text: b.text, url: b.url ?? '' })),
    audience: draft.audience,
  };
}

export function draftOf(b: Broadcast): Draft {
  return { text: b.text, image: b.image, buttons: b.buttons, audience: b.audience };
}

export function imageUrl(id: number): string {
  return `${BASE}/images/${id}`;
}

export function localInput(date: Date): string {
  const pad = (n: number) => n.toString().padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function zoneLabel(): string {
  const minutes = -new Date().getTimezoneOffset();
  const sign = minutes >= 0 ? '+' : '−';
  const abs = Math.abs(minutes);
  const hours = Math.floor(abs / 60);
  const rest = abs % 60;
  return `UTC${sign}${hours}${rest ? `:${rest.toString().padStart(2, '0')}` : ''}`;
}
