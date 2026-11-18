import { ApiError } from '../../lib/api';
import { count, dateLong, days as daysText, traffic } from '../../lib/format';
import type { TrafficReset } from '../tariffs/model';

export type LinkState = 'active' | 'expired' | 'exhausted' | 'disabled';

export type PromoLink = {
  id: number;
  code: string;
  name: string;
  squads: string[];
  days: number;
  traffic_limit_bytes: number | null;
  traffic_reset: TrafficReset;
  devices: number;
  max_activations: number | null;
  activations: number;
  expires_at: string | null;
  enabled: boolean;
  created_at: string;
  disabled_at: string | null;
  state: LinkState;
  purchased: number;
  bot_link: string | null;
};

export type LinkPage = { items: PromoLink[]; next: number | null };

export type Filter = 'all' | 'active' | 'off';

export const FILTERS: Array<[Filter, string]> = [
  ['all', 'Все'],
  ['active', 'Работают'],
  ['off', 'Не работают'],
];

export const ROOT = '/api/admin/m/promolinks';

export type SquadBrief = { uuid: string; name: string; nodes: number | null };

export type Draft = {
  name: string;
  code: string;
  squads: string[];
  days: string;
  traffic: string;
  reset: TrafficReset;
  devices: string;
  max: string;
  until: string;
};

export const EMPTY_DRAFT: Draft = {
  name: '',
  code: '',
  squads: [],
  days: '7',
  traffic: '',
  reset: 'no_reset',
  devices: '1',
  max: '',
  until: '',
};

export type DraftErrors = Partial<Record<'name' | 'code' | 'squads' | 'days' | 'traffic' | 'devices' | 'max' | 'until', string>>;

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function disabledModule(err: unknown): boolean {
  return err instanceof ApiError && err.code === 'module.disabled';
}

const CODE_CHARS = 'abcdefghjkmnpqrstuvwxyz23456789';

export function inventCode(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => CODE_CHARS[b % CODE_CHARS.length]).join('');
}

function whole(raw: string, min: number, max: number): number | null | 'bad' {
  const clean = raw.trim();
  if (!clean) {
    return null;
  }
  if (!/^\d{1,7}$/.test(clean)) {
    return 'bad';
  }
  const value = Number(clean);
  return value < min || value > max ? 'bad' : value;
}

export function buildBody(draft: Draft): { body: Record<string, unknown>; errors: DraftErrors } {
  const errors: DraftErrors = {};
  const name = draft.name.trim();
  if (!name) {
    errors.name = 'Назовите ссылку: так её будет проще найти в списке';
  } else if (name.length > 100) {
    errors.name = 'Не длиннее 100 знаков';
  }
  const code = draft.code.trim();
  if (code && !/^[A-Za-z0-9_-]{4,32}$/.test(code)) {
    errors.code = 'От 4 до 32 латинских букв, цифр, «-» или «_»';
  }
  if (!draft.squads.length) {
    errors.squads = 'Отметьте хотя бы один сквад';
  }
  const days = whole(draft.days, 1, 3650);
  if (days === null || days === 'bad') {
    errors.days = 'От 1 до 3650 дней';
  }
  const gb = whole(draft.traffic, 0, 1_000_000);
  if (gb === 'bad') {
    errors.traffic = 'Целое число гигабайт или пусто';
  }
  const devices = whole(draft.devices, 0, 100);
  if (devices === 'bad') {
    errors.devices = 'От 0 до 100';
  }
  const max = whole(draft.max, 1, 10_000_000);
  if (max === 'bad') {
    errors.max = 'Целое число больше нуля или пусто';
  }
  let until: string | null = null;
  if (draft.until) {
    const end = new Date(`${draft.until}T23:59:59`);
    if (Number.isNaN(end.getTime())) {
      errors.until = 'Неверная дата';
    } else if (end.getTime() <= Date.now()) {
      errors.until = 'Дата уже прошла';
    } else {
      until = end.toISOString();
    }
  }
  return {
    errors,
    body: {
      name,
      code: code || null,
      squads: draft.squads,
      days: typeof days === 'number' ? days : 0,
      traffic_gb: typeof gb === 'number' && gb > 0 ? gb : null,
      traffic_reset: typeof gb === 'number' && gb > 0 ? draft.reset : 'no_reset',
      devices: typeof devices === 'number' ? devices : 0,
      max_activations: typeof max === 'number' ? max : null,
      expires_at: until,
    },
  };
}

export function giveText(l: PromoLink): string {
  const parts = [daysText(l.days)];
  parts.push(l.traffic_limit_bytes ? traffic(l.traffic_limit_bytes) : 'трафик без лимита');
  parts.push(l.devices ? count(l.devices, 'устройство', 'устройства', 'устройств') : 'устройства без ограничения');
  return parts.join(' · ');
}

export function squadsText(l: PromoLink, squads: SquadBrief[] | null): string {
  if (!squads) {
    return count(l.squads.length, 'сквад', 'сквада', 'сквадов');
  }
  const names = l.squads.map((id) => squads.find((s) => s.uuid === id)?.name ?? 'удалённый сквад');
  return names.join(', ');
}

export function usedText(l: PromoLink): string {
  return l.max_activations ? `${l.activations} из ${l.max_activations}` : String(l.activations);
}

export function boughtText(l: PromoLink): string {
  if (!l.activations) {
    return 'пока никто';
  }
  const share = Math.round((l.purchased / l.activations) * 100);
  return `купили ${l.purchased} (${share}%)`;
}

export function stateOf(l: PromoLink): { text: string; tone: 'ink' | 'warn' | 'muted' } {
  switch (l.state) {
    case 'active':
      return { text: l.expires_at ? `Работает до ${dateLong(l.expires_at, true)}` : 'Работает', tone: 'ink' };
    case 'expired':
      return { text: 'Срок истёк', tone: 'muted' };
    case 'exhausted':
      return { text: 'Активации закончились', tone: 'warn' };
    case 'disabled':
      return { text: 'Выключена', tone: 'muted' };
  }
}

export function emptyText(filter: Filter): string {
  switch (filter) {
    case 'active':
      return 'Работающих ссылок нет';
    case 'off':
      return 'Выключенных и закончившихся ссылок нет';
    default:
      return 'Промо-ссылок пока нет';
  }
}
