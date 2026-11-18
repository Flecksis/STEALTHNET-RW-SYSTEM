import { ApiError } from '../../lib/api';
import { money, type Money } from '../../lib/format';
import type { Currency } from '../../lib/types';

export const BASE = '/api/admin/m/roulette';

export type Prize =
  | { kind: 'days'; days: number }
  | { kind: 'balance'; amount: Money }
  | { kind: 'promo'; percent: number; valid_days: number }
  | { kind: 'empty' };

export type PrizeKind = Prize['kind'];

export type FreeFor = 'all' | 'subscribers' | 'paid';

export type Period = 'day' | 'week' | 'month';

export type Sector = { prize: Prize; weight: number; label?: string | null };

export type Config = {
  sectors: Sector[];
  free_every_days: number | null;
  free_for: FreeFor;
  per_purchase: boolean;
  purchase_min: Money | null;
  budget: { period: Period; balance: Money | null; days: number | null; promos: number | null };
};

export type SectorView = { index: number; prize: Prize; kind: PrizeKind; title: string; weight: number; chance: number };

export type Tally = {
  spins: number;
  free: number;
  purchase: number;
  empty: number;
  balance_count: number;
  balance_minor: number;
  days_count: number;
  days: number;
  promo: number;
  failed: number;
  clients: number;
};

export type AdminView = {
  config: Config;
  sectors: SectorView[];
  version: number;
  saved: boolean;
  updated_at: string | null;
  currency: Currency;
  promo_enabled: boolean;
  spent: { balance: Money; days: number; promos: number };
  tally: Tally;
  tickets_open: number;
};

export type Person = { client_id: number; name: string | null; telegram_username: string | null; telegram_id: string | null; email: string | null };

export type SpinItem = {
  id: number;
  client_id: number;
  client: Person | null;
  source: 'free' | 'purchase';
  title: string;
  prize: Prize;
  state: 'empty' | 'issuing' | 'issued' | 'failed';
  note: string | null;
  code: string | null;
  error: string | null;
  at: string;
};

export const KINDS: Array<[PrizeKind, string]> = [
  ['empty', 'Пусто'],
  ['days', 'Дни подписки'],
  ['balance', 'На баланс'],
  ['promo', 'Промокод'],
];

export const FREE_FOR: Array<[FreeFor, string]> = [
  ['paid', 'С купленной или подаренной подпиской'],
  ['subscribers', 'С любой подпиской'],
  ['all', 'Всем'],
];

export const PERIODS: Array<[Period, string]> = [
  ['day', 'Сутки'],
  ['week', '7 дней'],
  ['month', '30 дней'],
];

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function disabledModule(err: unknown): boolean {
  return err instanceof ApiError && err.code === 'module.disabled';
}

export type SectorDraft = { key: string; kind: PrizeKind; days: string; amount: string; percent: string; validDays: string; weight: string; label: string };

export type Draft = {
  sectors: SectorDraft[];
  free: boolean;
  freeEvery: string;
  freeFor: FreeFor;
  perPurchase: boolean;
  purchaseMin: string;
  period: Period;
  budgetBalance: string;
  budgetDays: string;
  budgetPromos: string;
};

let seq = 0;

export function sectorKey(): string {
  seq += 1;
  return `s${seq}`;
}

export function units(m: Money | null | undefined): string {
  if (!m) {
    return '';
  }
  return m.currency === 'RUB' ? String(Math.round(m.minor / 100)) : (m.minor / 100).toFixed(2).replace(/\.00$/, '');
}

export function blankSector(): SectorDraft {
  return { key: sectorKey(), kind: 'empty', days: '3', amount: '', percent: '10', validDays: '14', weight: '10', label: '' };
}

export function draftOf(view: AdminView): Draft {
  const c = view.config;
  return {
    sectors: c.sectors.map((s) => ({
      key: sectorKey(),
      kind: s.prize.kind,
      days: s.prize.kind === 'days' ? String(s.prize.days) : '3',
      amount: s.prize.kind === 'balance' ? units(s.prize.amount) : '',
      percent: s.prize.kind === 'promo' ? String(s.prize.percent) : '10',
      validDays: s.prize.kind === 'promo' ? String(s.prize.valid_days) : '14',
      weight: String(s.weight),
      label: s.label ?? '',
    })),
    free: c.free_every_days !== null,
    freeEvery: c.free_every_days !== null ? String(c.free_every_days) : '7',
    freeFor: c.free_for,
    perPurchase: c.per_purchase,
    purchaseMin: units(c.purchase_min),
    period: c.budget.period,
    budgetBalance: units(c.budget.balance),
    budgetDays: c.budget.days !== null ? String(c.budget.days) : '',
    budgetPromos: c.budget.promos !== null ? String(c.budget.promos) : '',
  };
}

function whole(raw: string): number | null {
  const t = raw.trim();
  return /^\d+$/.test(t) ? Number(t) : null;
}

function amountOk(raw: string, currency: Currency): boolean {
  const t = raw.trim().replace(',', '.');
  return currency === 'RUB' ? /^\d+$/.test(t) && Number(t) > 0 : /^\d+(\.\d{1,2})?$/.test(t) && Number(t) > 0;
}

export function weightOf(s: SectorDraft): number {
  return whole(s.weight) ?? 0;
}

export function chances(sectors: SectorDraft[]): number[] {
  const total = sectors.reduce((sum, s) => sum + weightOf(s), 0);
  return sectors.map((s) => (total ? Math.round((weightOf(s) * 1000) / total) / 10 : 0));
}

export function shortLabel(prize: Prize): string {
  switch (prize.kind) {
    case 'days':
      return `+${prize.days} дн.`;
    case 'balance':
      return money(prize.amount, true);
    case 'promo':
      return `−${prize.percent}%`;
    default:
      return 'Пусто';
  }
}

export function buildBody(draft: Draft, currency: Currency, version: number): { body: unknown; errors: Record<string, string> } {
  const errors: Record<string, string> = {};
  if (draft.sectors.length < 2) {
    errors.sectors = 'Нужно хотя бы два приза';
  }
  const sectors = draft.sectors.map((s, i) => {
    const n = i + 1;
    let prize: unknown = { kind: 'empty' };
    if (s.kind === 'days') {
      const d = whole(s.days);
      if (d === null || d < 1 || d > 365) {
        errors[s.key] = `Приз ${n}: дней от 1 до 365`;
      }
      prize = { kind: 'days', days: d ?? 0 };
    } else if (s.kind === 'balance') {
      if (!amountOk(s.amount, currency)) {
        errors[s.key] = `Приз ${n}: ${currency === 'RUB' ? 'целое число рублей' : 'сумма вида 5 или 2.50'}`;
      }
      prize = { kind: 'balance', amount: s.amount.trim().replace(',', '.') };
    } else if (s.kind === 'promo') {
      const p = whole(s.percent);
      const v = whole(s.validDays);
      if (p === null || p < 1 || p > 100 || v === null || v < 1 || v > 365) {
        errors[s.key] = `Приз ${n}: скидка от 1 до 100 %, срок от 1 до 365 дней`;
      }
      prize = { kind: 'promo', percent: p ?? 0, valid_days: v ?? 0 };
    }
    const w = whole(s.weight);
    if (w === null || w > 10000) {
      errors[s.key] = `Приз ${n}: вес — целое число от 0 до 10 000`;
    }
    return { prize, weight: w ?? 0, label: s.label.trim() || null };
  });
  if (!errors.sectors && sectors.every((s) => s.weight === 0)) {
    errors.sectors = 'Хотя бы у одного приза вес должен быть больше нуля';
  }
  const every = whole(draft.freeEvery);
  if (draft.free && (every === null || every < 1 || every > 365)) {
    errors.freeEvery = 'От 1 до 365 дней';
  }
  if (!draft.free && !draft.perPurchase) {
    errors.freeEvery = 'Включите бесплатные вращения или вращения за покупку';
  }
  if (draft.perPurchase && draft.purchaseMin.trim() && !amountOk(draft.purchaseMin, currency)) {
    errors.purchaseMin = currency === 'RUB' ? 'Целое число рублей' : 'Сумма вида 5 или 2.50';
  }
  if (draft.budgetBalance.trim() && !amountOk(draft.budgetBalance, currency)) {
    errors.budgetBalance = currency === 'RUB' ? 'Целое число рублей' : 'Сумма вида 50 или 12.50';
  }
  const days = draft.budgetDays.trim() ? whole(draft.budgetDays) : null;
  if (draft.budgetDays.trim() && days === null) {
    errors.budgetDays = 'Целое число дней';
  }
  const promos = draft.budgetPromos.trim() ? whole(draft.budgetPromos) : null;
  if (draft.budgetPromos.trim() && promos === null) {
    errors.budgetPromos = 'Целое число кодов';
  }
  return {
    errors,
    body: {
      sectors,
      free_every_days: draft.free ? (every ?? 0) : null,
      free_for: draft.freeFor,
      per_purchase: draft.perPurchase,
      purchase_min: draft.perPurchase ? draft.purchaseMin.trim().replace(',', '.') || null : null,
      budget: {
        period: draft.period,
        balance: draft.budgetBalance.trim().replace(',', '.') || null,
        days,
        promos,
      },
      version,
    },
  };
}
