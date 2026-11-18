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
import { GroupFrame, type Outcome } from './group';

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

function retriedText(result: Retried): Outcome {
  if (result.outcome === 'done') {
    return { text: 'Оплата проведена', tone: 'ok' };
  }
  const reason = result.reason ? `: ${result.reason}` : '';
  return {
    text: result.outcome === 'review' ? `Провести не получилось, оплата снова на разборе${reason}` : `Сразу не получилось, система повторит сама${reason}`,
    tone: 'warn',
  };
}

function ActionDialog({ action, onClose, onDone }: { action: Action; onClose: () => void; onDone: (outcome: Outcome) => void }) {
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
        onDone({
          text: kind === 'refund' ? `Звёзды вернули: ${starsText(item)}` : notice ? 'Уведомление о возврате закрыто' : 'Событие закрыто без возврата',
          tone: 'ok',
        });
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

type Props = { id: string; onChanged: () => void; onState: (visible: boolean, count: number) => void; round: number };

export function StarsReview({ id, onChanged, onState, round }: Props) {
  const { can } = useSession();
  const refunds = can('refunds');
  const [items, setItems] = useState<ReviewItem[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<Outcome | null>(null);
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
  }, [load, round]);

  const loaded = items !== null;
  const visible = loaded && (items.length > 0 || error !== null || done !== null);
  useEffect(() => {
    if (loaded) {
      onState(visible, total);
    }
  }, [onState, loaded, visible, total]);

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

  if (items === null || !visible) {
    return null;
  }

  return (
    <GroupFrame
      id={id}
      title="Оплаты звёздами на разборе"
      total={total}
      lead="Звёзды, которые не получилось провести автоматически. Здесь их можно провести снова, вернуть клиенту или закрыть с пояснением."
      outcome={done}
      error={error}
    >
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
          onDone={(outcome) => {
            setAction(null);
            setDone(outcome);
            onChanged();
          }}
        />
      ) : null}
    </GroupFrame>
  );
}
