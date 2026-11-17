import { dateLong, NBSP, plural, timeHM } from '../../lib/format';
import type { Subscription } from '../clients/types';
import type { ClientCard, Identity, TariffBrief } from './types';

const PRIORITY: Record<string, number> = { active: 0, limited: 1, disabled: 2, expired: 3 };

export function liveSubscriptions(card: ClientCard): Subscription[] {
  return card.subscriptions
    .filter((s) => !s.ended_at)
    .sort((a, b) => {
      const pa = PRIORITY[a.mirror.status ?? ''] ?? 4;
      const pb = PRIORITY[b.mirror.status ?? ''] ?? 4;
      if (pa !== pb) {
        return pa - pb;
      }
      const ea = a.mirror.expire_at ? Date.parse(a.mirror.expire_at) : 0;
      const eb = b.mirror.expire_at ? Date.parse(b.mirror.expire_at) : 0;
      if (ea !== eb) {
        return eb - ea;
      }
      return b.id - a.id;
    });
}

export function telegramOf(card: ClientCard): Identity | undefined {
  return card.identities.find((i) => i.kind === 'telegram');
}

export function emailOf(card: ClientCard): Identity | undefined {
  return card.identities.find((i) => i.kind === 'email');
}

export function personName(card: ClientCard): string {
  const tg = telegramOf(card);
  const full = [tg?.meta?.first_name, tg?.meta?.last_name]
    .map((part) => (part ?? '').trim())
    .filter(Boolean)
    .join(' ');
  if (full) {
    return full;
  }
  if (tg) {
    return `id ${tg.value}`;
  }
  return `Клиент ${card.client.id}`;
}

export function tariffOf(card: ClientCard, sub: Subscription | null): TariffBrief | null {
  if (!sub?.tariff_id) {
    return null;
  }
  return card.tariffs.find((t) => t.id === sub.tariff_id) ?? null;
}

function sameYear(date: Date): boolean {
  return date.getFullYear() === new Date().getFullYear();
}

export function nextReset(sub: Subscription, tariff: TariffBrief | null, now = new Date()): string {
  if (tariff?.traffic.reset === 'on_payment') {
    return 'при продлении';
  }
  const y = now.getFullYear();
  const m = now.getMonth();
  switch (sub.mirror.traffic_strategy) {
    case 'MONTH': {
      const next = new Date(y, m + 1, 1);
      return dateLong(next.toISOString(), !sameYear(next));
    }
    case 'MONTH_ROLLING': {
      const anchor = new Date(sub.period_started_at ?? sub.created_at).getDate();
      const on = (month: number) => new Date(y, month, Math.min(anchor, new Date(y, month + 1, 0).getDate()));
      const next = on(m) > now ? on(m) : on(m + 1);
      return dateLong(next.toISOString(), !sameYear(next));
    }
    case 'WEEK':
      return 'в понедельник';
    case 'DAY':
      return 'завтра';
    default:
      return '';
  }
}

export function seenText(iso: string | null): string {
  if (!iso) {
    return '';
  }
  const date = new Date(iso);
  const minutes = Math.floor((Date.now() - date.getTime()) / 60_000);
  if (minutes < 1) {
    return 'только что';
  }
  if (minutes < 60) {
    return `${minutes}${NBSP}${plural(minutes, 'минуту', 'минуты', 'минут')} назад`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}${NBSP}${plural(hours, 'час', 'часа', 'часов')} назад`;
  }
  return dateLong(iso);
}

export function visitText(iso: string | null): string {
  if (!iso) {
    return 'не было';
  }
  const date = new Date(iso);
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = Math.round((start(new Date()) - start(date)) / 86_400_000);
  if (diff === 0) {
    return `сегодня в ${timeHM(iso)}`;
  }
  if (diff === 1) {
    return `вчера в ${timeHM(iso)}`;
  }
  return `${dateLong(iso)} в ${timeHM(iso)}`;
}

const MONTHS_GEN = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];

export function sinceText(iso: string): string {
  const date = new Date(iso);
  return `с нами с ${MONTHS_GEN[date.getMonth()] ?? ''}${NBSP}${date.getFullYear()}`;
}

export function devicesWord(platform: string | null): 'phone' | 'laptop' {
  const p = (platform ?? '').toLowerCase();
  if (p.includes('mac') || p.includes('windows') || p.includes('linux') || p.includes('darwin')) {
    return 'laptop';
  }
  return 'phone';
}

export function platformName(platform: string | null): string | null {
  if (!platform) {
    return null;
  }
  const p = platform.toLowerCase();
  const map: Record<string, string> = {
    ios: 'iOS',
    ipados: 'iPadOS',
    android: 'Android',
    macos: 'macOS',
    darwin: 'macOS',
    windows: 'Windows',
    linux: 'Linux',
    tvos: 'tvOS',
    androidtv: 'Android TV',
  };
  return map[p] ?? platform;
}

export function responseText(response: string): string {
  const r = response.toUpperCase();
  if (r === 'BLOCK' || r.startsWith('STATUS_CODE')) {
    return 'отказано';
  }
  if (r === 'BROWSER') {
    return 'открыта страница подписки';
  }
  return 'выдан конфиг';
}
