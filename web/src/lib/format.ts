import type { Currency } from './types';

export const NBSP = ' ';
export const MINUS = '−';

export type Money = { minor: number; currency: Currency };

export function groupDigits(value: number): string {
  const sign = value < 0 ? MINUS : '';
  const digits = Math.abs(Math.trunc(value)).toString();
  return sign + digits.replace(/\B(?=(\d{3})+(?!\d))/g, NBSP);
}

export function money(value: Money | null | undefined, compact = false): string {
  if (!value) {
    return '—';
  }
  const negative = value.minor < 0;
  const abs = Math.abs(value.minor);
  const sign = negative ? MINUS : '';
  if (value.currency === 'RUB') {
    return `${sign}${groupDigits(Math.round(abs / 100))}${NBSP}₽`;
  }
  const whole = Math.floor(abs / 100);
  const cents = abs % 100;
  if (compact && cents === 0) {
    return `${sign}$${groupDigits(whole)}`;
  }
  return `${sign}$${groupDigits(whole)}.${cents.toString().padStart(2, '0')}`;
}

export function plural(n: number, one: string, few: string, many: string): string {
  const abs = Math.abs(n);
  const mod10 = abs % 10;
  const mod100 = abs % 100;
  if (mod10 === 1 && mod100 !== 11) {
    return one;
  }
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) {
    return few;
  }
  return many;
}

export function count(n: number, one: string, few: string, many: string): string {
  return `${groupDigits(n)}${NBSP}${plural(n, one, few, many)}`;
}

export function days(n: number): string {
  return count(n, 'день', 'дня', 'дней');
}

export function periodLabel(daysCount: number | null | undefined): string | null {
  if (!daysCount) {
    return null;
  }
  const months = Math.max(1, Math.floor((daysCount * 12 + 182) / 365));
  if (daysCount >= 28 && Math.abs(daysCount - (months * 365) / 12) <= Math.max(2, months / 2)) {
    return count(months, 'месяц', 'месяца', 'месяцев');
  }
  return days(daysCount);
}

const MONTHS_GEN = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];

function parse(iso: string | null | undefined): Date | null {
  if (!iso) {
    return null;
  }
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? null : date;
}

function two(n: number): string {
  return String(n).padStart(2, '0');
}

export function dateNumeric(iso: string | null | undefined): string {
  const date = parse(iso);
  if (!date) {
    return '';
  }
  return `${two(date.getDate())}.${two(date.getMonth() + 1)}.${date.getFullYear()}`;
}

export function dayMonthNumeric(iso: string | null | undefined): string {
  const date = parse(iso);
  if (!date) {
    return '';
  }
  return `${two(date.getDate())}.${two(date.getMonth() + 1)}`;
}

export function dateLong(iso: string | null | undefined, withYear?: boolean): string {
  const date = parse(iso);
  if (!date) {
    return '';
  }
  const year = withYear ?? date.getFullYear() !== new Date().getFullYear();
  const base = `${date.getDate()}${NBSP}${MONTHS_GEN[date.getMonth()] ?? ''}`;
  return year ? `${base}${NBSP}${date.getFullYear()}` : base;
}

export function timeHM(iso: string | null | undefined): string {
  const date = parse(iso);
  if (!date) {
    return '';
  }
  return `${two(date.getHours())}:${two(date.getMinutes())}`;
}

export function dateTime(iso: string | null | undefined): string {
  const date = parse(iso);
  if (!date) {
    return '';
  }
  const now = new Date();
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = Math.round((startOf(now) - startOf(date)) / 86_400_000);
  const time = timeHM(iso);
  if (diff === 0) {
    return `Сегодня, ${time}`;
  }
  if (diff === 1) {
    return `Вчера, ${time}`;
  }
  return `${dateLong(iso)}, ${time}`;
}

export function ago(iso: string | null | undefined): string {
  const date = parse(iso);
  if (!date) {
    return '';
  }
  const minutes = Math.floor((Date.now() - date.getTime()) / 60_000);
  if (minutes < 1) {
    return 'только что';
  }
  if (minutes < 60) {
    return `${minutes}${NBSP}${plural(minutes, 'минуту', 'минуты', 'минут')} назад`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}${NBSP}${plural(hours, 'час', 'часа', 'часов')} назад`;
  }
  return dateTime(iso);
}

export function daysLeft(iso: string | null | undefined, now = Date.now()): number | null {
  const date = parse(iso);
  if (!date) {
    return null;
  }
  return Math.ceil((date.getTime() - now) / 86_400_000);
}

const GB = 1024 ** 3;
const TB = 1024 ** 4;

function amount(value: number): string {
  const fixed = value < 100 && Math.round(value * 10) % 10 !== 0 ? value.toFixed(1) : Math.round(value).toString();
  return fixed.replace('.', ',');
}

export function traffic(bytes: number | null | undefined): string {
  const value = bytes ?? 0;
  if (value >= TB) {
    return `${amount(value / TB)}${NBSP}ТБ`;
  }
  return `${amount(value / GB)}${NBSP}ГБ`;
}

const AVATAR_COLORS = ['#7C3AED', '#0E7490', '#15803D', '#A16207', '#C2410C', '#BE185D', '#1D4ED8', '#0F766E', '#475569'];

export function avatarColor(id: number): string {
  return AVATAR_COLORS[Math.abs(id) % AVATAR_COLORS.length] ?? '#475569';
}

export function initials(name: string): string {
  const clean = name.trim();
  if (/^id\s/i.test(clean)) {
    return 'ID';
  }
  const words = clean.split(/\s+/).filter(Boolean);
  if (words.length >= 2) {
    return ((words[0]?.[0] ?? '') + (words[1]?.[0] ?? '')).toUpperCase();
  }
  return (words[0] ?? '?').slice(0, 2).toUpperCase();
}
