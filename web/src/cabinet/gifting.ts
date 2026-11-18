import { ApiError } from '../lib/api';
import { count, dateLong, type Money } from '../lib/format';
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
  return days > 0 ? `Код действует ${term(days)} с момента оплаты.` : 'Код действует без срока.';
}

export function giftState(g: MyGift): { text: string; tone: 'ink2' | 'warn' | 'muted' } {
  switch (g.status) {
    case 'active':
      return { text: g.expires_at ? `Ждёт активации до ${dateLong(g.expires_at, true)}` : 'Ждёт активации', tone: 'warn' };
    case 'redeemed':
      return { text: g.redeemed_at ? `Активирован ${dateLong(g.redeemed_at, true)}` : 'Активирован', tone: 'ink2' };
    case 'expired':
      return { text: g.expires_at ? `Срок кода истёк ${dateLong(g.expires_at, true)}` : 'Срок кода истёк', tone: 'muted' };
    case 'canceled':
      return { text: 'Отменён: деньги вернули', tone: 'muted' };
  }
}

function currentName(c: GrantCurrent | null): string {
  return c?.tariff ?? 'текущая подписка';
}

export function planText(p: RedeemPreview, target: GiftTarget): { text: string; tone: 'muted' | 'warn' } {
  if (p.multi && target === 'new') {
    return { text: 'Это будет отдельная подписка со своей ссылкой, текущие останутся как есть.', tone: 'muted' };
  }
  if (p.plan === 'renewal') {
    return { text: `Срок добавится к подписке «${currentName(p.current)}».`, tone: 'muted' };
  }
  if (p.plan === 'change' && p.current?.kind === 'trial') {
    return { text: `Пробный период закончится, подписка «${p.gift.tariff}» начнёт работать сразу.`, tone: 'muted' };
  }
  if (p.plan === 'change') {
    return {
      text: `Будет переход на тариф «${p.gift.tariff}». Текущая подписка (${currentName(p.current)}) будет заменена, оставшиеся дни не переносятся.`,
      tone: 'warn',
    };
  }
  return { text: 'Подписка начнёт работать сразу после активации.', tone: 'muted' };
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
