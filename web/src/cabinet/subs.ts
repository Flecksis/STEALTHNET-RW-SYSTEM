import { useEffect, useRef, type RefObject } from 'react';
import { dateLong } from '../lib/format';
import { t } from '../lib/i18n';
import { readStored, writeStored } from '../lib/theme';
import type { SubscriptionView, Tone } from './model';

export const NEWEST = 'newest';

export type Remembered = number | typeof NEWEST;

export type PayTarget = number | 'new';

export function liveSubs(list: SubscriptionView[]): SubscriptionView[] {
  return list.filter((v) => !v.subscription.ended_at);
}

function baseName(view: SubscriptionView): string {
  return view.tariff?.name ?? (view.subscription.kind === 'trial' ? t('Пробный период') : t('Подписка'));
}

const LEGACY_NAME = /^sn_[0-9a-f]{8}-/i;

export function subName(view: SubscriptionView, list: SubscriptionView[]): string {
  const base = baseName(view);
  if (list.filter((v) => baseName(v) === base).length < 2) {
    return base;
  }
  const nick = view.subscription.remna?.username ?? null;
  if (nick && !LEGACY_NAME.test(nick)) {
    return `${base} · ${nick}`;
  }
  return t('Подписка {n}', { n: list.findIndex((v) => v.subscription.id === view.subscription.id) + 1 });
}

export function subState(view: SubscriptionView, blocked: boolean): { text: string; tone: Tone } {
  const expire = view.subscription.mirror.expire_at;
  const date = expire ? dateLong(expire, false) : '';
  if (blocked || view.status === 'blocked') {
    return { text: t('Заблокирована'), tone: 'bad' };
  }
  if (view.status === 'expired') {
    return { text: date ? t('Истекла {date}', { date }) : t('Истекла'), tone: 'muted' };
  }
  if (view.status === 'limited') {
    return { text: t('Трафик закончился'), tone: 'warn' };
  }
  if (view.status === 'expiring') {
    return { text: date ? t('Закончится {date}', { date }) : t('Скоро закончится'), tone: 'warn' };
  }
  return { text: date ? t('Активна до {date}', { date }) : t('Активна'), tone: 'ink2' };
}

export function readRemembered(key: string): Remembered | null {
  const raw = readStored(key);
  if (raw === NEWEST) {
    return NEWEST;
  }
  const id = Number(raw);
  return Number.isInteger(id) && id > 0 ? id : null;
}

export function remember(key: string, value: Remembered): void {
  writeStored(key, String(value));
}

export function pickSub(list: SubscriptionView[], wanted: Remembered | null): SubscriptionView | null {
  if (wanted === NEWEST) {
    return list.reduce<SubscriptionView | null>((best, v) => (!best || v.subscription.id > best.subscription.id ? v : best), null);
  }
  return list.find((v) => v.subscription.id === wanted) ?? list[0] ?? null;
}

export function payTargets(list: SubscriptionView[], multi: boolean): PayTarget[] {
  const renewable = list.filter((v) => !(multi && v.subscription.kind === 'trial')).map((v) => v.subscription.id);
  return multi || renewable.length === 0 ? [...renewable, 'new'] : renewable;
}

export function resolveTarget(targets: PayTarget[], list: SubscriptionView[], multi: boolean, want: PayTarget | null): PayTarget {
  if (want !== null && targets.includes(want)) {
    return want;
  }
  if (multi && typeof want === 'number' && list.some((v) => v.subscription.id === want && v.subscription.kind === 'trial')) {
    return 'new';
  }
  return targets[0] ?? 'new';
}

export function targetView(list: SubscriptionView[], target: PayTarget | null, multi: boolean): SubscriptionView | null {
  if (typeof target === 'number') {
    return list.find((v) => v.subscription.id === target) ?? null;
  }
  return multi ? (list.find((v) => v.subscription.kind === 'trial') ?? null) : null;
}

export function newTargetNote(list: SubscriptionView[], multi: boolean): string {
  return multi && list.some((v) => v.subscription.kind === 'trial') ? t('Вместо пробного периода') : t('Своя ссылка и свои устройства');
}

export function useShowPressed(box: RefObject<HTMLElement | null>, key: string): void {
  const first = useRef(true);
  useEffect(() => {
    const el = box.current;
    const item = el?.querySelector<HTMLElement>('[aria-pressed="true"]');
    if (!el || !item) {
      return;
    }
    const smooth = !first.current;
    first.current = false;
    if (el.scrollWidth <= el.clientWidth) {
      return;
    }
    const pad = parseFloat(getComputedStyle(el).paddingLeft) || 0;
    const left = item.offsetLeft - pad;
    const right = item.offsetLeft + item.offsetWidth + pad - el.clientWidth;
    const next = left < el.scrollLeft ? left : right > el.scrollLeft ? right : null;
    if (next !== null) {
      el.scrollTo({ left: next, behavior: smooth ? 'smooth' : 'auto' });
    }
  }, [box, key]);
}
