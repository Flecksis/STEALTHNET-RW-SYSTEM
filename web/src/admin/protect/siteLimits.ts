export type SiteDay = { created: number; address: number; network: number; minute: number; asked: number };

export type SiteHidden = { kind: 'cloudflare' | 'internal'; address: string; at: string };

export type SiteState = { day: SiteDay; hidden: SiteHidden | null; trusted?: string | null };

export type SiteLimits = { site_address_day: number; site_network_day: number; site_minute: number };

export const SITE_NUMBERS: Array<[keyof SiteLimits, string, string]> = [
  ['site_address_day', 'Новых аккаунтов с одного адреса за сутки', 'Следующим — отказ. За общим Wi-Fi и мобильным адресом людей много, разумное значение — 10 и больше. 0 — без предела.'],
  ['site_network_day', 'Новых аккаунтов с одной сети IPv6 за сутки', 'Сеть /48. Следующим — отказ. 0 — без предела.'],
  ['site_minute', 'Новых аккаунтов в минуту на всю панель', 'Сверх предела — капча, если она настроена, иначе отказ до конца минуты. 0 — без предела.'],
];

export type Paces = { email_ip: number; telegram_ip: number; verify_ip: number };

export const PACE_DEFAULTS: Paces = { email_ip: 5, telegram_ip: 10, verify_ip: 60 };

export const PACE_MAX = 1000;

export const PACE_NUMBERS: Array<[keyof Paces, string, string]> = [
  ['email_ip', 'Обращений по почте', 'Регистрация, код для входа, сброс и смена почты. Обычно 5. На сеть IPv6 /48 — втрое больше, но не меньше 15.'],
  ['telegram_ip', 'Входов через Telegram', 'Кнопка «Войти через Telegram» на сайте. Обычно 10.'],
  ['verify_ip', 'Открытий страницы проверки', 'Страница с капчей перед пробным периодом. Обычно 60.'],
];

export function paceError(label: string, raw: string): string | null {
  const n = Number(raw);
  return /^\d{1,4}$/.test(raw) && n >= 1 && n <= PACE_MAX ? null : `${label}: целое число от 1 до ${PACE_MAX}`;
}

export function pacesOf(s: Partial<Paces>): Paces {
  return {
    email_ip: s.email_ip ?? PACE_DEFAULTS.email_ip,
    telegram_ip: s.telegram_ip ?? PACE_DEFAULTS.telegram_ip,
    verify_ip: s.verify_ip ?? PACE_DEFAULTS.verify_ip,
  };
}

function times(n: number): string {
  const tens = n % 100;
  const ones = n % 10;
  return `${n} ${ones >= 2 && ones <= 4 && (tens < 12 || tens > 14) ? 'раза' : 'раз'}`;
}

export function siteLine(limits: SiteLimits, day: SiteDay): string | null {
  const parts: string[] = [];
  if (limits.site_address_day > 0 || day.address > 0) {
    parts.push(`с одного адреса — ${day.address}`);
  }
  if (limits.site_network_day > 0 || day.network > 0) {
    parts.push(`с одной сети IPv6 — ${day.network}`);
  }
  if (limits.site_minute > 0 || day.minute > 0) {
    parts.push(`в минуту — ${day.minute}`);
  }
  if (parts.length === 0) {
    return null;
  }
  const asked = day.asked > 0 ? ` Капчу сверх минутного предела попросили ${times(day.asked)}.` : '';
  return `Новых аккаунтов на сайте за сутки: ${day.created}. Отказов сверх предела: ${parts.join(', ')}.${asked}`;
}

export function hiddenText(hidden: SiteHidden, when: string): string {
  return hidden.kind === 'cloudflare'
    ? `Панель видит адреса Cloudflare, а не клиентов (последний — ${hidden.address}, ${when}), поэтому суточные пределы на них не действуют. Выключите оранжевое облако для домена панели. Если оно нужно и перед панелью nginx, впишите в deploy/.env строку ниже и выполните stealthnet restart.`
    : `Панель видит внутренний адрес ${hidden.address}, а не клиентов (${when}), поэтому суточные пределы на него не действуют. Прокси перед панелью должен передавать X-Forwarded-For, а его адрес должен быть в SN_TRUSTED_PROXIES в deploy/.env.`;
}

export function hiddenShort(hidden: SiteHidden): string {
  return hidden.kind === 'cloudflare'
    ? 'Панель видит адреса Cloudflare, а не клиентов: суточные пределы новых аккаунтов на них не действуют. Что сделать — в «Настроить».'
    : 'Панель видит внутренний адрес прокси, а не клиентов: суточные пределы новых аккаунтов на него не действуют. Что сделать — в «Настроить».';
}

export function dayLimited(limits: SiteLimits): boolean {
  return limits.site_address_day > 0 || limits.site_network_day > 0;
}
