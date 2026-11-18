import { useCallback, useEffect, useId, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count } from '../../lib/format';
import { Avatar } from '../Avatar';
import { Dialog } from '../client/dialogs';
import type { Amount } from '../client/types';
import { amountText } from '../client/history';
import { agoLong, personName } from '../home/model';
import { useSession } from '../session';

type ReviewItem = {
  id: number;
  charge_id: string;
  event: 'successful_payment' | 'refunded_payment';
  stars: number | null;
  telegram_user_id: number | null;
  payer_client_id: number | null;
  payer_name: string | null;
  payment: { id: number; client_id: number; status: string; amount: Amount } | null;
  reason: string | null;
  refundable: boolean;
  retryable: boolean;
  created_at: string;
};

type ReviewPage = { items: ReviewItem[]; next: number | null; total: number };

type Retried = { outcome: 'done' | 'review' | 'pending'; reason: string | null };

type Action = { kind: 'retry' | 'refund' | 'close'; item: ReviewItem };

const INVOICE_STATE: Record<string, string> = {
  pending: 'ждёт оплаты',
  failed: 'не прошёл',
  canceled: 'истёк',
  paid: 'оплачен',
  partially_refunded: 'возвращён частично',
  refunded: 'возвращён',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function starsText(item: ReviewItem): string {
  return item.stars === null ? 'сумма неизвестна' : count(item.stars, 'звезда', 'звезды', 'звёзд');
}

function whatText(item: ReviewItem): string {
  const what = item.event === 'refunded_payment' ? 'Возврат' : 'Оплата';
  const invoice = item.payment ? ` · счёт на ${amountText(item.payment.amount)}, ${INVOICE_STATE[item.payment.status] ?? item.payment.status}` : '';
  return `${what} ${starsText(item)}${invoice}`;
}

function retriedText(result: Retried): string {
  if (result.outcome === 'done') {
    return 'Оплата проведена';
  }
  const reason = result.reason ? `: ${result.reason}` : '';
  return result.outcome === 'review' ? `Провести не получилось${reason}` : `Сразу не получилось, система повторит сама${reason}`;
}

function ActionDialog({ action, onClose, onDone }: { action: Action; onClose: () => void; onDone: (text: string) => void }) {
  const { item, kind } = action;
  const [comment, setComment] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fieldId = useId();
  const path = `/api/admin/stars/review/${item.id}/${kind}`;
  const notice = item.event === 'refunded_payment';
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      if (kind === 'retry') {
        onDone(retriedText(await api<Retried>(path, { method: 'POST' })));
      } else {
        await api(path, { method: 'POST', body: comment.trim() ? { comment: comment.trim() } : {} });
        onDone(kind === 'refund' ? `Звёзды вернули: ${starsText(item)}` : notice ? 'Уведомление о возврате закрыто' : 'Событие закрыто без возврата');
      }
    } catch (err) {
      setError(failText(err, 'Не получилось'));
      setBusy(false);
    }
  };
  const title = kind === 'retry' ? 'Провести оплату снова?' : kind === 'refund' ? 'Вернуть звёзды?' : notice ? 'Закрыть уведомление о возврате?' : 'Закрыть без возврата?';
  const text =
    kind === 'retry'
      ? `Система ещё раз проверит оплату на ${starsText(item)} и, если всё сходится, выдаст заказ. Второй раз заказ не выдаётся.`
      : kind === 'refund'
        ? `${starsText(item)} вернутся клиенту в Telegram, бот напишет ему об этом. Отменить возврат нельзя.`
        : notice
          ? 'Telegram уже вернул эти звёзды клиенту, платежи не изменятся. Напишите, почему событие закрывается: пояснение увидят другие администраторы.'
          : 'Звёзды останутся у сервиса. Напишите, почему событие закрывается: пояснение увидят другие администраторы.';
  const needComment = kind === 'close';
  return (
    <Dialog title={title} busy={busy} onClose={onClose}>
      <p className="muted">{text}</p>
      {kind === 'retry' ? null : (
        <div className="field-row">
          <label htmlFor={fieldId}>{needComment ? 'Пояснение' : 'Пояснение, если нужно'}</label>
          <textarea
            id={fieldId}
            className="field"
            rows={3}
            maxLength={500}
            value={comment}
            onChange={(event) => setComment(event.target.value)}
            disabled={busy}
            autoFocus
          />
        </div>
      )}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button
          type="button"
          className={kind === 'refund' ? 'btn-bad' : 'btn-main btn'}
          onClick={() => void run()}
          disabled={busy || (needComment && !comment.trim())}
        >
          {busy ? 'Выполняем…' : kind === 'retry' ? 'Провести снова' : kind === 'refund' ? 'Вернуть звёзды' : 'Закрыть'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

export function StarsReview({ onChanged }: { onChanged: () => void }) {
  const { can } = useSession();
  const refunds = can('refunds');
  const [items, setItems] = useState<ReviewItem[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [action, setAction] = useState<Action | null>(null);

  const query = useCallback((after: number | null) => {
    const params = new URLSearchParams({ limit: '20' });
    if (after !== null) {
      params.set('after', String(after));
    }
    return api<ReviewPage>(`/api/admin/stars/review?${params.toString()}`);
  }, []);

  const load = useCallback(async () => {
    try {
      const page = await query(null);
      setItems(page.items);
      setNext(page.next);
      setTotal(page.total);
      setError(null);
    } catch (err) {
      setItems([]);
      setError(failText(err, 'Оплаты звёздами на разборе не загрузились'));
    }
  }, [query]);

  useEffect(() => {
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
      setTotal(page.total);
    } catch (err) {
      setError(failText(err, 'Оплаты звёздами на разборе не загрузились'));
    } finally {
      setBusy(false);
    }
  };

  if (items === null || (items.length === 0 && !error && !done)) {
    return null;
  }

  return (
    <section className="card mn-money mo-stars" aria-labelledby="ms-h">
      <div className="mo-head">
        <h2 id="ms-h" className="section-title">
          Оплаты звёздами на разборе{total > 0 ? ` · ${total}` : ''}
        </h2>
      </div>
      <p className="muted mo-lead">Звёзды, которые не получилось провести автоматически. Здесь их можно провести снова, вернуть клиенту или закрыть с пояснением.</p>
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
      {items.length === 0 && !error ? <p className="muted mo-pad">Разбирать больше нечего</p> : null}
      {items.map((item) => {
        const clientId = item.payer_client_id ?? item.payment?.client_id ?? null;
        const name = item.payer_name ?? (clientId ? personName(null, clientId) : item.telegram_user_id ? `Telegram ${item.telegram_user_id}` : 'Плательщик неизвестен');
        const who = (
          <>
            <Avatar id={clientId ?? 0} name={name} className="mo-ava" />
            <span className="mo-text">
              <span className="mo-name">{name}</span>
              <span className="muted">{whatText(item)}</span>
            </span>
          </>
        );
        return (
          <div key={item.id} className="mo-row">
            {clientId ? (
              <Link to={`/admin/clients/${clientId}`} className="mo-who hov">
                {who}
              </Link>
            ) : (
              <span className="mo-who">{who}</span>
            )}
            <span className="mo-sum">
              <strong className="num">{starsText(item)}</strong>
              <span className="muted">{agoLong(item.created_at)}</span>
            </span>
            <span className="mo-act mo-acts">
              {item.retryable ? (
                <button type="button" className="btn-line hov" onClick={() => setAction({ kind: 'retry', item })}>
                  Провести снова
                </button>
              ) : null}
              {item.refundable && refunds ? (
                <button type="button" className="btn-line hov" onClick={() => setAction({ kind: 'refund', item })}>
                  Вернуть звёзды
                </button>
              ) : null}
              <button type="button" className="btn-line hov" onClick={() => setAction({ kind: 'close', item })}>
                Закрыть
              </button>
            </span>
            {item.reason ? <span className="muted mo-reason">Причина: {item.reason}</span> : null}
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
      {action ? (
        <ActionDialog
          action={action}
          onClose={() => setAction(null)}
          onDone={(text) => {
            setAction(null);
            setDone(text);
            void load();
            onChanged();
          }}
        />
      ) : null}
    </section>
  );
}
