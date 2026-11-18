import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { dateLong, money } from '../lib/format';
import type { PayoutMethod, Referral, Withdrawal } from '../cabinet/model';
import { PAYOUT_METHODS, payoutDetails, payoutLabel, WITHDRAWAL_STATUS } from '../cabinet/withdrawals';
import { BackHead, useMini } from './MiniApp';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniWithdraw() {
  const { me } = useMini();
  const [data, setData] = useState<Referral | null | undefined>(undefined);
  const [amount, setAmount] = useState('');
  const [method, setMethod] = useState<PayoutMethod>(me.currency === 'RUB' ? 'sbp' : 'usdt_trc20');
  const [details, setDetails] = useState('');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api<Referral>('/api/client/m/referral/me'));
    } catch {
      setData(null);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (data === undefined) {
    return (
      <main className="mn-page">
        <BackHead title="Вывод" />
        <p className="mn-muted">Загружаем…</p>
      </main>
    );
  }

  const balance = data?.balance ?? null;
  const canWithdraw = data?.withdrawals_enabled === true && balance !== null && balance.minor > 0;
  const withdrawals = data?.withdrawals ?? [];
  const hint = PAYOUT_METHODS.find((m) => m.key === method)?.hint ?? '';

  const submit = async () => {
    if (busy) {
      return;
    }
    if (!amount.trim() || !details.trim()) {
      setStatus({ text: 'Укажите сумму и реквизиты', bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      await api('/api/client/m/referral/withdrawals', { method: 'POST', body: { amount: amount.trim().replace(',', '.'), method, details: details.trim() } });
      setAmount('');
      setDetails('');
      setStatus({ text: 'Заявка отправлена. Деньги придут после проверки, статус виден ниже.', bad: false });
      await load();
    } catch (err) {
      setStatus({ text: failText(err, 'Заявка не отправилась'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const cancel = async (item: Withdrawal) => {
    setBusy(true);
    setStatus(null);
    try {
      await api(`/api/client/m/referral/withdrawals/${item.id}/cancel`, { method: 'POST', body: {} });
      await load();
    } catch (err) {
      setStatus({ text: failText(err, 'Заявка не отменилась'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="mn-page withdraw">
      <BackHead title="Вывод" />
      {canWithdraw && balance ? (
        <>
          <p className="mn-notes">
            Можно вывести {money(balance)}.{data?.withdraw_min ? ` Минимум — ${money(data.withdraw_min)}.` : ''} Заявку проверяют вручную, обычно за день-два.
          </p>
          <div className="mn-field">
            <input
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
              inputMode="decimal"
              placeholder="Сумма"
              aria-label="Сумма вывода"
              maxLength={12}
              disabled={busy}
            />
            <button type="button" className="mn-link press" onClick={() => setAmount(String(balance.minor / 100))} disabled={busy}>
              Вывести всё
            </button>
          </div>
          <div role="radiogroup" aria-label="Куда вывести" className="mn-chips">
            {PAYOUT_METHODS.map((m) => (
              <button key={m.key} type="button" role="radio" aria-checked={method === m.key} className="mn-chip press" onClick={() => setMethod(m.key)} disabled={busy}>
                {m.label}
              </button>
            ))}
          </div>
          <div className="mn-field">
            <input value={details} onChange={(e) => setDetails(e.target.value)} placeholder={hint} aria-label="Реквизиты" maxLength={200} disabled={busy} />
          </div>
          {status ? (
            <p className={status.bad ? 'mn-error' : 'mn-note'} role={status.bad ? 'alert' : 'status'}>
              {status.text}
            </p>
          ) : null}
          <button type="button" className="mn-main mn-paybtn press" onClick={() => void submit()} disabled={busy}>
            {busy ? 'Отправляем…' : 'Отправить заявку'}
          </button>
        </>
      ) : status ? (
        <p className={status.bad ? 'mn-error' : 'mn-note'} role={status.bad ? 'alert' : 'status'}>
          {status.text}
        </p>
      ) : null}
      {withdrawals.length > 0 ? (
        <ul className="mn-list mn-payrows" aria-label="Заявки на вывод">
          {withdrawals.map((w) => {
            const st = WITHDRAWAL_STATUS[w.status];
            const open = w.status === 'holding' || w.status === 'pending';
            return (
              <li key={w.id} className="mn-payrow">
                <span className="mn-payrow-main">
                  <strong>
                    {payoutLabel(w.method)} · {payoutDetails(w.details)}
                  </strong>
                  <span>
                    {dateLong(w.created_at, true)}
                    {w.comment ? ` · ${w.comment}` : ''}
                  </span>
                </span>
                <span className="mn-payrow-side">
                  <strong className="num">{money(w.amount)}</strong>
                  <span className={`mn-tone ${st.tone}`}>{st.text}</span>
                </span>
                {open ? (
                  <button type="button" className="mn-link press mn-payrow-pay" onClick={() => void cancel(w)} disabled={busy}>
                    Отменить заявку
                  </button>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
    </main>
  );
}
