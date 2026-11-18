import { api, ApiError } from '../lib/api';
import { days, money, type Money } from '../lib/format';
import { t } from '../lib/i18n';
import type { SubscriptionView } from './model';

const PROMO = '/api/client/m/promo';

export type PromoFound = { code: string; label: string; discount: boolean } & (
  | { type: 'percent'; percent: number }
  | { type: 'fixed'; amount: Money }
  | { type: 'days'; days: number }
);

export type Activated = { code: string; days: number; subscription_id: number };

export type Handoff = { promo?: unknown } | null;

export function handedCode(state: unknown): string | null {
  const promo = (state as Handoff)?.promo;
  return typeof promo === 'string' && promo.trim() ? promo.trim().toUpperCase() : null;
}

export function checkPromo(code: string, tariffId?: number | null): Promise<PromoFound> {
  return api<PromoFound>(`${PROMO}/check`, { method: 'POST', body: tariffId ? { code, tariff_id: tariffId } : { code } });
}

export function activatePromo(code: string, subscriptionId: number): Promise<Activated> {
  return api<Activated>(`${PROMO}/activate`, { method: 'POST', body: { code, subscription_id: subscriptionId } });
}

export function promoTargets(list: SubscriptionView[]): SubscriptionView[] {
  return list.filter((v) => !v.subscription.ended_at && v.subscription.remna !== null);
}

export function discountTitle(found: PromoFound): string {
  if (found.type === 'percent') {
    return t('Промокод {code}: скидка {percent}%', { code: found.code, percent: found.percent });
  }
  if (found.type === 'fixed') {
    return t('Промокод {code}: скидка {amount}', { code: found.code, amount: money(found.amount) });
  }
  return t('Промокод {code}: +{days}', { code: found.code, days: days(found.days) });
}

export function promoFailText(err: unknown, checkout = false): string {
  if (!(err instanceof ApiError)) {
    return t('Промокод не применился. Попробуйте ещё раз');
  }
  switch (err.code) {
    case 'promo.not_found':
      return checkout ? t('Промокод не найден, уже не действует или не подходит к этому тарифу') : t('Промокод не найден или уже не действует. Проверьте, нет ли опечатки');
    case 'promo.already_used':
      return t('Этот промокод вы уже использовали');
    case 'promo.no_subscription':
      return t('К этой подписке нельзя добавить дни: она закончилась или ещё не готова');
    case 'promo.reserved':
      return checkout ? t('Промокод уже применён к неоплаченному счёту. Оплатите его или подождите, пока счёт истечёт') : t('Этот промокод уже активируется. Обновите страницу через минуту');
    case 'promo.rate_limited':
      return t('Слишком много неверных промокодов. Попробуйте снова через 15 минут');
    case 'promo.protected':
      return t('Промокоды на дни недоступны для этого аккаунта. Если это ошибка, напишите в поддержку');
    case 'promo.not_yours':
      return t('Этот промокод выдан как приз другому аккаунту');
    case 'promo.discount_code':
      return t('Это промокод на скидку: он применится при оплате');
    case 'module.disabled':
      return t('Промокоды сейчас не принимаются');
    case 'core.rejected':
      return t('Дни не добавились: сервер подписок не ответил. Попробуйте ещё раз через минуту');
    default:
      return err.message;
  }
}
