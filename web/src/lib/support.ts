import type { Site } from './types';

export function supportTickets(site: Site): boolean {
  return site.support_tickets !== false;
}

export function supportLink(site: Site): string | null {
  const url = site.support_url?.trim();
  return url ? url : null;
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
