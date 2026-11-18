import { ApiError } from '../../lib/api';
import { count, dateLong, days, groupDigits, MINUS, money, NBSP, plural, timeHM, type Money } from '../../lib/format';
import type { Currency } from '../../lib/types';

export type PromoKind = { type: 'percent'; percent: number } | { type: 'fixed'; amount: Money } | { type: 'days'; days: number };

export type PromoCode = {
  id: number;
  code: string;
  kind: PromoKind;
  max_uses: number | null;
  per_client: number;
  valid_from: string | null;
  valid_until: string | null;
  first_purchase_only: boolean;
  tariff_ids: number[];
  active: boolean;
  note: string | null;
  created_at: string;
  used: number;
  reserved: number;
};

export type PromoPage = { items: PromoCode[]; next: number | null };

export type Filter = 'all' | 'active' | 'disabled';

export const FILTERS: Array<[Filter, string]> = [
  ['all', 'Все'],
  ['active', 'Включены'],
  ['disabled', 'Выключены'],
];

export type TariffBrief = { id: number; name: string };

export type KindChoice = 'percent' | 'fixed' | 'days';

export const KINDS: Array<[KindChoice, string]> = [
  ['percent', 'Скидка, %'],
  ['fixed', 'Скидка суммой'],
  ['days', 'Дни к подписке'],
];

export type Draft = {
  code: string;
  kind: KindChoice;
  value: string;
  maxUses: string;
  perClient: string;
  from: string;
  until: string;
  firstOnly: boolean;
  tariffs: number[];
  note: string;
};

export const EMPTY_DRAFT: Draft = {
  code: '',
  kind: 'percent',
  value: '',
  maxUses: '',
  perClient: '1',
  from: '',
  until: '',
  firstOnly: false,
  tariffs: [],
  note: '',
};

export type DraftErrors = Partial<Record<'code' | 'value' | 'maxUses' | 'perClient' | 'from' | 'until' | 'note', string>>;

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function disabledModule(err: unknown): boolean {
  return err instanceof ApiError && err.code === 'module.disabled';
}

export function valueText(kind: PromoKind): string {
  switch (kind.type) {
    case 'percent':
      return `${MINUS}${kind.percent}%`;
    case 'fixed':
      return `${MINUS}${money(kind.amount)}`;
    case 'days':
      return `+${days(kind.days)}`;
  }
}

export function purposeText(kind: PromoKind): string {
  return kind.type === 'days' ? 'дни к подписке' : 'скидка при оплате';
}

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

function untilText(iso: string): string {
  const date = new Date(iso);
  if (date.getTime() === startOfDay(date)) {
    return `по${NBSP}${dateLong(new Date(date.getTime() - 1).toISOString())}`;
  }
  return `до${NBSP}${dateLong(iso)}, ${timeHM(iso)}`;
}

function fromText(iso: string): string {
  const date = new Date(iso);
  if (date.getTime() === startOfDay(date)) {
    return `с${NBSP}${dateLong(iso)}`;
  }
  return `с${NBSP}${dateLong(iso)}, ${timeHM(iso)}`;
}

function isMidnight(date: Date): boolean {
  return date.getTime() === startOfDay(date);
}

export function termText(p: PromoCode): string {
  if (!p.valid_from && !p.valid_until) {
    return 'Без срока';
  }
  if (p.valid_from && p.valid_until) {
    const from = new Date(p.valid_from);
    const last = new Date(Date.parse(p.valid_until) - 1);
    if (isMidnight(from) && isMidnight(new Date(p.valid_until)) && from.getFullYear() === last.getFullYear() && from.getMonth() === last.getMonth()) {
      return `С${NBSP}${from.getDate()} по${NBSP}${dateLong(last.toISOString())}`;
    }
  }
  const parts = [p.valid_from ? fromText(p.valid_from) : null, p.valid_until ? untilText(p.valid_until) : null].filter(Boolean);
  const text = parts.join(' ');
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export function usedText(p: PromoCode): string {
  return p.max_uses === null ? `${groupDigits(p.used)}` : `${groupDigits(p.used)}${NBSP}из${NBSP}${groupDigits(p.max_uses)}`;
}

export function limitsText(p: PromoCode, tariffs: TariffBrief[] | null): string {
  const parts: string[] = [];
  parts.push(p.max_uses === null ? 'без общего лимита' : `всего ${count(p.max_uses, 'раз', 'раза', 'раз')}`);
  parts.push(p.per_client === 1 ? 'один раз на клиента' : `до ${count(p.per_client, 'раза', 'раз', 'раз')} на клиента`);
  if (p.first_purchase_only) {
    parts.push('только первая покупка');
  }
  if (p.tariff_ids.length) {
    const names = tariffs ? p.tariff_ids.map((id) => tariffs.find((t) => t.id === id)?.name).filter((n): n is string => !!n) : [];
    parts.push(names.length === p.tariff_ids.length ? `тарифы: ${names.join(', ')}` : count(p.tariff_ids.length, 'тариф', 'тарифа', 'тарифов'));
  }
  return parts.join(' · ');
}

export type State = { text: string; tone: 'ink' | 'warn' | 'muted' };

export function stateOf(p: PromoCode, now = Date.now()): State {
  if (!p.active) {
    return { text: 'Выключен', tone: 'muted' };
  }
  if (p.valid_until && Date.parse(p.valid_until) <= now) {
    return { text: 'Срок истёк', tone: 'muted' };
  }
  if (p.valid_from && Date.parse(p.valid_from) > now) {
    return { text: 'Ещё не действует', tone: 'muted' };
  }
  if (p.max_uses !== null && p.used + p.reserved >= p.max_uses) {
    return { text: 'Исчерпан', tone: 'warn' };
  }
  return { text: 'Действует', tone: 'ink' };
}

export function reservedText(p: PromoCode): string | null {
  if (!p.reserved) {
    return null;
  }
  return `ещё ${groupDigits(p.reserved)}${NBSP}${plural(p.reserved, 'ждёт', 'ждут', 'ждут')} оплаты`;
}

const ALPHABET = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';

export function inventCode(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => ALPHABET[b % ALPHABET.length]).join('');
}

const CODE = /^[A-Z0-9_-]{3,32}$/;

function wholeNumber(raw: string): number | null {
  const clean = raw.trim();
  if (!/^\d+$/.test(clean)) {
    return null;
  }
  const n = Number(clean);
  return Number.isSafeInteger(n) ? n : null;
}

function dayStart(raw: string, shift = 0): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(raw);
  if (!match) {
    return null;
  }
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]) + shift);
  return Number.isNaN(date.getTime()) ? null : date;
}

export function amountField(raw: string, currency: Currency): string | null {
  const clean = raw.replace(/\s/g, '').replace(',', '.');
  if (!/^\d+(\.\d{1,2})?$/.test(clean)) {
    return null;
  }
  const [whole = '0', cents = ''] = clean.split('.');
  if (currency === 'RUB' && cents.replace(/0/g, '') !== '') {
    return null;
  }
  const minor = Number(whole) * 100 + Number(cents.padEnd(2, '0'));
  if (!Number.isSafeInteger(minor) || minor <= 0) {
    return null;
  }
  return currency === 'RUB' ? String(Number(whole)) : `${Number(whole)}.${cents.padEnd(2, '0')}`;
}

export type Built = { body: Record<string, unknown>; errors: DraftErrors };

export function buildBody(draft: Draft, currency: Currency, today = new Date()): Built {
  const errors: DraftErrors = {};
  const code = draft.code.trim().toUpperCase();
  if (!CODE.test(code)) {
    errors.code = 'От 3 до 32 латинских букв, цифр, «-» или «_»';
  }
  const body: Record<string, unknown> = { code, kind: draft.kind };
  if (draft.kind === 'percent') {
    const n = wholeNumber(draft.value);
    if (n === null || n < 1 || n > 100) {
      errors.value = 'Скидка — целое число от 1 до 100';
    } else {
      body.percent = n;
    }
  } else if (draft.kind === 'fixed') {
    const amount = amountField(draft.value, currency);
    if (amount === null) {
      errors.value = currency === 'RUB' ? 'Сумма — целое число рублей больше нуля' : 'Сумма больше нуля, до двух знаков после точки';
    } else {
      body.amount = amount;
    }
  } else {
    const n = wholeNumber(draft.value);
    if (n === null || n < 1 || n > 3650) {
      errors.value = 'Дней — от 1 до 3650';
    } else {
      body.days = n;
    }
  }
  if (draft.maxUses.trim()) {
    const n = wholeNumber(draft.maxUses);
    if (n === null || n < 1 || n > 2_000_000_000) {
      errors.maxUses = 'Целое число больше нуля или пусто';
    } else {
      body.max_uses = n;
    }
  }
  const perClient = wholeNumber(draft.perClient);
  if (perClient === null || perClient < 1 || perClient > 1000) {
    errors.perClient = 'Целое число от 1 до 1000';
  } else {
    body.per_client = perClient;
  }
  const from = draft.from ? dayStart(draft.from) : null;
  const until = draft.until ? dayStart(draft.until, 1) : null;
  if (draft.from && !from) {
    errors.from = 'Дата указана неверно';
  }
  if (draft.until && !until) {
    errors.until = 'Дата указана неверно';
  } else if (until && until.getTime() <= startOfDay(today)) {
    errors.until = 'Эта дата уже прошла';
  } else if (from && until && until.getTime() <= from.getTime()) {
    errors.until = 'Окончание раньше начала';
  }
  if (from) {
    body.valid_from = from.toISOString();
  }
  if (until) {
    body.valid_until = until.toISOString();
  }
  body.first_purchase_only = draft.firstOnly;
  body.tariff_ids = draft.kind === 'days' ? [] : draft.tariffs;
  const note = draft.note.trim();
  if (note.length > 500) {
    errors.note = 'Не длиннее 500 символов';
  } else if (note) {
    body.note = note;
  }
  return { body, errors };
}

export function createdText(p: PromoCode): string {
  return p.kind.type === 'days' ? 'Клиенты активируют его в боте, в разделе «Промокод»: дни добавятся к подписке.' : 'Клиенты вводят его при оплате.';
}

export function offText(p: PromoCode): string {
  return p.kind.type === 'days'
    ? 'Активировать его больше не получится, уже добавленные дни останутся.'
    : 'Новые оплаты с ним не пройдут, уже выставленные счета сохранят скидку.';
}
