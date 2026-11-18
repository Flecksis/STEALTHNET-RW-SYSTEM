import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { dateLong, money } from '../lib/format';
import { t } from '../lib/i18n';
import type { Referral, Withdrawal } from '../cabinet/model';
import { firstPayout, payoutDetails, payoutLabel, WITHDRAWAL_STATUS } from '../cabinet/withdrawals';
import { BackHead, useMini } from './MiniApp';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniWithdraw() {
  const { me } = useMini();
  const [data, setData] = useState<Referral | null | undefined>(undefined);
  const [amount, setAmount] = useState('');
  const [method, setMethod] = useState('');
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
        <BackHead title={t('Вывод')} />
        <p className="mn-muted">{t('Загружаем…')}</p>
      </main>
    );
  }

  const balance = data?.balance ?? null;
  const methods = data?.payout_methods ?? [];
  const chosen = methods.find((m) => m.id === method) ?? methods.find((m) => m.id === firstPayout(methods, me.currency)) ?? null;
  const canWithdraw = data?.withdrawals_enabled === true && methods.length > 0 && balance !== null && balance.minor > 0;
  const withdrawals = data?.withdrawals ?? [];

  const submit = async () => {
    if (busy) {
      return;
    }
    if (!chosen) {
      setStatus({ text: t('Выберите, куда вывести'), bad: true });
      return;
    }
    if (!amount.trim() || !details.trim()) {
      setStatus({ text: t('Укажите сумму и реквизиты'), bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      await api('/api/client/m/referral/withdrawals', { method: 'POST', body: { amount: amount.trim().replace(',', '.'), method: chosen.id, details: details.trim() } });
      setAmount('');
      setDetails('');
      setStatus({ text: t('Заявка отправлена. Деньги придут после проверки, статус виден ниже.'), bad: false });
      await load();
    } catch (err) {
      setStatus({ text: failText(err, t('Заявка не отправилась')), bad: true });
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
      setStatus({ text: failText(err, t('Заявка не отменилась')), bad: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="mn-page withdraw">
      <BackHead title={t('Вывод')} />
      {canWithdraw && balance ? (
        <>
          <p className="mn-notes">
            {chosen?.min
              ? t('Можно вывести {amount}. Минимум — {min}. Заявку проверяют вручную, обычно за день-два.', { amount: money(balance), min: money(chosen.min) })
              : t('Можно вывести {amount}. Заявку проверяют вручную, обычно за день-два.', { amount: money(balance) })}
          </p>
          <div className="mn-field">
            <input
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
              inputMode="decimal"
              placeholder={t('Сумма')}
              aria-label={t('Сумма вывода')}
              maxLength={12}
              disabled={busy}
            />
            <button type="button" className="mn-link press" onClick={() => setAmount(String(balance.minor / 100))} disabled={busy}>
              {t('Вывести всё')}
            </button>
          </div>
          <div role="radiogroup" aria-label={t('Куда вывести')} className="mn-chips">
            {methods.map((m) => (
              <button key={m.id} type="button" role="radio" aria-checked={chosen?.id === m.id} className="mn-chip press" onClick={() => setMethod(m.id)} disabled={busy}>
                {m.title}
              </button>
            ))}
          </div>
          <div className="mn-field">
            <input value={details} onChange={(e) => setDetails(e.target.value)} placeholder={chosen?.hint || t('Реквизиты')} aria-label={t('Реквизиты')} maxLength={200} disabled={busy} />
          </div>
          {status ? (
            <p className={status.bad ? 'mn-error' : 'mn-note'} role={status.bad ? 'alert' : 'status'}>
              {status.text}
            </p>
          ) : null}
          <button type="button" className="mn-main mn-paybtn press" onClick={() => void submit()} disabled={busy}>
            {busy ? t('Отправляем…') : t('Отправить заявку')}
          </button>
        </>
      ) : status ? (
        <p className={status.bad ? 'mn-error' : 'mn-note'} role={status.bad ? 'alert' : 'status'}>
          {status.text}
        </p>
      ) : null}
      {withdrawals.length > 0 ? (
        <ul className="mn-list mn-payrows" aria-label={t('Заявки на вывод')}>
          {withdrawals.map((w) => {
            const st = WITHDRAWAL_STATUS[w.status];
            const open = w.status === 'holding' || w.status === 'pending';
            return (
              <li key={w.id} className="mn-payrow">
                <span className="mn-payrow-main">
                  <strong>
                    {payoutLabel(w)} · {payoutDetails(w.details)}
                  </strong>
                  <span>
                    {dateLong(w.created_at, true)}
                    {w.comment ? ` · ${w.comment}` : ''}
                  </span>
                </span>
                <span className="mn-payrow-side">
                  <strong className="num">{money(w.amount)}</strong>
                  <span className={`mn-tone ${st.tone}`}>{t(st.text)}</span>
                </span>
                {open ? (
                  <button type="button" className="mn-link press mn-payrow-pay" onClick={() => void cancel(w)} disabled={busy}>
                    {t('Отменить заявку')}
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
