export const UTM_FIELDS = ['source', 'medium', 'campaign', 'content', 'term'] as const;
export type UtmField = (typeof UTM_FIELDS)[number];
export type Utm = Partial<Record<UtmField, string>>;

export const TAG_MAX = 64;
export const START_MAX = 64;
export const UTM_TTL_MS = 30 * 24 * 3600 * 1000;

const TAG = /^[A-Za-z0-9-]+(?:_[A-Za-z0-9-]+)*$/;
const PREFIX: Record<UtmField, string> = { source: 's_', medium: 'm_', campaign: 'k_', content: 'n_', term: 't_' };
const TELEGRAM_HOSTS = new Set(['t.me', 'telegram.me']);

export function cleanTag(value: unknown): string | null {
  if (typeof value !== 'string') {
    return null;
  }
  const tag = value.trim();
  return tag.length <= TAG_MAX && TAG.test(tag) ? tag : null;
}

export function pickTags(read: (field: UtmField) => unknown): Utm | null {
  const utm: Utm = {};
  for (const field of UTM_FIELDS) {
    const tag = cleanTag(read(field));
    if (tag) {
      utm[field] = tag;
    }
  }
  return Object.keys(utm).length ? utm : null;
}

export function tagsFrom(search: string): Utm | null {
  const params = new URLSearchParams(search);
  return pickTags((field) => params.get(`utm_${field}`));
}

export function savedTags(raw: string | null, now: number): Utm | null {
  if (!raw) {
    return null;
  }
  try {
    const saved = JSON.parse(raw) as Record<string, unknown> | null;
    if (!saved || typeof saved.at !== 'number' || now - saved.at > UTM_TTL_MS || saved.at > now + 60_000) {
      return null;
    }
    return pickTags((field) => saved[field]);
  } catch {
    return null;
  }
}

export function startParam(utm: Utm | null, referral: string | null): string | null {
  const parts = referral ? [`ref_${referral}`] : [];
  for (const field of UTM_FIELDS) {
    const tag = utm?.[field];
    if (tag) {
      parts.push(`${PREFIX[field]}${tag}`);
    }
  }
  let out = '';
  for (const part of parts) {
    const next = out ? `${out}__${part}` : part;
    if (next.length <= START_MAX) {
      out = next;
    }
  }
  return out || null;
}

export function withBotStart(href: string, bot: string | null | undefined, start: string | null | undefined): string {
  if (!bot || !start) {
    return href;
  }
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return href;
  }
  const host = url.hostname.toLowerCase().replace(/^www\./, '');
  const name = bot.replace(/^@/, '').toLowerCase();
  const path = url.pathname.replace(/\/+$/, '').toLowerCase();
  if (!TELEGRAM_HOSTS.has(host) || path !== `/${name}` || url.search || url.hash) {
    return href;
  }
  url.search = `?start=${start}`;
  return url.toString();
}

const HIDDEN_STARTS = ['gift_', 'pl_', 'auth_'];

function startParts(raw: unknown): string[] {
  return typeof raw === 'string'
    ? raw
        .split('__')
        .map((part) => part.trim())
        .filter(Boolean)
    : [];
}

export function startMark(raw: unknown): string | null {
  const shown = startParts(raw).filter((part) => !HIDDEN_STARTS.some((prefix) => part.startsWith(prefix)));
  return shown.length ? shown.join('__') : null;
}

export function startVia(raw: unknown): 'gift' | 'promo' | null {
  const parts = startParts(raw);
  if (parts.some((part) => part.startsWith('gift_'))) {
    return 'gift';
  }
  if (parts.some((part) => part.startsWith('pl_'))) {
    return 'promo';
  }
  return null;
}

export const START_VIA_TEXT: Record<'gift' | 'promo', string> = {
  gift: 'По ссылке подарка',
  promo: 'По промо-ссылке',
};
