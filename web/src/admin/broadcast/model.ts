import { ApiError } from '../../lib/api';
import { dateLong, groupDigits, plural, timeHM } from '../../lib/format';

export const BASE = '/api/admin/m/broadcast';
export const ROOT = '/admin/m/broadcast';

export type Status = 'draft' | 'scheduled' | 'sending' | 'done' | 'canceled';

export type Channel = 'bot' | 'email' | 'both';

export type Lane = 'bot' | 'email';

export const CHANNELS: Array<{ key: Channel; title: string }> = [
  { key: 'bot', title: 'В бот' },
  { key: 'email', title: 'На почту' },
  { key: 'both', title: 'В бот и на почту' },
];

export function hasBot(channel: Channel): boolean {
  return channel !== 'email';
}

export function hasEmail(channel: Channel): boolean {
  return channel !== 'bot';
}

export function channelLabel(channel: Channel): string {
  return channel === 'bot' ? 'в бот' : channel === 'email' ? 'на почту' : 'в бот и на почту';
}

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

export type Problem = { channel: Lane; error: string; count: number };

export type ChannelProgress = { channel: Lane; progress: Progress };

export type FailedEmail = { email: string; error: string };

export type Broadcast = {
  id: number;
  status: Status;
  channel: Channel;
  subject: string | null;
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
  mail_notice: string | null;
  progress: Progress;
  channels: ChannelProgress[];
  problems: Problem[];
  failed_emails: FailedEmail[];
  queued_behind: number | null;
};

export type ListItem = {
  id: number;
  status: Status;
  channel: Channel;
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
  mail_notice: string | null;
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
  subject: number;
};

export type Trigger =
  | 'no_subscription'
  | 'trial_expiring'
  | 'expiring'
  | 'traffic_low'
  | 'trial_ended'
  | 'expired'
  | 'no_purchase'
  | 'not_connected';

export type Unit = 'hours' | 'days';

export type AutoOptions = {
  every_minutes: number;
  catch_up_hours: number;
  rules_max: number;
  days_max: number;
  hours_max: number;
  percent_min: number;
  percent_max: number;
  percent_default: number;
  english: boolean;
  triggers: Array<{ key: Trigger; title: string; min: number; percent: boolean }>;
};

export type Options = {
  bot: boolean;
  bot_username: string | null;
  mail: { ready: boolean; reason: string | null; site?: boolean };
  sections: Array<{ key: Section; title: string; available: boolean }>;
  tariffs: Array<{ id: number; name: string }>;
  limits: Limits;
  auto: AutoOptions;
};

export const AUTO_ROOT = `${ROOT}/auto`;

export type RuleStats = { cases: number; unreachable: number; progress: Progress; last_sent_at: string | null };

export type Localized = { subject: string | null; text: string; buttons: ButtonSpec[] };

export type Rule = {
  id: number;
  trigger: Trigger;
  amount: number;
  unit: Unit;
  percent: number | null;
  with_autorenew: boolean;
  title: string;
  timing: string;
  channel: Channel;
  subject: string | null;
  text: string;
  image: Image | null;
  buttons: ButtonSpec[];
  en: Localized | null;
  preset: string | null;
  enabled: boolean;
  notice: string | null;
  sending_notices: string[];
  checked_at: string | null;
  created_at: string;
  updated_at: string;
  stats: RuleStats;
  problems: Problem[];
};

export type RuleItem = {
  id: number;
  trigger: Trigger;
  amount: number;
  unit: Unit;
  percent: number | null;
  with_autorenew: boolean;
  title: string;
  timing: string;
  channel: Channel;
  preview: string;
  has_image: boolean;
  buttons: number;
  english: boolean;
  preset: string | null;
  enabled: boolean;
  notice: string | null;
  stats: RuleStats;
};

export type AutoReach = { cases: number; recipients: number; stopped: number; emails: number; unsubscribed: number };

export const TRIGGER_HINTS: Record<Trigger, string> = {
  no_subscription: 'Прошло столько времени с регистрации, а подписок у клиента нет — ни пробной, ни платной',
  trial_expiring: 'До конца пробного периода осталось столько времени. Тем, кто уже купил подписку, не пишет',
  expiring: 'Подписка действует, до конца осталось столько времени. Пробные — отдельным событием',
  traffic_low: 'У подписки с лимитом израсходована указанная доля трафика. Один раз до сброса или докупки трафика',
  trial_ended: 'Пробный период закончился, оплаченных покупок у клиента нет',
  expired: 'Подписку не продлили, другой действующей нет. Пробные — отдельным событием',
  no_purchase: 'Прошло столько времени с регистрации, оплаченных покупок нет. Пробный период — не покупка',
  not_connected: 'Оплаченная подписка действует, но с покупки ни одного подключения',
};

export const TRIGGER_WHEN: Record<Trigger, { before: string; after: string; amount: number; unit: Unit }> = {
  no_subscription: { before: 'Через', after: 'после регистрации', amount: 1, unit: 'days' },
  trial_expiring: { before: 'За', after: 'до окончания пробного периода', amount: 6, unit: 'hours' },
  expiring: { before: 'За', after: 'до окончания подписки', amount: 3, unit: 'days' },
  traffic_low: { before: 'Когда израсходовано', after: 'трафика', amount: 0, unit: 'days' },
  trial_ended: { before: 'Через', after: 'после окончания пробного периода', amount: 0, unit: 'days' },
  expired: { before: 'Через', after: 'после окончания подписки', amount: 1, unit: 'days' },
  no_purchase: { before: 'Через', after: 'после регистрации', amount: 3, unit: 'days' },
  not_connected: { before: 'Через', after: 'после покупки', amount: 2, unit: 'days' },
};

export function unitWord(n: number, unit: Unit): string {
  return unit === 'hours' ? plural(n, 'час', 'часа', 'часов') : plural(n, 'день', 'дня', 'дней');
}

export function timing(trigger: Trigger, amount: number, unit: Unit, percent: number | null): string {
  if (trigger === 'traffic_low') {
    return `когда израсходовано ${percent ?? 80}% трафика`;
  }
  const span = `${amount}${'\u00a0'}${unitWord(amount, unit)}`;
  switch (trigger) {
    case 'expiring':
    case 'trial_expiring':
      return `за ${span} до окончания`;
    case 'expired':
    case 'trial_ended':
      return amount === 0 ? 'сразу после окончания' : `через ${span} после окончания`;
    case 'no_purchase':
    case 'no_subscription':
      return `через ${span} после регистрации`;
    case 'not_connected':
      return `через ${span} после покупки`;
  }
}

export type Reach = { recipients: number; stopped: number; emails: number; unsubscribed: number };

export function reachTotal(reach: Reach, channel: Channel): number {
  return (hasBot(channel) ? reach.recipients : 0) + (hasEmail(channel) ? reach.emails : 0);
}

export const AUDIENCES: Array<{ kind: AudienceKind; title: string; hint: string }> = [
  { kind: 'all', title: 'Все клиенты', hint: 'Каждый, кто запускал бота или указал почту' },
  { kind: 'active', title: 'С активной подпиской', hint: 'Подписка действует, пробные не входят' },
  { kind: 'expiring', title: 'Подписка скоро закончится', hint: 'Действует ещё не больше указанного числа дней' },
  { kind: 'expired', title: 'Подписка закончилась', hint: 'Была подписка, сейчас не продлена' },
  { kind: 'never_connected', title: 'Не подключались', hint: 'Подписка есть, но ни одного подключения' },
  { kind: 'trial', title: 'На пробном периоде', hint: 'Сейчас действует пробная подписка' },
  { kind: 'no_subscription', title: 'Ни разу не было подписки', hint: 'Есть аккаунт, но ничего не оформили' },
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

export function addresses(n: number): string {
  return `${groupDigits(n)}${' '}${plural(n, 'адрес', 'адреса', 'адресов')}`;
}

export function reachText(reach: Reach, channel: Channel): string | null {
  const parts = [
    hasBot(channel) && (reach.recipients > 0 || channel === 'bot') ? `${clients(reach.recipients)} в${'\u00a0'}бот` : null,
    hasEmail(channel) && (reach.emails > 0 || channel === 'email') ? `письмом на${'\u00a0'}${addresses(reach.emails)}` : null,
  ].filter((part): part is string => part !== null);
  return parts.length ? parts.join(' и ') : null;
}

export function at(iso: string | null): string {
  if (!iso) {
    return '';
  }
  return `${dateLong(iso)} в${' '}${timeHM(iso)}`;
}

export function eta(channels: ChannelProgress[]): string {
  const seconds = Math.max(
    0,
    ...channels.map((c) => Math.ceil(c.progress.pending / (c.channel === 'email' ? 2 : 25))),
  );
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

export function statusLine(b: {
  status: Status;
  send_at: string | null;
  finished_at: string | null;
  started_at: string | null;
  progress: Progress;
  notice: string | null;
  mail_notice: string | null;
}): StatusLine {
  switch (b.status) {
    case 'draft':
      return { text: 'Черновик', tone: 'muted' };
    case 'scheduled':
      return { text: `Запланирована на ${at(b.send_at)}`, tone: 'ink' };
    case 'sending':
      return {
        text: b.progress.total > 0 ? `Идёт отправка: ${count(done(b.progress))} из ${count(b.progress.total)}` : 'Готовится к отправке',
        tone: b.notice || b.mail_notice ? 'warn' : 'ink',
      };
    case 'done':
      return { text: b.progress.total > 0 ? `Отправлена ${at(b.finished_at)}` : 'Получателей не нашлось', tone: b.progress.total > 0 ? 'ok' : 'muted' };
    case 'canceled':
      return { text: `Остановлена ${at(b.finished_at)}`, tone: 'warn' };
  }
}

export function outcome(p: Progress, kind: Channel | Lane = 'bot'): string {
  const parts = [`${kind === 'bot' ? 'доставлено' : 'отправлено'} ${count(p.sent)}`];
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
