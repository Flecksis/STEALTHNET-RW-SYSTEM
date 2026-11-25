import type { Site } from './types';

export function supportTickets(site: Site): boolean {
  return site.support_tickets !== false;
}

export function supportLink(site: Site): string | null {
  const url = site.support_url?.trim();
  return url ? webLink(url) : null;
}

function webLink(url: string): string {
  const found = /^tg:\/\/resolve\/?\?(.+)$/i.exec(url);
  const params = new URLSearchParams(found?.[1] ?? '');
  const name = params.get('domain') ?? '';
  if (!/^[A-Za-z0-9_]{4,64}$/.test(name)) {
    return url;
  }
  params.delete('domain');
  const rest = params.toString();
  return `https://t.me/${name}${rest ? `?${rest}` : ''}`;
}

export function supportWay(site: Site, screen: string): { to?: string; url?: string } | null {
  if (supportTickets(site)) {
    return { to: screen };
  }
  const url = supportLink(site);
  return url ? { url } : null;
}

export function telegramLink(raw: string): string | null {
  const value = raw.trim();
  if (!value) {
    return null;
  }
  const at = /^@([A-Za-z0-9_]{4,64})$/.exec(value);
  if (at) {
    return `https://t.me/${at[1]}`;
  }
  const bare = /^(?:https?:\/\/)?(?:t\.me|telegram\.me)\/(\S+)$/i.exec(value);
  if (bare) {
    return `https://t.me/${bare[1]}`;
  }
  if (/^(https:\/\/|tg:\/\/)\S+$/.test(value)) {
    return value;
  }
  if (/^http:\/\/\S+$/.test(value)) {
    return null;
  }
  if (/^[a-z0-9-]+(\.[a-z0-9-]+)+(\/\S*)?$/i.test(value)) {
    return `https://${value}`;
  }
  return null;
}
