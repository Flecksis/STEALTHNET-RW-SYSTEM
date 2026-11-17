import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { absoluteUrl, amountText, expired, PAGE, paymentNote, paymentState, paymentWhen, payUntil, purchaseText, type ClientPayment, type PaymentsPage } from '../cabinet/payments';
import { BackHead } from './MiniApp';
import { openExternal } from './telegram';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniPayments() {
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

  return (
    <main className="mn-page">
      <BackHead title="История оплат" />
      {items === null && !error ? <p className="mn-muted">Загружаем…</p> : null}
      {items && items.length === 0 ? (
        <div className="mn-card mn-pad">
          <span className="mn-ink2">Оплат пока нет.</span>
          <Link to="/app/plans" className="mn-main sm press">
            Выбрать тариф
          </Link>
        </div>
      ) : null}
      {items && items.length > 0 ? (
        <ul className="mn-list mn-payrows" aria-label="Оплаты">
          {items.map((p) => {
            const state = paymentState(p);
            const dim = p.status === 'canceled' || p.status === 'failed' || expired(p);
            return (
              <li key={p.id} className="mn-payrow">
                <span className="mn-payrow-main">
                  <strong>{purchaseText(p)}</strong>
                  <span>
                    {paymentWhen(p)} · {paymentNote(p)}
                  </span>
                </span>
                <span className="mn-payrow-side">
                  <strong className={dim ? 'num mn-muted' : 'num'}>{amountText(p.amount)}</strong>
                  <span className={`mn-tone ${state.tone}`}>{state.text}</span>
                </span>
                {p.pay_url ? (
                  <button type="button" className="mn-link press mn-payrow-pay" onClick={() => openExternal(absoluteUrl(p.pay_url ?? ''))}>
                    {payUntil(p)}
                  </button>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
      {error ? (
        <div className="mn-card mn-pad">
          <span className="mn-error">{error}</span>
          <button type="button" className="mn-btn2 press" onClick={() => void load(items && items.length > 0 ? next : null)} disabled={busy}>
            Повторить
          </button>
        </div>
      ) : null}
      {next && !error ? (
        <button type="button" className="mn-btn2 press" onClick={() => void load(next)} disabled={busy}>
          {busy ? 'Загружаем…' : 'Показать ещё'}
        </button>
      ) : null}
    </main>
  );
}
