import type { Subscription } from '../admin/clients/types';
import { count, dateLong, days, type Money } from '../lib/format';
import { lang, msg, t } from '../lib/i18n';
import type { Currency } from '../lib/types';

export type TrafficReset = 'no_reset' | 'day' | 'week' | 'month' | 'month_rolling' | 'on_payment';

export type TariffBrief = {
  id: number;
  name: string;
  on_sale: boolean;
  devices: { included: number; extra: { max: number; monthly: { usd?: number; rub?: number }; monthly_stars: number | null } | null };
  traffic: { limit_bytes: number | null; reset: TrafficReset };
};

export type SubStatus = 'active' | 'expiring' | 'trial' | 'limited' | 'expired' | 'blocked' | 'no_subscription';

export type SubscriptionView = {
  subscription: Subscription;
  status: SubStatus;
  progress: { days_left: number; days_total: number } | null;
  subscription_url: string | null;
  countries: string[];
  tariff: TariffBrief | null;
};

export type ClientMe = {
  client: { id: number; status: 'active' | 'blocked'; created_at: string; locale: string };
  locale?: 'ru' | 'en';
  balance: Money;
  balance_held?: Money;
  balance_enabled?: boolean;
  multi_subscriptions?: boolean;
  autorenew_card?: boolean;
  referral_balance: Money;
  currency: Currency;
  channel: 'cabinet' | 'miniapp';
  full_access: boolean;
  two_factor: TwoFactor;
  payments_count?: number;
  email?: string | null;
  email_change?: boolean;
  has_password?: boolean;
};

export type TwoFactor = { enabled: boolean; enabled_at: string | null };

export function heldBalance(me: ClientMe): Money | null {
  return me.balance_held && me.balance_held.minor > 0 ? me.balance_held : null;
}

export type Device = { hwid: string; title: string | null; platform: string | null; os_version: string | null; app: string | null; added_at: string | null };

export type PeriodOffer = {
  period_id: number;
  days: number;
  months: number;
  price: Money;
  per_month: Money;
  savings_percent: number | null;
  stars: number | null;
  stars_per_month: number | null;
  stars_savings_percent: number | null;
};

export type TariffOffer = {
  category: { id: number; name: string; sort: number; visible: boolean };
  tariff: TariffBrief & { category_id: number; description: string | null; sort: number };
  periods: PeriodOffer[];
};

export type Catalog = { currency: Currency; tariffs: TariffOffer[] };

export type CheckoutMethod = {
  provider: string;
  method: string | null;
  kind: 'external' | 'stars' | 'balance';
  title: string;
  hint: string;
  amount: { minor?: number; currency?: Currency; stars?: number } | number | null;
  unavailable: null | { type: 'insufficient_balance'; balance: Money; missing: Money } | { type: 'no_stars_price' };
};

export type Quote = {
  quote: {
    currency: Currency;
    lines: Array<{ kind: { type: string }; label: string; amount: Money; stars: number | null }>;
    subtotal: Money;
    discount: Money;
    total: Money;
    stars: { subtotal: number; discount: number; total: number } | null;
    days: number;
    extra_devices: number;
  };
  kind: 'new' | 'renewal' | 'change';
  expire_preview: string | null;
  methods: CheckoutMethod[];
  current?: { subscription_id: number; tariff: string | null; trial: boolean; expire_at: string | null } | null;
};

export function transitionText(current: string | null): string {
  return t('Текущая подписка ({current}) будет удалена, новая станет доступна сразу после оплаты. Оставшиеся дни не переносятся, ссылка подписки останется прежней.', {
    current: current ?? t('без тарифа'),
  });
}

export type WithdrawalStatus = 'holding' | 'pending' | 'paid' | 'rejected' | 'canceled';

export type PayoutOption = {
  id: string;
  title: string;
  hint: string;
  min: Money | null;
};

export type Withdrawal = {
  id: number;
  amount: Money;
  method: string;
  method_title?: string;
  details: string;
  status: WithdrawalStatus;
  comment: string | null;
  created_at: string;
  decided_at: string | null;
};

export type Referral = {
  code: string;
  bot_link: string | null;
  invited: number;
  earned: Money;
  balance?: Money;
  levels: number[];
  withdraw_min?: Money | null;
  withdrawals_enabled?: boolean;
  payout_methods?: PayoutOption[];
  withdrawals?: Withdrawal[];
};

export const GB = 1024 ** 3;

export function stars(n: number): string {
  return count(n, 'звезда', 'звезды', 'звёзд');
}

export function gb(bytes: number): string {
  const value = bytes / GB;
  const rounded = value >= 100 ? Math.round(value) : Math.round(value * 10) / 10;
  return lang() === 'en' ? String(rounded) : String(rounded).replace('.', ',');
}

export function gbText(bytes: number): string {
  return t('{n} ГБ', { n: gb(bytes) });
}

function nextMonthFirst(now = new Date()): Date {
  return new Date(now.getFullYear(), now.getMonth() + 1, 1);
}

function nextAnniversary(start: string | null, now = new Date()): Date | null {
  if (!start) {
    return null;
  }
  const day = new Date(start).getDate();
  const clamp = (y: number, m: number) => new Date(y, m, Math.min(day, new Date(y, m + 1, 0).getDate()));
  const thisMonth = clamp(now.getFullYear(), now.getMonth());
  return thisMonth > now ? thisMonth : clamp(now.getFullYear(), now.getMonth() + 1);
}

export function resetWhen(reset: TrafficReset, sub: Subscription | null): { tail: string; until: string } {
  switch (reset) {
    case 'month': {
      const date = dateLong(nextMonthFirst().toISOString());
      return { tail: t('обновится {date}', { date }), until: t('До {date}', { date }) };
    }
    case 'month_rolling': {
      const next = nextAnniversary(sub?.period_started_at ?? sub?.created_at ?? null);
      const date = next ? dateLong(next.toISOString()) : '';
      return {
        tail: date ? t('обновится {date}', { date }) : t('обновится в день оформления'),
        until: date ? t('До {date}', { date }) : t('До дня оформления'),
      };
    }
    case 'week':
      return { tail: t('обновится в понедельник'), until: t('До понедельника') };
    case 'day':
      return { tail: t('обновится завтра'), until: t('До завтра') };
    case 'on_payment':
      return { tail: t('обновится при продлении'), until: t('До продления') };
    default:
      return { tail: t('на весь срок'), until: '' };
  }
}

export function resetPlan(reset: TrafficReset): string {
  switch (reset) {
    case 'month':
      return t('в месяц, обновляется 1-го числа');
    case 'month_rolling':
      return t('в месяц, обновляется в день оформления');
    case 'week':
      return t('в неделю, обновляется по понедельникам');
    case 'day':
      return t('в день, обновляется каждый день');
    case 'on_payment':
      return t('на оплаченный срок, обновляется при каждой оплате');
    default:
      return t('на весь срок');
  }
}

export type Tone = 'ink2' | 'warn' | 'muted' | 'bad';

export function statusWord(view: SubscriptionView | null, blocked: boolean): { text: string; tone: Tone } {
  if (blocked || view?.status === 'blocked') {
    return { text: t('Заблокирована'), tone: 'bad' };
  }
  if (!view) {
    return { text: t('Нет подписки'), tone: 'muted' };
  }
  switch (view.status) {
    case 'limited':
      return { text: t('Трафик закончился'), tone: 'warn' };
    case 'expired':
      return { text: t('Истекла'), tone: 'muted' };
    case 'expiring':
      return { text: t('Скоро закончится'), tone: 'warn' };
    case 'trial':
      return { text: t('Пробный период'), tone: 'ink2' };
    default:
      return { text: t('Активна'), tone: 'ink2' };
  }
}

export function periodTitle(view: SubscriptionView): string {
  const name = view.tariff?.name ?? (view.subscription.kind === 'trial' ? t('Пробный период') : t('Подписка'));
  const d = view.subscription.period_days;
  if (!d || view.subscription.kind === 'trial') {
    return name;
  }
  const months = Math.max(1, Math.round((d * 12) / 365));
  const label = d >= 28 ? count(months, 'месяц', 'месяца', 'месяцев') : days(d);
  return `${name} · ${label}`;
}

export function trafficLine(view: SubscriptionView): { used: string; tail: string; exhausted: boolean } | null {
  const limit = view.subscription.mirror.traffic_limit_bytes ?? view.tariff?.traffic.limit_bytes ?? 0;
  if (!limit) {
    return null;
  }
  const exhausted = view.status === 'limited';
  const used = exhausted ? limit : (view.subscription.mirror.traffic_used_bytes ?? 0);
  const reset = view.tariff?.traffic.reset ?? 'no_reset';
  const when = resetWhen(reset, view.subscription);
  return {
    used: t('{used} из {limit} ГБ', { used: gb(used), limit: gb(limit) }),
    tail: reset === 'no_reset' ? ` ${t('на весь срок')}` : ` · ${when.tail}`,
    exhausted,
  };
}

export type TrafficMeter = { used: string; limit: string; left: string; share: number; tail: string; exhausted: boolean };

export function trafficMeter(view: SubscriptionView): TrafficMeter | null {
  const limit = view.subscription.mirror.traffic_limit_bytes ?? view.tariff?.traffic.limit_bytes ?? 0;
  if (!limit) {
    return null;
  }
  const exhausted = view.status === 'limited';
  const used = exhausted ? limit : Math.min(Math.max(view.subscription.mirror.traffic_used_bytes ?? 0, 0), limit);
  const reset = view.tariff?.traffic.reset ?? 'no_reset';
  return {
    used: gbText(used),
    limit: gbText(limit),
    left: gbText(Math.max(limit - used, 0)),
    share: Math.min(1, used / limit),
    tail: resetWhen(reset, view.subscription).tail,
    exhausted,
  };
}

export function explanation(view: SubscriptionView | null, blocked: boolean): string | null {
  if (blocked || view?.status === 'blocked') {
    return t('Подписка заблокирована. Напишите в поддержку.');
  }
  if (!view) {
    return null;
  }
  if (view.status === 'limited') {
    const reset = view.tariff?.traffic.reset ?? 'no_reset';
    const until = resetWhen(reset, view.subscription).until;
    const head = until ? t('{until} VPN не работает.', { until }) : t('VPN не работает.');
    return `${head} ${reset === 'on_payment' ? t('После оплаты трафик обновится.') : t('Продление трафик не добавляет.')}`;
  }
  if (view.status === 'expired') {
    return t('VPN не работает, пока подписка не продлена. После оплаты обновите подписку в приложении — ссылка останется прежней.');
  }
  return null;
}

export type Platform = 'ios' | 'android' | 'win' | 'mac' | 'linux' | 'pc';

export type AppInfo = { app: string; store: string; storeShort: string; download: string; others: string; add: (url: string) => string };

const HAPP_ADD = (url: string) => `happ://add/${url}`;
const HIDDIFY_ADD = (url: string) => `hiddify://import/${url}`;

export const APPS: Record<Platform, AppInfo> = {
  ios: { app: 'Happ', store: msg('App Store, бесплатно'), storeShort: 'App Store', download: 'https://apps.apple.com/app/happ-proxy-utility/id6504287215', others: 'Streisand, v2rayTun', add: HAPP_ADD },
  android: { app: 'Happ', store: msg('Google Play или файл APK'), storeShort: 'Google Play', download: 'https://play.google.com/store/apps/details?id=com.happproxy', others: 'v2rayTun, Hiddify, NekoBox', add: HAPP_ADD },
  win: { app: 'Hiddify', store: msg('Установщик для Windows 10 и 11'), storeShort: msg('Скачать'), download: 'https://github.com/hiddify/hiddify-app/releases', others: 'Clash Verge, v2rayN', add: HIDDIFY_ADD },
  mac: { app: 'Hiddify', store: msg('Установщик .dmg для macOS'), storeShort: msg('Скачать'), download: 'https://github.com/hiddify/hiddify-app/releases', others: 'Happ, v2rayTun', add: HIDDIFY_ADD },
  linux: { app: 'Hiddify', store: msg('Пакеты .deb и AppImage'), storeShort: msg('Скачать'), download: 'https://github.com/hiddify/hiddify-app/releases', others: 'Clash Verge, NekoBox', add: HIDDIFY_ADD },
  pc: { app: 'Hiddify', store: msg('Windows, macOS и Linux'), storeShort: msg('Скачать'), download: 'https://github.com/hiddify/hiddify-app/releases', others: 'Clash Verge, v2rayN', add: HIDDIFY_ADD },
};

export function guessPlatform(): Platform {
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent;
  if (/iPhone|iPad|iPod/i.test(ua)) {
    return 'ios';
  }
  if (/Android/i.test(ua)) {
    return 'android';
  }
  if (/Mac OS X|Macintosh/i.test(ua)) {
    return 'mac';
  }
  if (/Linux/i.test(ua)) {
    return 'linux';
  }
  return 'win';
}

export function deviceIcon(device: Device): 'phone' | 'laptop' {
  const p = `${device.platform ?? ''}`.toLowerCase();
  return p.includes('ios') || p.includes('android') || p.includes('ipad') ? 'phone' : 'laptop';
}

export function appName(device: Device): string {
  return (device.app ?? '').replace(/\s+[\d.]+$/, '');
}

export function amountMinor(m: Money): number {
  return m.minor;
}
