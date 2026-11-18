import { ApiError } from '../lib/api';
import { count, dateLong, type Money } from '../lib/format';
import { t } from '../lib/i18n';
import type { PeriodOffer } from './model';

export const GIFTS = '/api/client/m/gifts';

export type GiftOffer = {
  tariff: { id: number; category_id: number; name: string; description: string | null };
  periods: PeriodOffer[];
};

export type GiftOffers = { offers: GiftOffer[]; code_days: number };

export type GiftStatus = 'active' | 'redeemed' | 'expired' | 'canceled';

export type MyGift = {
  id: number;
  code: string;
  tariff_name: string;
  days: number;
  status: GiftStatus;
  expires_at: string | null;
  redeemed_at: string | null;
  created_at: string;
  term: string;
  links: { bot: string | null; cabinet: string | null };
};

export type GrantCurrent = { subscription_id: number; tariff: string | null; kind: string; expire_at: string | null; active: boolean };

export type RedeemPreview = {
  gift: { code: string; tariff: string; days: number; term: string; expires_at: string | null; own: boolean };
  plan: 'new' | 'renewal' | 'change';
  multi: boolean;
  current: GrantCurrent | null;
  choices: GrantCurrent[];
};

export type Redeemed = { gift: RedeemPreview['gift']; order_id: string; plan: RedeemPreview['plan']; subscription_id: number | null };

export type GiftTarget = 'auto' | 'new' | number;

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function giftItem(tariffId: number, periodId: number) {
  return { module: 'gifts', kind: 'gift', payload: { tariff_id: tariffId, period_id: periodId } };
}

export function term(days: number): string {
  if (days >= 28) {
    return count(Math.max(1, Math.round((days * 12) / 365)), 'месяц', 'месяца', 'месяцев');
  }
  return count(days, 'день', 'дня', 'дней');
}

export function codeDaysText(days: number): string {
  return days > 0 ? t('Код действует {term} с момента оплаты.', { term: term(days) }) : t('Код действует без срока.');
}

export function giftState(g: MyGift): { text: string; tone: 'ink2' | 'warn' | 'muted' } {
  switch (g.status) {
    case 'active':
      return { text: g.expires_at ? t('Ждёт активации до {date}', { date: dateLong(g.expires_at, true) }) : t('Ждёт активации'), tone: 'warn' };
    case 'redeemed':
      return { text: g.redeemed_at ? t('Активирован {date}', { date: dateLong(g.redeemed_at, true) }) : t('Активирован'), tone: 'ink2' };
    case 'expired':
      return { text: g.expires_at ? t('Срок кода истёк {date}', { date: dateLong(g.expires_at, true) }) : t('Срок кода истёк'), tone: 'muted' };
    case 'canceled':
      return { text: t('Отменён: деньги вернули'), tone: 'muted' };
  }
}

function currentName(c: GrantCurrent | null): string {
  return c?.tariff ?? t('текущая подписка');
}

export function planText(p: RedeemPreview, target: GiftTarget): { text: string; tone: 'muted' | 'warn' } {
  if (p.multi && target === 'new') {
    return { text: t('Это будет отдельная подписка со своей ссылкой, текущие останутся как есть.'), tone: 'muted' };
  }
  if (p.plan === 'renewal') {
    return { text: t('Срок добавится к подписке «{name}».', { name: currentName(p.current) }), tone: 'muted' };
  }
  if (p.plan === 'change' && p.current?.kind === 'trial') {
    return { text: t('Пробный период закончится, подписка «{name}» начнёт работать сразу.', { name: p.gift.tariff }), tone: 'muted' };
  }
  if (p.plan === 'change') {
    return {
      text: t('Будет переход на тариф «{name}». Текущая подписка ({current}) будет заменена, оставшиеся дни не переносятся.', { name: p.gift.tariff, current: currentName(p.current) }),
      tone: 'warn',
    };
  }
  return { text: t('Подписка начнёт работать сразу после активации.'), tone: 'muted' };
}

export function targetBody(code: string, target: GiftTarget) {
  return {
    code,
    subscription_id: typeof target === 'number' ? target : null,
    new: target === 'new',
  };
}

export function priceOf(p: PeriodOffer, starsMode: boolean): Money | number {
  return starsMode && p.stars ? p.stars : p.price;
}

export async function shareLink(url: string, text: string): Promise<'shared' | 'copied' | 'failed'> {
  const nav = navigator as Navigator & { share?: (data: { url: string; text: string }) => Promise<void> };
  if (typeof nav.share === 'function') {
    try {
      await nav.share({ url, text });
      return 'shared';
    } catch {
      return 'failed';
    }
  }
  try {
    await navigator.clipboard.writeText(url);
    return 'copied';
  } catch {
    return 'failed';
  }
}
