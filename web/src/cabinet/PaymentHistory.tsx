import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { money } from '../lib/format';
import { Card, useCabinet } from './CabinetApp';
import { absoluteUrl, amountText, expired, PAGE, paymentNote, paymentState, paymentWhen, payUntil, purchaseText, type ClientPayment, type PaymentsPage } from './payments';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function PaymentRow({ payment }: { payment: ClientPayment }) {
  const state = paymentState(payment);
  const dim = payment.status === 'canceled' || payment.status === 'failed' || expired(payment);
  return (
    <li className="cb-payrow">
      <span className="cb-payrow-main">
        <strong>{purchaseText(payment)}</strong>
        <span className="cb-small cb-muted">
          {paymentWhen(payment)} · {paymentNote(payment)}
        </span>
      </span>
      <span className="cb-payrow-side">
        <strong className={dim ? 'num cb-muted' : 'num'}>{amountText(payment.amount)}</strong>
        <span className={`cb-small cb-status ${state.tone}`}>{state.text}</span>
      </span>
      {payment.pay_url ? (
        <a className="cb-second sm cb-payrow-pay" href={absoluteUrl(payment.pay_url)}>
          {payUntil(payment)}
        </a>
      ) : null}
    </li>
  );
}

export function PaymentsHistory() {
  const { me } = useCabinet();
  const [items, setItems] = useState<ClientPayment[] | null>(null);
  const [next, setNext] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async (before: string | null) => {
    setBusy(true);
    setError(null);
    try {
      const query = before ? `?limit=${PAGE}&before=${encodeURIComponent(before)}` : `?limit=${PAGE}`;
      const page = await api<PaymentsPage>(`/api/client/payments${query}`);
      setItems((prev) => (before && prev ? [...prev, ...page.items] : page.items));
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'История не загрузилась'));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load(null);
  }, [load]);

  const canTopUp = me.balance_enabled !== false;

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <h1 className="cb-title">История оплат</h1>
        {canTopUp ? (
          <p className="cb-lead muted">
            На балансе {money(me.balance)} ·{' '}
            <Link to="/cabinet/topup" className="cb-accent-link">
              Пополнить
            </Link>
          </p>
        ) : null}
      </header>
      {items === null && !error ? <p className="cb-muted">Загружаем историю…</p> : null}
      {items && items.length === 0 ? (
        <Card label="Оплаты">
          <p className="cb-ink2">Оплат пока нет.</p>
          <div className="cb-btns">
            <Link to="/cabinet/pay" className="cb-main">
              Выбрать тариф
            </Link>
            {canTopUp ? (
              <Link to="/cabinet/topup" className="cb-second">
                Пополнить баланс
              </Link>
            ) : null}
          </div>
        </Card>
      ) : null}
      {items && items.length > 0 ? (
        <Card className="cb-paylist" label="Оплаты">
          <ul className="cb-payrows">
            {items.map((p) => (
              <PaymentRow key={p.id} payment={p} />
            ))}
          </ul>
        </Card>
      ) : null}
      {error ? (
        <div className="cb-btns">
          <p className="cb-error" role="alert">
            {error}
          </p>
          <button type="button" className="cb-second sm" onClick={() => void load(items && items.length > 0 ? next : null)} disabled={busy}>
            Повторить
          </button>
        </div>
      ) : null}
      {next && !error ? (
        <button type="button" className="cb-second cb-more" onClick={() => void load(next)} disabled={busy}>
          {busy ? 'Загружаем…' : 'Показать ещё'}
        </button>
      ) : null}
    </div>
  );
}
