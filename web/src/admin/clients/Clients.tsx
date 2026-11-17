import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import {
  avatarColor,
  dateNumeric,
  dayMonthNumeric,
  days,
  daysLeft,
  groupDigits,
  initials,
  money,
  NBSP,
  periodLabel,
} from '../../lib/format';
import { Icon } from '../../lib/icons';
import { useSession } from '../session';
import { displayName, STATUS_WORD, type ClientRow, type Segment, type SegmentCounts } from './types';
import './clients.css';

const SEGMENTS: Array<[Segment, string, keyof SegmentCounts]> = [
  ['all', 'Все', 'total'],
  ['active', 'Активные', 'active'],
  ['expiring_soon', 'Истекают за 3 дня', 'expiring_soon'],
  ['trial', 'Пробный период', 'trial'],
  ['never_connected', 'Не подключились', 'never_connected'],
  ['expired', 'Истекли', 'expired'],
  ['limited', 'Трафик исчерпан', 'limited'],
  ['blocked', 'Заблокированы', 'blocked'],
];

type Page = { items: ClientRow[]; next: number | null };

type Bulk = 'extend' | 'reset' | 'block' | 'unblock';

const BULK_TEXT: Record<Bulk, { title: string; confirm: string; text: string }> = {
  extend: { title: 'Продлить на 7 дней', confirm: 'Продлить', text: 'Каждому выбранному клиенту с подпиской добавится 7 дней.' },
  reset: { title: 'Сбросить трафик', confirm: 'Сбросить', text: 'Счётчик трафика обнулится у выбранных подписок с лимитом трафика.' },
  block: { title: 'Заблокировать', confirm: 'Заблокировать', text: 'Подписки выбранных клиентов отключатся, вход в кабинет закроется, неоплаченные счета отменятся.' },
  unblock: { title: 'Разблокировать', confirm: 'Разблокировать', text: 'Подписки выбранных клиентов снова включатся.' },
};

function rowView(row: ClientRow) {
  const name = displayName(row);
  const handle = row.telegram_username ? `@${row.telegram_username}` : row.email ?? 'без username';
  const sub = row.subscription;
  const [word, color] = STATUS_WORD[row.status];
  let plan = '';
  if (row.status === 'blocked') {
    plan = row.client.block_reason ?? '';
  } else if (sub?.kind === 'trial') {
    plan = sub.period_days ? `${days(sub.period_days)} бесплатно` : 'Пробный период';
  } else if (sub) {
    plan = [row.tariff_name, periodLabel(sub.period_days)].filter(Boolean).join(`${NBSP}· `);
  } else if (row.balance && row.balance.minor > 0) {
    plan = 'Пополнил баланс';
  }
  const expire = sub?.mirror.expire_at ?? null;
  const left = daysLeft(expire);
  let leftText = '—';
  let until = '';
  const dim = row.status === 'expired' || row.status === 'blocked' || row.status === 'no_subscription';
  if (row.status === 'expired') {
    until = expire ? `была до ${dateNumeric(expire)}` : '';
  } else if (!dim && left !== null) {
    leftText = days(Math.max(left, 0));
    until = sub?.autorenew.enabled && row.status === 'expiring' ? `автопродление ${dayMonthNumeric(expire)}` : `до ${dateNumeric(expire)}`;
  }
  const devices =
    sub && sub.mirror.hwid_limit !== null && row.status !== 'no_subscription'
      ? `${sub.mirror.devices_count ?? 0}${NBSP}из${NBSP}${sub.mirror.hwid_limit}`
      : '—';
  return { name, handle, word, color, plan, leftText, until, dim, devices };
}

export function Clients() {
  const { can } = useSession();
  const [params, setParams] = useSearchParams();
  const segment = (params.get('segment') as Segment | null) ?? 'all';
  const query = params.get('q') ?? '';
  const [input, setInput] = useState(query);
  const [counts, setCounts] = useState<SegmentCounts | null>(null);
  const [page, setPage] = useState<Page | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [bulk, setBulk] = useState<Bulk | null>(null);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkResult, setBulkResult] = useState<string | null>(null);
  const requestRef = useRef(0);
  const writable = can('clients_write');

  const url = useCallback(
    (after?: number | null) => {
      const search = new URLSearchParams({ limit: '50' });
      if (segment !== 'all') {
        search.set('segment', segment);
      }
      if (query.trim()) {
        search.set('q', query.trim());
      }
      if (after) {
        search.set('after', String(after));
      }
      return `/api/admin/clients?${search.toString()}`;
    },
    [segment, query],
  );

  const reload = useCallback(async () => {
    const id = ++requestRef.current;
    setError(null);
    try {
      const [list, segs] = await Promise.all([api<Page>(url()), api<SegmentCounts>('/api/admin/clients/segments')]);
      if (id === requestRef.current) {
        setPage(list);
        setCounts(segs);
      }
    } catch (err) {
      if (id === requestRef.current) {
        setError(err instanceof ApiError ? err.message : 'Список не загрузился');
      }
    }
  }, [url]);

  useEffect(() => {
    setSelected(new Set());
    void reload();
  }, [reload]);

  useEffect(() => {
    setInput(query);
  }, [query]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      if (input.trim() === query.trim()) {
        return;
      }
      const next = new URLSearchParams(params);
      if (input.trim()) {
        next.set('q', input.trim());
      } else {
        next.delete('q');
      }
      setParams(next, { replace: true });
    }, 350);
    return () => window.clearTimeout(timer);
  }, [input, query, params, setParams]);

  const pickSegment = (value: Segment) => {
    const next = new URLSearchParams(params);
    if (value === 'all') {
      next.delete('segment');
    } else {
      next.set('segment', value);
    }
    setParams(next);
  };

  const more = async () => {
    if (!page?.next || loadingMore) {
      return;
    }
    setLoadingMore(true);
    try {
      const list = await api<Page>(url(page.next));
      setPage({ items: [...page.items, ...list.items], next: list.next });
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Не удалось догрузить');
    } finally {
      setLoadingMore(false);
    }
  };

  const rows = page?.items ?? [];
  const allOn = rows.length > 0 && rows.every((row) => selected.has(row.client.id));
  const toggleAll = () => setSelected(allOn ? new Set() : new Set(rows.map((row) => row.client.id)));
  const toggle = (id: number) => {
    const next = new Set(selected);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    setSelected(next);
  };

  const chosen = useMemo(() => rows.filter((row) => selected.has(row.client.id)), [rows, selected]);
  const total = counts ? counts[SEGMENTS.find(([key]) => key === segment)?.[2] ?? 'total'] : null;

  const runBulk = async (action: Bulk) => {
    setBulkBusy(true);
    let done = 0;
    let failed = 0;
    for (const row of chosen) {
      try {
        if (action === 'extend' && row.subscription && !row.subscription.ended_at) {
          await api(`/api/admin/subscriptions/${row.subscription.id}/actions/extend`, { method: 'POST', body: { days: 7 } });
          done += 1;
        } else if (action === 'reset' && row.subscription?.mirror.traffic_limit_bytes) {
          await api(`/api/admin/subscriptions/${row.subscription.id}/actions/reset-traffic`, { method: 'POST' });
          done += 1;
        } else if (action === 'block' && row.client.status !== 'blocked') {
          await api(`/api/admin/clients/${row.client.id}/block`, { method: 'POST' });
          done += 1;
        } else if (action === 'unblock' && row.client.status === 'blocked') {
          await api(`/api/admin/clients/${row.client.id}/unblock`, { method: 'POST' });
          done += 1;
        }
      } catch {
        failed += 1;
      }
    }
    setBulkBusy(false);
    setBulk(null);
    setBulkResult(failed ? `Готово: ${groupDigits(done)}, не получилось: ${groupDigits(failed)}` : `Готово: ${groupDigits(done)}`);
    setSelected(new Set());
    void reload();
  };

  return (
    <>
      <header className="cl-head">
        <div className="cl-head-title">
          <h1 className="page-title">Клиенты</h1>
          {counts ? (
            <span className="muted">
              {groupDigits(counts.total)}
              {NBSP}
              {counts.total % 10 >= 2 && counts.total % 10 <= 4 && (counts.total % 100 < 12 || counts.total % 100 > 14) ? 'человека' : 'человек'}, у {groupDigits(counts.active)} активная подписка
            </span>
          ) : null}
        </div>
        <label className="cl-search">
          <Icon name="search" size={18} stroke={1.9} />
          <span className="sr-only">Поиск клиента</span>
          <input type="search" placeholder="Имя, @username, ID, email или ссылка" value={input} onChange={(e) => setInput(e.target.value)} maxLength={200} />
        </label>
        <button type="button" className="cl-new btn" aria-disabled="true" title="Выдача подписки без оплаты появится позже">
          <Icon name="plus" size={18} stroke={2.4} />
          Выдать подписку
        </button>
      </header>

      <div role="group" aria-label="Кого показать" className="chips">
        {SEGMENTS.map(([key, label, countKey]) => (
          <button key={key} type="button" className="chip" aria-pressed={segment === key} onClick={() => pickSegment(key)}>
            <span>{label}</span>
            <span className="chip-n">{counts ? groupDigits(counts[countKey]) : ''}</span>
          </button>
        ))}
      </div>

      <section aria-label="Список клиентов" className="cl-sec">
        {chosen.length > 0 && writable ? (
          <div role="group" aria-label="Действия с выбранными" className="cl-bulk">
            <b>Выбрано: {chosen.length}</b>
            <button type="button" className="b3 hov" onClick={() => setBulk('extend')}>
              Продлить
            </button>
            <button type="button" className="b3 hov" onClick={() => setBulk('reset')}>
              Сбросить трафик
            </button>
            <button type="button" className="b3 hov" aria-disabled="true" title="Рассылка выбранным появится вместе с модулем «Рассылки»">
              Написать
            </button>
            {segment === 'blocked' ? (
              <button type="button" className="b3 hov" onClick={() => setBulk('unblock')}>
                Разблокировать
              </button>
            ) : (
              <button type="button" className="b3 hov" style={{ color: 'var(--bad)' }} onClick={() => setBulk('block')}>
                Заблокировать
              </button>
            )}
            <span style={{ flex: 1 }} />
            <button type="button" className="quiet hov" onClick={() => setSelected(new Set())}>
              Снять выбор
            </button>
          </div>
        ) : null}
        {bulkResult ? (
          <div className="cl-bulk" role="status">
            <span>{bulkResult}</span>
            <span style={{ flex: 1 }} />
            <button type="button" className="quiet hov" onClick={() => setBulkResult(null)}>
              Скрыть
            </button>
          </div>
        ) : null}
        <div className="cl-scroll">
          <div role="table" aria-label="Клиенты" className="cl-table">
            <div role="row" className="cl-row head">
              <span role="columnheader" style={{ display: 'flex' }}>
                {writable ? <input type="checkbox" aria-label="Выбрать всех" checked={allOn} onChange={toggleAll} /> : null}
              </span>
              <span role="columnheader">Клиент</span>
              <span role="columnheader">Подписка</span>
              <span role="columnheader">Осталось</span>
              <span role="columnheader">Устройства</span>
              <span role="columnheader" className="right">
                Баланс
              </span>
              <span role="columnheader" className="right">
                Оплатил
              </span>
              <span role="columnheader">
                <span className="sr-only">Действия</span>
              </span>
            </div>
            {rows.map((row) => {
              const view = rowView(row);
              const on = selected.has(row.client.id);
              const balanceZero = !row.balance || row.balance.minor === 0;
              const paidZero = !row.paid_total || row.paid_total.minor === 0;
              return (
                <div role="row" key={row.client.id} className={on ? 'cl-row hov sel' : 'cl-row hov'}>
                  <span role="cell" style={{ display: 'flex' }}>
                    {writable ? <input type="checkbox" aria-label={`Выбрать: ${view.name}`} checked={on} onChange={() => toggle(row.client.id)} /> : null}
                  </span>
                  <div role="cell" className="cl-who">
                    <span className="av" aria-hidden="true" style={{ background: avatarColor(row.client.id) }}>
                      {initials(view.name)}
                    </span>
                    <span className="two">
                      <Link to={`/admin/clients/${row.client.id}`} className="main" title={view.name}>
                        {view.name}
                      </Link>
                      <span className="sub">{view.handle}</span>
                    </span>
                  </div>
                  <div role="cell" className="two">
                    <span style={{ fontWeight: 600, color: view.color }}>{view.word}</span>
                    <span className="sub" title={view.plan}>
                      {view.plan}
                    </span>
                  </div>
                  <div role="cell" className="two num">
                    <span style={{ fontWeight: 700, color: view.dim ? 'var(--muted)' : 'var(--ink)' }}>{view.leftText}</span>
                    <span className="sub">{view.until}</span>
                  </div>
                  <span role="cell" className="num">
                    {view.devices}
                  </span>
                  <span role="cell" className="num right" style={{ fontWeight: 600, color: balanceZero ? 'var(--muted)' : 'var(--ink)' }}>
                    {money(row.balance)}
                  </span>
                  <span role="cell" className="num right" style={{ color: paidZero ? 'var(--muted)' : 'var(--ink-2)' }}>
                    {money(row.paid_total)}
                  </span>
                  <span role="cell">
                    <Link to={`/admin/clients/${row.client.id}`} className="dots-btn hov" aria-label={`Действия: ${view.name}`}>
                      <Icon name="dots" size={18} stroke={2.6} />
                    </Link>
                  </span>
                </div>
              );
            })}
            {page && rows.length === 0 ? (
              <div className="cl-empty">{query ? 'Никого не нашли. Проверьте запрос или сбросьте фильтр.' : 'В этом списке пока никого нет.'}</div>
            ) : null}
            {error ? (
              <div className="cl-empty" role="alert" style={{ color: 'var(--bad)' }}>
                {error}
              </div>
            ) : null}
          </div>
        </div>
        <div className="cl-foot">
          <span>
            Показано {groupDigits(rows.length)}
            {total !== null && !query ? ` из ${groupDigits(total)}` : ''}
          </span>
          {page?.next ? (
            <button type="button" className="cl-more hov" onClick={() => void more()} disabled={loadingMore}>
              {loadingMore ? 'Загружаем…' : 'Показать ещё 50'}
            </button>
          ) : null}
        </div>
      </section>

      {bulk ? (
        <div className="dialog-back" role="presentation" onClick={() => (bulkBusy ? undefined : setBulk(null))}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="bulk-h" onClick={(e) => e.stopPropagation()}>
            <h2 id="bulk-h">
              {BULK_TEXT[bulk].title}: {chosen.length}
            </h2>
            <p className="muted">{BULK_TEXT[bulk].text}</p>
            <div className="dialog-actions">
              <button type="button" className={bulk === 'block' ? 'btn-bad' : 'btn-main btn'} onClick={() => void runBulk(bulk)} disabled={bulkBusy}>
                {bulkBusy ? 'Выполняем…' : BULK_TEXT[bulk].confirm}
              </button>
              <button type="button" className="btn-line hov" onClick={() => setBulk(null)} disabled={bulkBusy}>
                Отменить
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
