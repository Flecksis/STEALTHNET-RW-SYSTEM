import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { Avatar } from '../Avatar';
import { ConfirmDialog } from '../client/dialogs';
import { amountText, lowerFirst, methodText } from '../client/history';
import { agoLong, orderLine, personName, type PaymentItem } from '../home/model';
import './money.css';

type Filter = 'pending' | 'canceled' | 'paid' | 'all';

const FILTERS: Array<[Filter, string]> = [
  ['pending', 'Ждут оплаты'],
  ['canceled', 'Истекли и отменены'],
  ['paid', 'Оплачены'],
  ['all', 'Все'],
];

const MANUAL = new Set(['pending', 'canceled', 'failed']);
const NOT_MANUAL = new Set(['stars', 'balance']);

type PageData = { items: PaymentItem[]; next: number | null };

function stateOf(p: PaymentItem): { text: string; color: string } {
  switch (p.status) {
    case 'paid':
    case 'partially_refunded':
      return { text: `оплачен ${lowerFirst(agoLong(p.paid_at ?? p.created_at))}`, color: 'var(--muted)' };
    case 'pending':
      return { text: 'ждём оплату', color: 'var(--warn)' };
    case 'canceled':
      return { text: 'истёк или отменён', color: 'var(--muted)' };
    case 'refunded':
      return { text: 'возвращён', color: 'var(--muted)' };
    default:
      return { text: 'не прошёл', color: 'var(--bad)' };
  }
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function Money() {
  const [filter, setFilter] = useState<Filter>('pending');
  const [items, setItems] = useState<PaymentItem[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<PaymentItem | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const query = useCallback(
    (after: number | null) => {
      const params = new URLSearchParams({ limit: '30' });
      if (filter !== 'all') {
        params.set('status', filter);
      }
      if (after !== null) {
        params.set('after', String(after));
      }
      return api<PageData>(`/api/admin/payments?${params.toString()}`);
    },
    [filter],
  );

  const load = useCallback(async () => {
    setError(null);
    try {
      const page = await query(null);
      setItems(page.items);
      setNext(page.next);
    } catch (err) {
      setItems([]);
      setError(failText(err, 'Платежи не загрузились'));
    }
  }, [query]);

  useEffect(() => {
    setItems(null);
    setDone(null);
    void load();
  }, [load]);

  const more = async () => {
    if (next === null) {
      return;
    }
    setBusy(true);
    try {
      const page = await query(next);
      setItems((list) => [...(list ?? []), ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'Платежи не загрузились'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <h1 className="page-title">Деньги</h1>
      <section className="card mn-money" aria-labelledby="mo-h">
        <div className="mo-head">
          <h2 id="mo-h" className="section-title">
            Платежи
          </h2>
          <div role="group" aria-label="Какие платежи показать" className="segs">
            {FILTERS.map(([key, label]) => (
              <button key={key} type="button" aria-pressed={filter === key} onClick={() => setFilter(key)}>
                {label}
              </button>
            ))}
          </div>
        </div>
        {done ? (
          <p className="mo-done" role="status">
            {done}
          </p>
        ) : null}
        {error ? (
          <p className="err mo-pad" role="alert">
            {error}
          </p>
        ) : null}
        {items === null ? <p className="muted mo-pad">Загружаем платежи…</p> : null}
        {items !== null && items.length === 0 && !error ? <p className="muted mo-pad">Здесь пока пусто</p> : null}
        {items?.map((p) => {
          const name = personName(p.client, p.client_id);
          const state = stateOf(p);
          const manual = MANUAL.has(p.status) && !NOT_MANUAL.has(p.provider) && p.amount.currency !== 'XTR';
          return (
            <div key={p.id} className="mo-row">
              <Link to={`/admin/clients/${p.client_id}`} className="mo-who hov">
                <Avatar id={p.client_id} name={name} className="mo-ava" />
                <span className="mo-text">
                  <span className="mo-name">{name}</span>
                  <span className="muted">
                    {orderLine(p.order)} · {lowerFirst(methodText(p.provider, p.method))}
                  </span>
                </span>
              </Link>
              <span className="mo-sum">
                <strong className="num">{amountText(p.amount)}</strong>
                <span style={{ color: state.color }}>{state.text}</span>
              </span>
              <span className="mo-act">
                {manual ? (
                  <button type="button" className="btn-line hov" onClick={() => setConfirm(p)}>
                    Отметить оплаченным
                  </button>
                ) : null}
              </span>
            </div>
          );
        })}
        {next !== null ? (
          <div className="mo-pad">
            <button type="button" className="btn-line hov" onClick={() => void more()} disabled={busy}>
              {busy ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>
      {confirm ? (
        <ConfirmDialog
          title="Отметить платёж оплаченным?"
          text={`${personName(confirm.client, confirm.client_id)}, ${amountText(confirm.amount)}, ${lowerFirst(methodText(confirm.provider, confirm.method))}. Заказ выполнится так же, как после обычной оплаты: клиент получит подписку и сообщение. Отмечайте, только если деньги действительно пришли.`}
          confirm="Отметить оплаченным"
          run={async () => {
            await api(`/api/admin/payments/${confirm.id}/mark-paid`, {
              method: 'POST',
              body: { comment: 'Отмечено вручную в разделе «Деньги»' },
            });
            setDone(`Платёж клиента ${personName(confirm.client, confirm.client_id)} отмечен оплаченным`);
            setConfirm(null);
            await load();
          }}
          onClose={() => setConfirm(null)}
        />
      ) : null}
    </>
  );
}
