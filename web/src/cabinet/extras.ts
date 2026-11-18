import { count, type Money } from '../lib/format';
import { t } from '../lib/i18n';
import type { CheckoutMethod, Quote, TrafficReset } from './model';

export type Unavailable = { code: string; message: string };

export type Availability<T> = { offer: T | null; unavailable: Unavailable | null };

export type DevicesOffer = {
  subscription_id: number;
  included: number;
  extra_now: number;
  can_add: number;
  days: number;
  monthly: Money;
  monthly_stars: number | null;
  per_device: Money;
  per_device_stars: number | null;
  expire_at: string;
};

export type TrafficPack = { gb: number; price: Money; stars: number | null };

export type TrafficOffer = {
  subscription_id: number;
  packs: TrafficPack[];
  limit_bytes: number | null;
  used_bytes: number | null;
  extra_bytes: number;
  leftover: 'burn' | 'carry';
  reset: TrafficReset;
  expire_at: string;
  window_ends_at: string | null;
};

export type AddonsOffer = {
  subscription_id: number;
  devices: Availability<DevicesOffer>;
  traffic: Availability<TrafficOffer>;
};

export type AddonKind = 'devices' | 'traffic';

export type AddonChoice = { kind: 'devices'; count: number } | { kind: 'traffic'; gb: number };

export type ItemQuote = { quote: Quote['quote']; methods: CheckoutMethod[] };

export type PayResult = { type: 'redirect'; payment_id: string; pay_url: string } | { type: 'completed'; order_id: string };

export function addonItem(subscriptionId: number, choice: AddonChoice) {
  return {
    module: 'options',
    kind: choice.kind,
    payload: choice.kind === 'devices' ? { subscription_id: subscriptionId, count: choice.count } : { subscription_id: subscriptionId, gb: choice.gb },
    subscription_id: subscriptionId,
  };
}

export function offerUrl(subscriptionId: number): string {
  return `/api/client/m/options/offer/${subscriptionId}`;
}

const PERIODIC: TrafficReset[] = ['day', 'week', 'month', 'month_rolling'];

export function leftoverText(offer: TrafficOffer): string {
  const periodic = PERIODIC.includes(offer.reset);
  if (offer.leftover === 'carry') {
    return periodic ? t('Неизрасходованный остаток перейдёт на следующий период после сброса трафика.') : t('Неизрасходованный остаток сохранится после продления.');
  }
  return periodic ? t('Что не израсходуете до сброса трафика, сгорит.') : t('Что не израсходуете до продления, сгорит.');
}

export function daysText(n: number): string {
  return count(n, 'день', 'дня', 'дней');
}

export function devicesText(n: number): string {
  return count(n, 'устройство', 'устройства', 'устройств');
}

export function firstKind(offer: AddonsOffer, asked: string | null): AddonKind {
  if (asked === 'traffic' && offer.traffic.offer) {
    return 'traffic';
  }
  if (asked === 'devices' && offer.devices.offer) {
    return 'devices';
  }
  return offer.devices.offer && offer.devices.offer.can_add > 0 ? 'devices' : offer.traffic.offer ? 'traffic' : 'devices';
}

export function doneText(choice: AddonChoice): string {
  return choice.kind === 'devices' ? t('Готово, лимит устройств вырос на {n}.', { n: choice.count }) : t('Готово, лимит трафика вырос на {n} ГБ.', { n: choice.gb });
}
