import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { count, dateLong, groupDigits, money, NBSP, plural, timeHM, type Money } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { countryName, minutesSince, spanText } from '../home/model';
import { usePhone } from '../../lib/media';
import { InfraHead } from './InfraHead';
import { NodesPhone } from './NodesPhone';
import '../form.css';
import './infra.css';

type Inbound = { uuid: string; tag: string; kind: string; security: string | null; network: string | null; port: number | null };

export type NodeRow = {
  node: {
    uuid: string;
    name: string;
    address: string;
    port: number | null;
    isConnected: boolean;
    isDisabled: boolean;
    countryCode: string;
    usersOnline: number;
    xrayUptime: number;
    lastStatusChange: string | null;
    lastStatusMessage: string | null;
    trafficLimitBytes: number | null;
    trafficUsedBytes: number | null;
    trafficResetDay: number | null;
    notifyPercent: number | null;
    consumptionMultiplier: number;
    versions: { xray: string | null; node: string | null } | null;
    provider: { name: string; loginUrl?: string | null } | null;
  };
  state: 'online' | 'connecting' | 'down' | 'disabled';
  traffic_alert: boolean;
  city: string | null;
  system: {
    info: { cpus: number; memory_total?: number; memoryTotal?: number; platform: string; release: string; version?: string };
    stats: { memory_free?: number; memoryFree?: number; memory_used?: number; memoryUsed?: number; load_avg?: number[]; loadAvg?: number[]; uptime: number };
  } | null;
  inbounds: Inbound[];
  profile_uuid: string | null;
  squads: string[];
  billing: { uuid: string; provider: string; login_url: string | null; next_billing_at: string; price: Money | null } | null;
};

export type NodesData = { summary: { total: number; online: number; online_users: number; attention: number }; nodes: NodeRow[] };

type Profile = { uuid: string; name: string; inbounds: Inbound[] };
type Provider = { uuid: string; name: string; login_url: string | null };

const TB = 1024 ** 4;
const GB = 1024 ** 3;

const COUNTRIES = ['NL', 'DE', 'FI', 'KZ', 'RU', 'PL', 'TR', 'FR', 'GB', 'US', 'SE', 'LV', 'EE', 'AE', 'JP', 'SG'];

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function num1(value: number): string {
  return value.toFixed(1).replace('.', ',');
}

export function tb(bytes: number): string {
  return bytes >= 100 * GB ? `${num1(bytes / TB)}${NBSP}ТБ` : `${num1(bytes / GB)}${NBSP}ГБ`;
}

function numTrim(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : num1(rounded);
}

export function limitPair(used: number, limit: number): string {
  return `${num1(used / TB)} из ${numTrim(limit / TB)}${NBSP}ТБ`;
}

const MONTHS_NOM = ['январь', 'февраль', 'март', 'апрель', 'май', 'июнь', 'июль', 'август', 'сентябрь', 'октябрь', 'ноябрь', 'декабрь'];

function trafficSince(day: number | null, now = new Date()): string {
  if (!day) {
    return '';
  }
  const clamp = (year: number, month: number) => new Date(year, month, Math.min(day, new Date(year, month + 1, 0).getDate()));
  const current = clamp(now.getFullYear(), now.getMonth());
  const last = current <= now ? current : clamp(now.getFullYear(), now.getMonth() - 1);
  return day === 1 ? `за ${MONTHS_NOM[last.getMonth()]}` : `с ${dateLong(last.toISOString())}`;
}

export function inboundLabel(inbound: Inbound): string {
  const network = (inbound.network ?? '').toLowerCase();
  const kind = inbound.kind.toUpperCase();
  const security = inbound.security && inbound.security !== 'none' ? ` ${inbound.security.charAt(0).toUpperCase()}${inbound.security.slice(1)}` : '';
  const base = network === 'xhttp' ? 'XHTTP' : `${kind}${security}`;
  return inbound.port ? `${base} · ${inbound.port}` : base;
}

function memoryOf(row: NodeRow): { used: number; total: number } | null {
  const info = row.system?.info;
  const stats = row.system?.stats;
  const total = info?.memory_total ?? info?.memoryTotal ?? 0;
  const used = stats?.memory_used ?? stats?.memoryUsed ?? 0;
  return total > 0 ? { used, total } : null;
}

function cpuOf(row: NodeRow): number | null {
  const load = row.system?.stats.load_avg ?? row.system?.stats.loadAvg;
  const cpus = row.system?.info.cpus ?? 0;
  if (!load || load.length === 0 || cpus <= 0) {
    return null;
  }
  return Math.min(100, Math.round(((load[0] ?? 0) / cpus) * 100));
}

function uptimeDays(seconds: number): string {
  const daysCount = Math.floor(seconds / 86_400);
  return daysCount > 0 ? count(daysCount, 'день', 'дня', 'дней') : spanText(Math.floor(seconds / 60));
}

export function trafficPercent(row: NodeRow): number | null {
  const limit = row.node.trafficLimitBytes ?? 0;
  if (limit <= 0) {
    return null;
  }
  return Math.round(((row.node.trafficUsedBytes ?? 0) / limit) * 100);
}

export function resetDate(day: number | null): string {
  if (!day) {
    return '';
  }
  const now = new Date();
  const candidate = new Date(now.getFullYear(), now.getMonth(), day);
  const next = candidate > now ? candidate : new Date(now.getFullYear(), now.getMonth() + 1, day);
  return dateLong(next.toISOString());
}

function billingSoon(row: NodeRow): boolean {
  if (!row.billing) {
    return false;
  }
  const left = (Date.parse(row.billing.next_billing_at) - Date.now()) / 86_400_000;
  return left <= 7;
}

function needsAttention(row: NodeRow): boolean {
  return row.state === 'down' || row.state === 'connecting' || row.traffic_alert || billingSoon(row);
}

function placeOf(row: NodeRow): string {
  return [row.city, countryName(row.node.countryCode)].filter(Boolean).join(', ');
}

function stateText(row: NodeRow, restarting: boolean): { text: string; tone: 'ok' | 'bad' | 'muted'; reason: string } {
  if (row.state === 'disabled') {
    return { text: 'выключена', tone: 'muted', reason: '' };
  }
  if (restarting || row.state === 'connecting') {
    return { text: 'подключается', tone: 'muted', reason: '' };
  }
  if (row.state === 'down') {
    const minutes = minutesSince(row.node.lastStatusChange);
    return { text: minutes === null ? 'не отвечает' : `не отвечает ${spanText(minutes)}`, tone: 'bad', reason: row.node.lastStatusMessage ?? '' };
  }
  return { text: `в сети · ${uptimeDays(row.node.xrayUptime)}`, tone: 'ok', reason: '' };
}

function installCommand(port: number, secret: string): string {
  return [
    'mkdir -p /opt/remnanode && cd /opt/remnanode',
    "cat > docker-compose.yml <<'EOF'",
    'services:',
    '  remnanode:',
    '    container_name: remnanode',
    '    hostname: remnanode',
    '    image: remnawave/node:latest',
    '    network_mode: host',
    '    restart: always',
    '    environment:',
    `      - NODE_PORT=${port}`,
    `      - SECRET_KEY=${secret}`,
    'EOF',
    'docker compose up -d',
  ].join('\n');
}

export function InfraNodes() {
  const phone = usePhone();
  if (phone) {
    return <NodesPhone />;
  }
  return <InfraNodesDesktop />;
}

function InfraNodesDesktop() {
  const [data, setData] = useState<NodesData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<'all' | 'attention'>('all');
  const [open, setOpen] = useState<string | null>(null);
  const [ask, setAsk] = useState<'' | 'reset' | 'delete'>('');
  const [busy, setBusy] = useState<string | null>(null);
  const [restarting, setRestarting] = useState<Set<string>>(new Set());
  const [adding, setAdding] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const autoOpened = useRef(false);

  const load = useCallback(async () => {
    try {
      const next = await api<NodesData>('/api/admin/infra/nodes');
      setData(next);
      setError(null);
      setRestarting((prev) => new Set([...prev].filter((uuid) => next.nodes.find((n) => n.node.uuid === uuid)?.state === 'down')));
      if (!autoOpened.current) {
        autoOpened.current = true;
        setOpen((current) => current ?? next.nodes.find((n) => n.state === 'down')?.node.uuid ?? null);
      }
    } catch (err) {
      setError(failText(err, 'Ноды не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 30_000);
    return () => window.clearInterval(timer);
  }, [load]);

  const nodes = data?.nodes ?? [];
  const attention = nodes.filter(needsAttention);
  const shown = filter === 'all' ? nodes : attention;
  const disabled = nodes.filter((n) => n.state === 'disabled').length;
  const enabled = nodes.length - disabled;
  const online = nodes.filter((n) => n.state === 'online').length;
  const usersOnline = nodes.filter((n) => n.state === 'online').reduce((sum, n) => sum + n.node.usersOnline, 0);
  const trafficTotal = nodes.reduce((sum, n) => sum + (n.node.trafficUsedBytes ?? 0), 0);
  const periods = [...new Set(nodes.filter((n) => (n.node.trafficUsedBytes ?? 0) > 0).map((n) => trafficSince(n.node.trafficResetDay)))];
  const period = periods.length === 1 && periods[0] ? `${periods[0]} ` : '';
  const alarm = nodes.find((n) => n.state === 'down') ?? nodes.find((n) => n.state === 'connecting' || restarting.has(n.node.uuid)) ?? null;

  const act = async (row: NodeRow, action: string, done: string) => {
    setBusy(row.node.uuid);
    setNote(null);
    try {
      await api(`/api/admin/infra/nodes/${row.node.uuid}/actions/${action}`, { method: 'POST', body: {} });
      if (action === 'restart') {
        setRestarting((prev) => new Set(prev).add(row.node.uuid));
      }
      if (action === 'delete') {
        setOpen(null);
      }
      setAsk('');
      setNote(done);
      await load();
    } catch (err) {
      setNote(failText(err, 'Действие не выполнилось'));
    } finally {
      setBusy(null);
    }
  };

  const summary = useMemo(() => {
    if (!data) {
      return '';
    }
    if (nodes.length === 0) {
      return 'Нод пока нет';
    }
    if (enabled === 0) {
      return `Все ноды выключены · трафик ${period}${tb(trafficTotal)}`;
    }
    const parts = [`${online} из ${enabled} ${plural(enabled, 'ноды', 'нод', 'нод')} в сети`];
    if (disabled) {
      parts.push(`${disabled}${NBSP}${plural(disabled, 'выключена', 'выключены', 'выключены')}`);
    }
    parts.push(`онлайн ${groupDigits(usersOnline)}`);
    parts.push(`трафик ${period}${tb(trafficTotal)}`);
    return parts.join(' · ');
  }, [data, nodes.length, enabled, online, disabled, usersOnline, trafficTotal, period]);

  return (
    <div className="inf">
      <InfraHead
        current="nodes"
        nodes={nodes.length}
        action={
          <button type="button" className={adding ? 'btn-line hov' : 'btn-main btn'} aria-expanded={adding} aria-controls="add-node" onClick={() => setAdding(!adding)}>
            Добавить ноду
          </button>
        }
      />

      {adding ? <AddNode onClose={() => setAdding(false)} onCreated={() => void load()} /> : null}

      {error && !data ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}

      <div className="inf-col" style={{ gap: 10 }}>
        <p className="inf-summary" aria-live="polite">
          {summary || 'Загружаем ноды…'}
        </p>
        {alarm ? (
          <div className="inf-alarm">
            <span style={{ color: alarm.state === 'down' && !restarting.has(alarm.node.uuid) ? 'var(--bad)' : 'var(--muted)', fontWeight: 600 }}>
              {alarm.state === 'down' && !restarting.has(alarm.node.uuid)
                ? `${alarm.node.name} не отвечает${alarm.node.lastStatusChange ? ` с ${timeHM(alarm.node.lastStatusChange)}` : ''}`
                : `${alarm.node.name} подключается`}
            </span>
            <button
              type="button"
              className="b36 hov"
              aria-label={`Перезапустить ноду ${alarm.node.name}`}
              onClick={() => void act(alarm, 'restart', `Перезапуск ${alarm.node.name} отправлен`)}
              disabled={busy !== null || restarting.has(alarm.node.uuid)}
            >
              {restarting.has(alarm.node.uuid) ? 'Перезапускаем…' : 'Перезапустить'}
            </button>
          </div>
        ) : null}
        {note ? (
          <p role="status" className="cap" style={{ color: 'var(--ink-2)' }}>
            {note}
          </p>
        ) : null}
      </div>

      <section className="inf-card" aria-label="Список нод">
        <div role="group" aria-label="Фильтр нод" className="hm-chips inf-filters">
          <button type="button" aria-pressed={filter === 'all'} onClick={() => setFilter('all')}>
            Все <span className="inf-n">{nodes.length}</span>
          </button>
          <button type="button" aria-pressed={filter === 'attention'} onClick={() => setFilter('attention')}>
            Требуют внимания <span className="inf-n">{attention.length}</span>
          </button>
        </div>
        <div className="inf-scroll">
          <div role="table" aria-label="Ноды" className="inf-table">
            <div role="row" className="inf-row head">
              <span role="columnheader">Нода</span>
              <span role="columnheader">Состояние</span>
              <span role="columnheader" className="right" style={{ paddingRight: 28 }}>
                Онлайн
              </span>
              <span role="columnheader">Трафик</span>
              <span role="columnheader">Нагрузка</span>
              <span role="columnheader">Сервер</span>
            </div>
            {data && shown.length === 0 ? (
              <div className="inf-empty">{nodes.length === 0 ? 'Добавьте первую ноду, чтобы клиентам было к чему подключаться' : 'Всё в порядке: ноды в сети, трафик и оплата в норме'}</div>
            ) : null}
            {shown.map((row) => {
              const isOpen = open === row.node.uuid;
              const st = stateText(row, restarting.has(row.node.uuid));
              const percent = trafficPercent(row);
              const limit = row.node.trafficLimitBytes ?? 0;
              const used = row.node.trafficUsedBytes ?? 0;
              const cpu = cpuOf(row);
              const mem = memoryOf(row);
              const soon = billingSoon(row);
              return (
                <Fragment key={row.node.uuid}>
                  <div role="row" className={isOpen ? 'inf-row open' : 'inf-row'}>
                    <span role="cell" className="inf-col">
                      <button
                        type="button"
                        className="inf-rowbtn"
                        aria-expanded={isOpen}
                        aria-controls={`node-${row.node.uuid}`}
                        onClick={() => {
                          setOpen(isOpen ? null : row.node.uuid);
                          setAsk('');
                        }}
                      >
                        {row.node.name}
                      </button>
                      <span className="inf-sub">{placeOf(row)}</span>
                    </span>
                    <span role="cell" className="inf-col">
                      <span style={{ color: st.tone === 'bad' ? 'var(--bad)' : st.tone === 'muted' ? 'var(--muted)' : 'var(--ink-2)', fontWeight: st.tone === 'bad' ? 600 : 500 }}>{st.text}</span>
                      {st.reason ? <span className="inf-sub">{st.reason}</span> : null}
                    </span>
                    <span role="cell" className="num right" style={{ paddingRight: 28, fontWeight: 600, color: row.state === 'online' ? 'var(--ink)' : 'var(--muted)' }}>
                      {row.state === 'online' ? groupDigits(row.node.usersOnline) : '0'}
                    </span>
                    <span role="cell" style={{ color: 'var(--ink-2)' }}>
                      {limit > 0 ? (
                        <>
                          {limitPair(used, limit)} ·{' '}
                          <span style={{ color: row.traffic_alert ? 'var(--warn)' : undefined }}>{percent}%</span>
                        </>
                      ) : (
                        tb(used)
                      )}
                    </span>
                    <span role="cell" style={{ color: row.state === 'online' ? 'var(--ink-2)' : 'var(--muted)' }}>
                      {row.state === 'online' && (cpu !== null || mem) ? [cpu !== null ? `CPU ${cpu}%` : null, mem ? `${num1(mem.used / GB)} из ${Math.round(mem.total / GB)}${NBSP}ГБ` : null].filter(Boolean).join(' · ') : '—'}
                    </span>
                    <span role="cell" className="inf-server" style={{ color: soon ? 'var(--warn)' : 'var(--ink-2)' }}>
                      <span>{row.billing ? `${row.billing.provider} · ${soon ? 'до ' : ''}${dateLong(row.billing.next_billing_at)}` : (row.node.provider?.name ?? '—')}</span>
                      <span className="inf-chev" aria-hidden="true" style={{ transform: isOpen ? 'rotate(180deg)' : undefined }}>
                        <Icon name="chevron-down" size={16} stroke={2} />
                      </span>
                    </span>
                  </div>
                  {isOpen ? (
                    <NodeDetails
                      row={row}
                      restarting={restarting.has(row.node.uuid)}
                      busy={busy === row.node.uuid}
                      ask={ask}
                      setAsk={setAsk}
                      act={(action, done) => void act(row, action, done)}
                      onEdited={(name) => {
                        setNote(`${name}: изменения сохранены`);
                        void load();
                      }}
                    />
                  ) : null}
                </Fragment>
              );
            })}
          </div>
        </div>
      </section>
    </div>
  );
}

type DetailsProps = {
  row: NodeRow;
  restarting: boolean;
  busy: boolean;
  ask: '' | 'reset' | 'delete';
  setAsk: (ask: '' | 'reset' | 'delete') => void;
  act: (action: string, done: string) => void;
  onEdited: (name: string) => void;
};

function NodeDetails({ row, restarting, busy, ask, setAsk, act, onEdited }: DetailsProps) {
  const [editing, setEditing] = useState(false);
  const st = stateText(row, restarting);
  const since = trafficSince(row.node.trafficResetDay);
  const limit = row.node.trafficLimitBytes ?? 0;
  const used = row.node.trafficUsedBytes ?? 0;
  const info = row.system?.info;
  const mem = memoryOf(row);
  const soon = billingSoon(row);
  const daysLeft = row.billing ? Math.max(0, Math.ceil((Date.parse(row.billing.next_billing_at) - Date.now()) / 86_400_000)) : null;
  const stateLine =
    row.state === 'disabled'
      ? ' · подключений не принимает'
      : restarting
        ? ' · перезапуск отправлен'
        : row.state === 'down'
          ? [row.node.lastStatusChange ? ` · С ${timeHM(row.node.lastStatusChange)}` : '', row.node.lastStatusMessage ? ` · ${row.node.lastStatusMessage}` : ''].join('')
          : '';
  const facts: Array<[string, string, boolean?]> = [
    ['Адрес', `${row.node.address}${row.node.port ? `:${row.node.port}` : ''}`, true],
    ['Сквады', row.squads.length ? row.squads.join(', ') : 'ни в одном скваде'],
    ['Входы', row.inbounds.length ? row.inbounds.map(inboundLabel).join(', ') : '—'],
    ['Система', info ? [info.cpus ? count(info.cpus, 'ядро', 'ядра', 'ядер') : null, mem ? `${Math.round(mem.total / GB)}${NBSP}ГБ` : null, info.platform ? `${info.platform} ${info.release ?? ''}`.trim() : null].filter(Boolean).join(' · ') : '—'],
  ];
  const right: Array<[string, string]> = [
    ['Версии', [row.node.versions?.xray ? `Xray ${row.node.versions.xray}` : null, row.node.versions?.node ? `нода ${row.node.versions.node}` : null].filter(Boolean).join(' · ') || '—'],
    [
      'Трафик',
      limit > 0
        ? `${limitPair(used, limit)}${since ? ` ${since}` : ''}${row.node.trafficResetDay ? `, сброс ${resetDate(row.node.trafficResetDay)}` : ''}${row.node.notifyPercent ? `, уведомление при ${row.node.notifyPercent}%` : ''}`
        : `${tb(used)}${since ? ` ${since}` : ''}, без лимита`,
    ],
    ['Коэффициент трафика', String(row.node.consumptionMultiplier || 1).replace('.', ',')],
    [
      'Провайдер',
      row.billing
        ? `${row.billing.provider} · ${soon ? `оплатить до ${dateLong(row.billing.next_billing_at)}, ${daysLeft === 0 ? 'сегодня' : `через ${count(daysLeft ?? 0, 'день', 'дня', 'дней')}`}` : `следующая оплата ${dateLong(row.billing.next_billing_at)}`}${row.billing.price ? ` · ${money(row.billing.price)}` : ''}`
        : (row.node.provider?.name ?? '—'),
    ],
  ];
  return (
    <div role="row" className="inf-details" id={`node-${row.node.uuid}`}>
      <div className="inf-dhead">
        <div className="inf-col" style={{ flex: '1 1 260px' }}>
          <h2 className="inf-h2">
            {row.node.name}
            {row.city ? ` · ${row.city}` : ''}
          </h2>
          <span>
            <span style={{ color: st.tone === 'bad' ? 'var(--bad)' : st.tone === 'muted' ? 'var(--muted)' : 'var(--ink-2)', fontWeight: 600 }}>
              {row.state === 'online' ? `в сети, работает ${uptimeDays(row.node.xrayUptime)}` : st.text}
            </span>
            <span className="muted">{stateLine}</span>
          </span>
        </div>
        <div role="group" aria-label={`Действия с ${row.node.name}`} className="cc-btns" style={{ gap: 8 }}>
          {row.state !== 'disabled' ? (
            <button type="button" className={row.state === 'down' && !restarting ? 'inf-b main btn' : 'inf-b hov'} onClick={() => act('restart', `Перезапуск ${row.node.name} отправлен`)} disabled={busy || restarting}>
              {restarting ? 'Перезапускаем…' : 'Перезапустить'}
            </button>
          ) : null}
          {row.state === 'disabled' ? (
            <button type="button" className="inf-b main btn" onClick={() => act('enable', `${row.node.name} включена`)} disabled={busy}>
              Включить
            </button>
          ) : (
            <button type="button" className="inf-b hov" onClick={() => act('disable', `${row.node.name} выключена`)} disabled={busy}>
              Выключить
            </button>
          )}
          {limit > 0 ? (
            <button type="button" className="inf-b hov" aria-expanded={ask === 'reset'} onClick={() => setAsk(ask === 'reset' ? '' : 'reset')} disabled={busy}>
              Сбросить трафик ноды
            </button>
          ) : null}
          <button type="button" className="inf-b hov" aria-expanded={editing} onClick={() => setEditing(!editing)} disabled={busy}>
            Изменить
          </button>
        </div>
      </div>
      {editing ? (
        <EditNode
          row={row}
          onClose={() => setEditing(false)}
          onSaved={(name) => {
            setEditing(false);
            onEdited(name);
          }}
        />
      ) : null}
      {ask === 'reset' ? (
        <div role="status" className="cc-note">
          <span className="cc-note-text wide">
            <strong>Обнулить трафик {row.node.name}?</strong> Сейчас {num1(used / TB)} из {num1(limit / TB)}
            {NBSP}ТБ, станет 0. Отменить будет нельзя.
          </span>
          <span className="cc-btns">
            <button type="button" className="b34 on-surface" onClick={() => act('reset-traffic', `Трафик ${row.node.name} обнулён`)} disabled={busy}>
              Сбросить трафик
            </button>
            <button type="button" className="b34 on-surface" onClick={() => setAsk('')} disabled={busy}>
              Отменить
            </button>
          </span>
        </div>
      ) : null}
      <div className="inf-facts">
        <div className="inf-col">
          {facts.map(([k, v, mono]) => (
            <div key={k} className="inf-fact">
              <span className="muted">{k}</span>
              <span className={mono ? 'mono' : undefined}>{v}</span>
            </div>
          ))}
        </div>
        <div className="inf-col">
          {right.map(([k, v]) => (
            <div key={k} className="inf-fact">
              <span className="muted">{k}</span>
              <span style={k === 'Провайдер' && soon ? { color: 'var(--warn)' } : k === 'Трафик' && row.traffic_alert ? { color: 'var(--warn)' } : undefined}>{v}</span>
            </div>
          ))}
        </div>
      </div>
      <details className="inf-danger" open={ask === 'delete' ? true : undefined}>
        <summary>Опасные действия</summary>
        <div className="inf-col" style={{ gap: 10, paddingTop: 12 }}>
          {ask === 'delete' ? (
            <div role="status" className="cc-note">
              <span className="cc-note-text wide">
                <strong>Удалить {row.node.name}?</strong>{' '}
                {row.squads.length ? `Её входы пропадут из ${row.squads.length === 1 ? `сквада ${row.squads[0]}` : `сквадов ${row.squads.join(', ')}`}.` : ''} Отменить будет нельзя.
              </span>
              <span className="cc-btns">
                <button type="button" className="b34 on-surface" style={{ color: 'var(--bad)', borderColor: 'var(--bad)' }} onClick={() => act('delete', `${row.node.name} удалена`)} disabled={busy}>
                  Удалить ноду
                </button>
                <button type="button" className="b34 on-surface" onClick={() => setAsk('')} disabled={busy}>
                  Отменить
                </button>
              </span>
            </div>
          ) : (
            <button type="button" className="b36 bad hov" style={{ width: 'fit-content' }} onClick={() => setAsk('delete')} disabled={busy}>
              Удалить ноду
            </button>
          )}
        </div>
      </details>
    </div>
  );
}

function EditNode({ row, onClose, onSaved }: { row: NodeRow; onClose: () => void; onSaved: (name: string) => void }) {
  const [name, setName] = useState(row.node.name);
  const [address, setAddress] = useState(row.node.address);
  const [country, setCountry] = useState(row.node.countryCode && row.node.countryCode !== 'XX' ? row.node.countryCode : '');
  const [city, setCity] = useState(row.city ?? '');
  const [tried, setTried] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameBad = tried && (name.trim().length < 3 || name.trim().length > 30);
  const addressBad = tried && (address.trim().length < 2 || /\s/.test(address.trim()));
  const countries = country && !COUNTRIES.includes(country) ? [country, ...COUNTRIES] : COUNTRIES;

  const save = async () => {
    setTried(true);
    const n = name.trim();
    const a = address.trim();
    if (n.length < 3 || n.length > 30 || a.length < 2 || /\s/.test(a)) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api(`/api/admin/infra/nodes/${row.node.uuid}`, { method: 'PUT', body: { name: n, address: a, country_code: country || null, city: city.trim() || null } });
      onSaved(n);
    } catch (err) {
      setError(failText(err, 'Нода не сохранилась'));
      setBusy(false);
    }
  };

  return (
    <div className="inf-edit" aria-label={`Изменить ${row.node.name}`} role="group">
      <div className="inf-grid">
        <label className="tf-field">
          <span>Название</span>
          <input className={nameBad ? 'field bad' : 'field'} value={name} maxLength={30} onChange={(e) => setName(e.target.value)} disabled={busy} />
          {nameBad ? <span className="err">От 3 до 30 символов</span> : null}
        </label>
        <label className="tf-field">
          <span>Адрес</span>
          <input className={addressBad ? 'field mono bad' : 'field mono'} value={address} maxLength={253} onChange={(e) => setAddress(e.target.value)} disabled={busy} />
          {addressBad ? <span className="err">IP или домен без пробелов</span> : <span className="cap">По этому адресу Remnawave подключается к ноде</span>}
        </label>
        <label className="tf-field">
          <span>Страна</span>
          <select className="field" value={country} onChange={(e) => setCountry(e.target.value)} disabled={busy}>
            <option value="">Не указана</option>
            {countries.map((code) => (
              <option key={code} value={code}>
                {countryName(code) || code}
              </option>
            ))}
          </select>
        </label>
        <label className="tf-field">
          <span>Город</span>
          <input className="field" value={city} maxLength={60} onChange={(e) => setCity(e.target.value)} placeholder="Например, Амстердам" disabled={busy} />
        </label>
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="cc-btns" style={{ gap: 10 }}>
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy} style={{ padding: '0 16px' }}>
          Отменить
        </button>
      </div>
    </div>
  );
}

function AddNode({ onClose, onCreated }: { onClose: () => void; onCreated: () => void }) {
  const [profiles, setProfiles] = useState<Profile[] | null>(null);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [name, setName] = useState('');
  const [address, setAddress] = useState('');
  const [country, setCountry] = useState('NL');
  const [city, setCity] = useState('');
  const [profile, setProfile] = useState('');
  const [provider, setProvider] = useState('');
  const [billing, setBilling] = useState('');
  const [inbounds, setInbounds] = useState<string[]>([]);
  const [tried, setTried] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<{ name: string; address: string; command: string } | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    api<Profile[]>('/api/admin/infra/profiles')
      .then((list) => {
        setProfiles(list);
        const first = list[0];
        if (first) {
          setProfile(first.uuid);
          setInbounds(first.inbounds.map((i) => i.uuid));
        }
      })
      .catch((err) => {
        setProfiles([]);
        setError(failText(err, 'Профили конфигурации не загрузились'));
      });
    api<Provider[]>('/api/admin/infra/providers')
      .then(setProviders)
      .catch(() => setProviders([]));
  }, []);

  const current = profiles?.find((p) => p.uuid === profile) ?? null;
  const pickProfile = (uuid: string) => {
    setProfile(uuid);
    setInbounds(profiles?.find((p) => p.uuid === uuid)?.inbounds.map((i) => i.uuid) ?? []);
  };
  const parsedBilling = (() => {
    const m = /^(\d{2})\.(\d{2})\.(\d{4})$/.exec(billing.trim());
    if (!m) {
      return null;
    }
    const date = new Date(Number(m[3]), Number(m[2]) - 1, Number(m[1]), 12);
    return Number.isNaN(date.getTime()) ? null : date.toISOString();
  })();
  const nameBad = tried && (name.trim().length < 3 || name.trim().length > 30);
  const addressBad = tried && (address.trim().length < 2 || /\s/.test(address.trim()));
  const billingBad = tried && billing.trim() !== '' && parsedBilling === null;

  const create = async () => {
    setTried(true);
    if (name.trim().length < 3 || name.trim().length > 30 || address.trim().length < 2 || /\s/.test(address.trim()) || !profile || inbounds.length === 0 || (billing.trim() && !parsedBilling)) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const created = await api<{ node: { name: string; address: string }; port: number; secret_key: string }>('/api/admin/infra/nodes', {
        method: 'POST',
        body: {
          name: name.trim(),
          address: address.trim(),
          country_code: country,
          city: city.trim() || null,
          profile_uuid: profile,
          inbounds,
          provider_uuid: provider || null,
          next_billing_at: provider && parsedBilling ? parsedBilling : null,
        },
      });
      setDone({ name: created.node.name, address: created.node.address, command: installCommand(created.port, created.secret_key) });
      onCreated();
    } catch (err) {
      setError(failText(err, 'Нода не создалась'));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    if (!done) {
      return;
    }
    try {
      await navigator.clipboard.writeText(done.command);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setError('Браузер не дал скопировать: выделите команду вручную');
    }
  };

  if (done) {
    return (
      <section id="add-node" className="inf-card inf-add" aria-label="Установка ноды">
        <h2 className="inf-h2">Установите ноду на сервер</h2>
        <p style={{ color: 'var(--ink-2)' }}>
          {done.name} создана в Remnawave. Выполните команду на сервере <span className="mono">{done.address}</span>:
        </p>
        <pre className="inf-cmd mono">{done.command}</pre>
        <div className="cc-btns" style={{ gap: 10, alignItems: 'center' }}>
          <button type="button" className="btn-main btn" onClick={() => void copy()}>
            {copied ? 'Скопировано' : 'Скопировать'}
          </button>
          <span role="status" className="muted" style={{ flex: 1 }}>
            Ждём подключения… нода появится в списке сама
          </span>
          <button type="button" className="btn-line hov" onClick={onClose}>
            Готово
          </button>
        </div>
      </section>
    );
  }

  return (
    <section id="add-node" className="inf-card inf-add" aria-label="Новая нода">
      <div className="inf-col" style={{ gap: 2 }}>
        <h2 className="inf-h2">Новая нода</h2>
        <span className="muted">После создания покажем команду для установки на сервер</span>
      </div>
      <div className="inf-grid">
        <label className="tf-field">
          <span>Название</span>
          <input className={nameBad ? 'field bad' : 'field'} value={name} maxLength={30} onChange={(e) => setName(e.target.value)} placeholder="Например, NL-3" disabled={busy} />
          {nameBad ? <span className="err">От 3 до 30 символов</span> : null}
        </label>
        <label className="tf-field">
          <span>Адрес</span>
          <input className={addressBad ? 'field mono bad' : 'field mono'} value={address} maxLength={253} onChange={(e) => setAddress(e.target.value)} placeholder="IP или домен сервера" disabled={busy} />
          {addressBad ? <span className="err">IP или домен без пробелов</span> : null}
        </label>
        <label className="tf-field">
          <span>Страна</span>
          <select className="field" value={country} onChange={(e) => setCountry(e.target.value)} disabled={busy}>
            {COUNTRIES.map((code) => (
              <option key={code} value={code}>
                {countryName(code)}
              </option>
            ))}
          </select>
        </label>
        <label className="tf-field">
          <span>Город</span>
          <input className="field" value={city} maxLength={40} onChange={(e) => setCity(e.target.value)} placeholder="Например, Амстердам" disabled={busy} />
        </label>
        <label className="tf-field">
          <span>Профиль конфигурации</span>
          <select className="field" value={profile} onChange={(e) => pickProfile(e.target.value)} disabled={busy || !profiles?.length}>
            {(profiles ?? []).map((p) => (
              <option key={p.uuid} value={p.uuid}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <label className="tf-field">
          <span>Провайдер</span>
          <select className="field" value={provider} onChange={(e) => setProvider(e.target.value)} disabled={busy}>
            <option value="">Не указан</option>
            {providers.map((p) => (
              <option key={p.uuid} value={p.uuid}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        {provider ? (
          <label className="tf-field">
            <span>Дата оплаты</span>
            <input className={billingBad ? 'field bad' : 'field'} value={billing} maxLength={10} onChange={(e) => setBilling(e.target.value)} placeholder="ДД.ММ.ГГГГ" disabled={busy} />
            <span className={billingBad ? 'err' : 'cap'}>{billingBad ? 'Дата в виде ДД.ММ.ГГГГ' : 'Напомним за 7 дней до оплаты'}</span>
          </label>
        ) : null}
      </div>
      <fieldset className="tf-fieldset">
        <legend style={{ fontWeight: 600 }}>Входы</legend>
        <div className="tf-wrap" style={{ gap: '10px 28px' }}>
          {(current?.inbounds ?? []).map((i) => (
            <label key={i.uuid} className="tf-check">
              <input
                type="checkbox"
                checked={inbounds.includes(i.uuid)}
                onChange={() => setInbounds(inbounds.includes(i.uuid) ? inbounds.filter((x) => x !== i.uuid) : [...inbounds, i.uuid])}
                disabled={busy}
              />
              {inboundLabel(i)}
            </label>
          ))}
          {profiles && profiles.length === 0 ? <span className="muted">В Remnawave нет профилей конфигурации</span> : null}
        </div>
        {current && inbounds.length === 0 ? (
          <p className="cap" style={{ color: 'var(--warn)', marginTop: 8 }}>
            Выберите хотя бы один вход
          </p>
        ) : null}
      </fieldset>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="cc-btns" style={{ gap: 10 }}>
        <button type="button" className="btn-main btn" onClick={() => void create()} disabled={busy || !current || inbounds.length === 0}>
          {busy ? 'Создаём…' : 'Создать ноду'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy} style={{ padding: '0 16px' }}>
          Отменить
        </button>
      </div>
    </section>
  );
}
