import { Fragment, useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, dateLong, groupDigits, money, NBSP, plural } from '../../lib/format';
import { Flag } from '../../lib/Flag';
import { Icon } from '../../lib/icons';
import { minutesSince, spanText } from '../home/model';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import { useRemnaPanel } from './InfraHead';
import { limitPair, resetDate, tb, trafficPercent, type NodeRow, type NodesData } from './InfraNodes';
import '../home/phone.css';
import './infra.css';

type Attention = { key: string; title: string; note: string; tone: 'bad' | 'warn' | 'muted'; row: NodeRow; kind: 'down' | 'connecting' | 'traffic' | 'billing'; days?: number };

const SOON = 7;

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function daysLeft(iso: string): number {
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  return Math.round((start(new Date(iso)) - start(new Date())) / 86_400_000);
}

function nextMonth(iso: string): string {
  const date = new Date(iso);
  date.setMonth(date.getMonth() + 1);
  return date.toISOString();
}

export function NodesPhone() {
  const { can } = useSession();
  const panel = useRemnaPanel();
  const [data, setData] = useState<NodesData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [restarting, setRestarting] = useState<Set<string>>(new Set());
  const [asking, setAsking] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<{ head: string; tail: string } | null>(null);
  const canAct = can('infrastructure');

  const load = useCallback(async () => {
    try {
      const next = await api<NodesData>('/api/admin/infra/nodes');
      setData(next);
      setRestarting((prev) => new Set([...prev].filter((uuid) => next.nodes.find((n) => n.node.uuid === uuid)?.state === 'down')));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Ноды не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 30_000);
    return () => window.clearInterval(timer);
  }, [load]);

  useEffect(() => {
    if (!toast) {
      return;
    }
    const timer = window.setTimeout(() => setToast(null), 6000);
    return () => window.clearTimeout(timer);
  }, [toast]);

  const restart = async (row: NodeRow) => {
    setRestarting((prev) => new Set(prev).add(row.node.uuid));
    try {
      await api(`/api/admin/infra/nodes/${row.node.uuid}/actions/restart`, { method: 'POST', body: {} });
      window.setTimeout(() => void load(), 4000);
    } catch (err) {
      setRestarting((prev) => {
        const next = new Set(prev);
        next.delete(row.node.uuid);
        return next;
      });
      setError(failText(err, 'Перезапуск не отправлен'));
    }
  };

  const markPaid = async (row: NodeRow) => {
    if (!row.billing) {
      return;
    }
    setBusy(true);
    try {
      await api(`/api/admin/infra/billing/${row.billing.uuid}/paid`, { method: 'POST', body: {} });
      setAsking(null);
      setToast({ head: `Оплата ${row.node.name} отмечена.`, tail: ` Следующая${NBSP}— ${dateLong(nextMonth(row.billing.next_billing_at))}.` });
      await load();
    } catch (err) {
      setError(failText(err, 'Оплата не отметилась'));
    } finally {
      setBusy(false);
    }
  };

  const nodes = data?.nodes ?? [];
  const enabled = nodes.filter((n) => n.state !== 'disabled');
  const off = nodes.length - enabled.length;
  const online = nodes.filter((n) => n.state === 'online').length;
  const users = nodes.filter((n) => n.state === 'online').reduce((sum, n) => sum + n.node.usersOnline, 0);

  const attention: Attention[] = [];
  for (const row of nodes) {
    const busyRestart = restarting.has(row.node.uuid);
    if (row.state === 'down' || row.state === 'connecting' || busyRestart) {
      const minutes = minutesSince(row.down_since ?? row.node.lastStatusChange);
      const down = row.state === 'down' && !busyRestart;
      attention.push({
        key: `s-${row.node.uuid}`,
        kind: down ? 'down' : 'connecting',
        row,
        title: down ? `${row.node.name} не отвечает` : `${row.node.name} подключается`,
        note: down
          ? [minutes === null ? null : spanText(minutes), row.node.lastStatusMessage].filter(Boolean).join(' · ')
          : minutes === null
            ? ''
            : `Не отвечала ${spanText(minutes)}`,
        tone: down ? 'bad' : 'muted',
      });
    }
  }
  for (const row of nodes) {
    const limit = row.node.trafficLimitBytes ?? 0;
    if (row.traffic_alert && limit > 0) {
      attention.push({
        key: `t-${row.node.uuid}`,
        kind: 'traffic',
        row,
        title: `${row.node.name}: трафик ${trafficPercent(row)}%`,
        note: `${limitPair(row.node.trafficUsedBytes ?? 0, limit)}${row.node.trafficResetDay ? `, сброс ${resetDate(row.node.trafficResetDay)}` : ''}`,
        tone: 'warn',
      });
    }
  }
  for (const row of nodes) {
    if (!row.billing) {
      continue;
    }
    const left = daysLeft(row.billing.next_billing_at);
    if (left > SOON) {
      continue;
    }
    attention.push({
      key: `b-${row.node.uuid}`,
      kind: 'billing',
      row,
      days: left,
      title: `${row.node.name}: оплатить хостинг`,
      note:
        left < 0
          ? `${row.billing.provider} · просрочено на ${count(-left, 'день', 'дня', 'дней')}`
          : `${row.billing.provider} · до ${dateLong(row.billing.next_billing_at)}`,
      tone: left < 0 ? 'bad' : 'warn',
    });
  }

  return (
    <div className="hp np">
      <PhoneHeader title="Ноды" back={{ to: '/admin', label: 'Назад' }}>
        {panel ? (
          <a href={panel} target="_blank" rel="noreferrer" className="np-ext">
            Открыть в Remnawave
          </a>
        ) : null}
      </PhoneHeader>

      <p className="np-sum">
        {data
          ? nodes.length === 0
            ? 'Нод пока нет'
            : [
                `${online} из ${enabled.length} в сети`,
                off ? `${off}${NBSP}${plural(off, 'выключена', 'выключены', 'выключены')}` : null,
                `онлайн ${groupDigits(users)}`,
              ]
                .filter(Boolean)
                .join(' · ')
          : error
            ? error
            : 'Загружаем ноды…'}
      </p>
      {error && data ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}

      {attention.length ? (
        <section aria-labelledby="np-att" className="hp-group">
          <h2 id="np-att" className="hp-h2">
            Требуют внимания
          </h2>
          <ul className="hp-card hp-list np-ul">
            {attention.map((a) => (
              <Fragment key={a.key}>
                <li className="hp-todo">
                  <span className="hp-text">
                    <span className="hp-title">{a.title}</span>
                    {a.note ? <span className={`hp-note ${a.tone}`}>{a.note}</span> : null}
                  </span>
                  {a.kind === 'down' && canAct ? (
                    <button type="button" className="hp-pill press" aria-label={`Перезапустить ноду ${a.row.node.name}`} onClick={() => void restart(a.row)}>
                      <span>Перезапустить</span>
                    </button>
                  ) : null}
                  {a.kind === 'connecting' && restarting.has(a.row.node.uuid) ? (
                    <button type="button" className="hp-pill" aria-label={`Перезапускаем ноду ${a.row.node.name}`} aria-disabled="true">
                      <span>Перезапускаем…</span>
                    </button>
                  ) : null}
                  {a.kind === 'billing' && canAct ? (
                    <button
                      type="button"
                      className="hp-pill press"
                      aria-label={`Отметить оплату ${a.row.node.name}`}
                      aria-expanded={asking === a.row.node.uuid}
                      onClick={() => setAsking(asking === a.row.node.uuid ? null : a.row.node.uuid)}
                    >
                      <span>Отметить</span>
                    </button>
                  ) : null}
                </li>
                {a.kind === 'billing' && asking === a.row.node.uuid && a.row.billing ? (
                  <li className="np-ask">
                    <p>
                      <strong>Отметить оплату {a.row.node.name}:</strong> {a.row.billing.price ? money(a.row.billing.price) : 'сумма не задана'} сегодня, {dateLong(new Date().toISOString())}. Следующая{NBSP}— {dateLong(nextMonth(a.row.billing.next_billing_at))}.
                    </p>
                    {a.row.billing.price ? null : <p className="hp-note warn">Сначала укажите цену сервера в разделе «Оплата серверов»</p>}
                    <span className="np-ask-btns">
                      <button type="button" className="np-ok press" onClick={() => void markPaid(a.row)} disabled={busy || !a.row.billing.price}>
                        Отметить
                      </button>
                      <button type="button" className="np-cancel press" onClick={() => setAsking(null)} disabled={busy}>
                        Отменить
                      </button>
                    </span>
                  </li>
                ) : null}
              </Fragment>
            ))}
          </ul>
        </section>
      ) : null}

      {nodes.length ? (
        <section aria-labelledby="np-all" className="hp-group">
          <h2 id="np-all" className="hp-h2">
            Все ноды
          </h2>
          <ul className="hp-card hp-list np-ul">
            {nodes.map((row) => {
              const limit = row.node.trafficLimitBytes ?? 0;
              const used = row.node.trafficUsedBytes ?? 0;
              const minutes = minutesSince(row.down_since ?? row.node.lastStatusChange);
              const busyRestart = restarting.has(row.node.uuid);
              const state =
                row.state === 'disabled'
                  ? { text: 'выключена', cls: 'muted' }
                  : busyRestart || row.state === 'connecting'
                    ? { text: 'подключается', cls: 'muted' }
                    : row.state === 'down'
                      ? { text: minutes === null ? 'не отвечает' : `не отвечает ${spanText(minutes)}`, cls: 'bad strong' }
                      : { text: `${groupDigits(row.node.usersOnline)} онлайн`, cls: 'ink2' };
              return (
                <li key={row.node.uuid} className="np-node">
                  <span className="np-node-body">
                    <span>
                      <Flag code={row.node.countryCode} />
                      <strong>{row.node.name}</strong>
                      {row.city ? <span className="np-city"> · {row.city}</span> : null}
                    </span>
                    <span className="hp-note muted">
                      <span className={`st-dot ${row.state === 'disabled' ? 'muted' : busyRestart || row.state === 'connecting' ? 'warn' : row.state === 'down' ? 'bad' : 'ok'}`} aria-hidden="true" />
                      <span className={`np-st ${state.cls}`}>{state.text}</span> · {limit > 0 ? limitPair(used, limit) : tb(used)}
                      {limit > 0 && row.traffic_alert ? <span style={{ color: 'var(--warn)' }}> · {trafficPercent(row)}%</span> : null}
                    </span>
                  </span>
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}

      {data ? (
        <section aria-labelledby="np-more" className="hp-group">
          <h2 id="np-more" className="hp-h2">
            Что видят клиенты
          </h2>
          <ul className="hp-card hp-list np-ul">
            <li>
              <Link to="/admin/infra/hosts" className="hs-plink press">
                <span className="hp-text">
                  <span className="hp-title">Хосты</span>
                  <span className="hp-note muted">Названия серверов в приложении, адреса и порядок</span>
                </span>
                <Icon name="chevron-right" size={18} />
              </Link>
            </li>
          </ul>
        </section>
      ) : null}

      <div role="status" className="np-toast-wrap">
        {toast ? (
          <p className="np-toast">
            <strong>{toast.head}</strong>
            {toast.tail}
          </p>
        ) : null}
      </div>
    </div>
  );
}
