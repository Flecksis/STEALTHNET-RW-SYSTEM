import { ApiError } from '../../lib/api';
import { count, dateLong, money, type Money } from '../../lib/format';
import type { Person } from '../home/model';

export type GiftStatus = 'active' | 'redeemed' | 'expired' | 'canceled';

export type Amount = { minor: number; currency: 'USD' | 'RUB' | 'XTR' };

export type AdminGift = {
  id: number;
  code: string;
  tariff_id: number;
  days: number;
  tariff_name: string;
  status: GiftStatus;
  expires_at: string | null;
  redeemed_at: string | null;
  canceled_at: string | null;
  created_at: string;
  giver: Person | null;
  recipient: Person | null;
  paid?: Amount | null;
  refunded: boolean;
};

export type GiftPage = { items: AdminGift[]; next: number | null };

export type Counts = { active: number; redeemed: number; expired: number; canceled: number };

export type SettingsTariff = { id: number; name: string; on_sale: boolean; giftable: boolean };

export type GiftSettings = { tariffs: number[]; code_days: number; available: SettingsTariff[] };

export type Filter = 'all' | GiftStatus;

export const FILTERS: Array<[Filter, string]> = [
  ['all', 'Все'],
  ['active', 'Ждут'],
  ['redeemed', 'Активированы'],
  ['expired', 'Истекли'],
  ['canceled', 'Отменены'],
];

export const ROOT = '/api/admin/m/gifts';

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function disabledModule(err: unknown): boolean {
  return err instanceof ApiError && err.code === 'module.disabled';
}

export function term(days: number): string {
  if (days >= 28) {
    const months = Math.max(1, Math.round((days * 12) / 365));
    return count(months, 'месяц', 'месяца', 'месяцев');
  }
  return count(days, 'день', 'дня', 'дней');
}

export function giftText(g: AdminGift): string {
  return `«${g.tariff_name}» на ${term(g.days)}`;
}

export function who(person: Person | null): string {
  if (!person) {
    return '';
  }
  if (person.telegram_username) {
    return `@${person.telegram_username}`;
  }
  if (person.name) {
    return person.name;
  }
  if (person.email) {
    return person.email;
  }
  if (person.telegram_id) {
    return `Telegram ${person.telegram_id}`;
  }
  return `клиент ${person.client_id}`;
}

export function paidText(g: AdminGift): string {
  if (!g.paid) {
    return '';
  }
  if (g.paid.currency === 'XTR') {
    return count(g.paid.minor, 'звезда', 'звезды', 'звёзд');
  }
  return money({ minor: g.paid.minor, currency: g.paid.currency } as Money);
}

export function stateOf(g: AdminGift): { text: string; note: string; tone: 'ink' | 'warn' | 'muted' } {
  switch (g.status) {
    case 'active':
      return { text: 'Ждёт активации', note: g.expires_at ? `код до ${dateLong(g.expires_at, true)}` : 'код без срока', tone: 'warn' };
    case 'redeemed':
      return { text: 'Активирован', note: g.redeemed_at ? dateLong(g.redeemed_at, true) : '', tone: 'ink' };
    case 'expired':
      return { text: 'Срок кода истёк', note: g.expires_at ? dateLong(g.expires_at, true) : '', tone: 'muted' };
    case 'canceled':
      return { text: 'Отменён', note: 'деньги вернули', tone: 'muted' };
  }
}

export function summaryText(c: Counts | null): string {
  if (!c) {
    return '';
  }
  const total = c.active + c.redeemed + c.expired + c.canceled;
  if (!total) {
    return 'Подарков ещё не покупали.';
  }
  const parts: string[] = [];
  if (c.active) {
    parts.push(count(c.active, 'ждёт активации', 'ждут активации', 'ждут активации'));
  }
  if (c.redeemed) {
    parts.push(count(c.redeemed, 'активирован', 'активированы', 'активированы'));
  }
  if (c.expired) {
    parts.push(count(c.expired, 'с истёкшим кодом', 'с истёкшим кодом', 'с истёкшим кодом'));
  }
  if (c.canceled) {
    parts.push(count(c.canceled, 'отменён', 'отменены', 'отменены'));
  }
  return `Всего ${count(total, 'подарок', 'подарка', 'подарков')}: ${parts.join(', ')}.`;
}

export function emptyText(filter: Filter, search: string): string {
  if (search.trim()) {
    return 'Ничего не нашли. Проверьте код или имя клиента.';
  }
  switch (filter) {
    case 'active':
      return 'Нет подарков, которые ждут активации';
    case 'redeemed':
      return 'Активированных подарков нет';
    case 'expired':
      return 'Подарков с истёкшим кодом нет';
    case 'canceled':
      return 'Отменённых подарков нет';
    default:
      return 'Подарков пока нет. Клиенты покупают их в боте, кабинете и мини-приложении.';
  }
}
