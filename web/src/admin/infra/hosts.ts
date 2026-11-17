import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { count, NBSP, plural } from '../../lib/format';
import { inboundLabel } from './InfraNodes';

export type HostData = {
  uuid: string;
  viewPosition: number;
  remark: string;
  address: string;
  port: number;
  isDisabled: boolean;
  isHidden: boolean;
  inbound: { configProfileUuid: string | null; configProfileInboundUuid: string | null };
  nodes: string[];
  internalSquads: { mode: string; squads: string[] } | null;
  sni: string | null;
  host: string | null;
  path: string | null;
  alpn: string | null;
  fingerprint: string | null;
  securityLayer: string | null;
};

export type HostSquad = { uuid: string; name: string; shown: boolean };

export type HostRow = { host: HostData; nodes: string[]; squads: HostSquad[] };

export type HostNode = {
  uuid: string;
  name: string;
  country_code: string;
  address: string;
  state: 'online' | 'connecting' | 'down' | 'disabled';
  inbounds: string[];
};

export type HostInbound = {
  uuid: string;
  tag: string;
  kind: string;
  security: string | null;
  network: string | null;
  port: number | null;
  profile_uuid: string | null;
  profile_name: string;
  nodes: string[];
};

export type HostsData = { hosts: HostRow[]; nodes: HostNode[]; inbounds: HostInbound[] };

export type HostGroup = { key: string; nodes: HostNode[]; rows: HostRow[] };

export type HostDraft = {
  inbound: string;
  remark: string;
  address: string;
  port: string;
  sni: string;
  host: string;
  path: string;
  fingerprint: string;
  alpn: string;
  security: string;
};

export const REMARK_MAX = 100;

export const FINGERPRINTS = ['chrome', 'firefox', 'safari', 'ios', 'android', 'edge', '360', 'qq', 'random', 'randomized'];

export const ALPNS = ['h3', 'h2', 'http/1.1', 'h2,http/1.1', 'h3,h2,http/1.1', 'h3,h2'];

export const SECURITY: Array<[string, string]> = [
  ['DEFAULT', 'Как у входа'],
  ['TLS', 'TLS'],
  ['NONE', 'Без TLS'],
];

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function hostsCount(n: number): string {
  return count(n, 'хост', 'хоста', 'хостов');
}

export function nodesOf(row: HostRow, data: HostsData): HostNode[] {
  return row.nodes.map((uuid) => data.nodes.find((n) => n.uuid === uuid)).filter((n): n is HostNode => Boolean(n));
}

export function inboundOf(row: HostRow, data: HostsData): HostInbound | null {
  const id = row.host.inbound.configProfileInboundUuid;
  return id ? (data.inbounds.find((i) => i.uuid === id) ?? null) : null;
}

export function inboundText(row: HostRow, data: HostsData): string {
  const inbound = inboundOf(row, data);
  return inbound ? inboundLabel(inbound) : 'вход не найден';
}

export function addressText(host: HostData): string {
  const address = host.address.includes(':') && !host.address.startsWith('[') ? `[${host.address}]` : host.address;
  return host.port ? `${address}:${host.port}` : address;
}

export function groupHosts(data: HostsData): HostGroup[] {
  const groups = new Map<string, HostGroup>();
  for (const row of data.hosts) {
    const key = row.nodes.join(',');
    const group = groups.get(key) ?? { key, nodes: nodesOf(row, data), rows: [] };
    group.rows.push(row);
    groups.set(key, group);
  }
  return [...groups.values()].sort((a, b) => Number(a.key === '') - Number(b.key === ''));
}

export function groupName(group: HostGroup): string {
  return group.nodes.length ? group.nodes.map((n) => n.name).join(', ') : 'Нода не определена';
}

export function stateNote(nodes: HostNode[]): { text: string; tone: 'bad' | 'muted' } | null {
  if (nodes.length !== 1) {
    return null;
  }
  const node = nodes[0];
  if (node?.state === 'down') {
    return { text: 'не отвечает', tone: 'bad' };
  }
  if (node?.state === 'disabled') {
    return { text: 'выключена', tone: 'muted' };
  }
  if (node?.state === 'connecting') {
    return { text: 'подключается', tone: 'muted' };
  }
  return null;
}

export function squadsText(row: HostRow): { text: string; warn: boolean } {
  const seen = row.squads.filter((s) => s.shown).map((s) => s.name);
  const hidden = row.squads.filter((s) => !s.shown).map((s) => s.name);
  if (row.squads.length === 0) {
    return { text: 'Вход не добавлен ни в один сквад: клиенты этот сервер не видят', warn: true };
  }
  if (seen.length === 0) {
    return { text: `Скрыт во всех сквадах этого входа: ${hidden.join(', ')}`, warn: true };
  }
  const head = `Видят клиенты ${seen.length === 1 ? 'сквада' : 'сквадов'} ${seen.join(', ')}`;
  const tail = hidden.length ? `, в ${hidden.length === 1 ? 'скваде' : 'сквадах'} ${hidden.join(', ')} скрыт` : '';
  return { text: `${head}${tail}`, warn: false };
}

export function draftOf(row: HostRow | null): HostDraft {
  const host = row?.host;
  return {
    inbound: host?.inbound.configProfileInboundUuid ?? '',
    remark: host?.remark ?? '',
    address: host?.address ?? '',
    port: host ? String(host.port) : '',
    sni: host?.sni ?? '',
    host: host?.host ?? '',
    path: host?.path ?? '',
    fingerprint: host?.fingerprint ?? '',
    alpn: host?.alpn ?? '',
    security: host?.securityLayer || 'DEFAULT',
  };
}

export type DraftErrors = Partial<Record<keyof HostDraft, string>>;

const SPACE = /\s/;

export function draftErrors(draft: HostDraft, creating: boolean): DraftErrors {
  const errors: DraftErrors = {};
  const remark = draft.remark.trim();
  if (creating && !draft.inbound) {
    errors.inbound = 'Выберите вход';
  }
  if (!remark || remark.length > REMARK_MAX) {
    errors.remark = `От 1 до ${REMARK_MAX} символов`;
  }
  const address = draft.address.trim();
  if (!address || SPACE.test(address) || address.length > 253) {
    errors.address = 'IP или домен без пробелов';
  }
  const port = Number(draft.port.trim());
  if (!/^\d+$/.test(draft.port.trim()) || port < 1 || port > 65535) {
    errors.port = 'От 1 до 65535';
  }
  for (const key of ['sni', 'host', 'path'] as const) {
    if (SPACE.test(draft[key].trim())) {
      errors[key] = 'Без пробелов';
    }
  }
  return errors;
}

export function draftBody(draft: HostDraft, creating: boolean): Record<string, unknown> {
  const text = (value: string) => value.trim() || null;
  return {
    ...(creating ? { inbound: draft.inbound } : {}),
    remark: draft.remark.trim(),
    address: draft.address.trim(),
    port: Number(draft.port.trim()),
    sni: text(draft.sni),
    host: text(draft.host),
    path: text(draft.path),
    fingerprint: text(draft.fingerprint),
    alpn: text(draft.alpn),
    security_layer: draft.security,
  };
}

export function hostsSummary(data: HostsData | null, error: string | null): string {
  if (!data) {
    return error ?? 'Загружаем хосты…';
  }
  const total = data.hosts.length;
  if (total === 0) {
    return 'Хостов пока нет';
  }
  const off = data.hosts.filter((r) => r.host.isDisabled).length;
  const hidden = data.hosts.filter((r) => !r.host.isDisabled && r.host.isHidden).length;
  const on = total - off - hidden;
  const parts = [hostsCount(total)];
  if (off === 0 && hidden === 0) {
    parts.push(total === 1 ? 'включён' : 'все включены');
  } else {
    parts.push(`${on}${NBSP}${plural(on, 'включён', 'включены', 'включены')}`);
    if (off) {
      parts.push(`${off}${NBSP}${plural(off, 'выключен', 'выключены', 'выключены')}`);
    }
    if (hidden) {
      parts.push(`${hidden}${NBSP}${plural(hidden, 'скрыт', 'скрыты', 'скрыты')}`);
    }
  }
  return parts.join(' · ');
}

export function useHostsData() {
  const [data, setData] = useState<HostsData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setData(await api<HostsData>('/api/admin/infra/hosts'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Хосты не загрузились'));
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  return { data, error, load };
}

export async function switchHost(row: HostRow): Promise<string> {
  const action = row.host.isDisabled ? 'enable' : 'disable';
  await api(`/api/admin/infra/hosts/${row.host.uuid}/actions/${action}`, { method: 'POST', body: {} });
  return `Хост ${row.host.remark} ${row.host.isDisabled ? 'включён' : 'выключен'}`;
}

export function moveIn(order: string[], index: number, step: -1 | 1): string[] {
  const target = index + step;
  if (target < 0 || target >= order.length) {
    return order;
  }
  const next = [...order];
  const item = next[index] as string;
  next[index] = next[target] as string;
  next[target] = item;
  return next;
}
