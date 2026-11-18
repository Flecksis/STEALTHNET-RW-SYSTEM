import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { dateLong, money } from '../lib/format';
import { siteInviteLink } from '../lib/referral';
import { Card, useCabinet } from './CabinetApp';
import type { PayoutMethod, Referral, Withdrawal } from './model';
import { PAYOUT_METHODS as METHODS, payoutDetails, payoutLabel as methodLabel, WITHDRAWAL_STATUS as STATUS } from './withdrawals';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function shareUrl(link: string, brand: string): string {
  const text = `Пользуюсь ${brand} — подключайся по моей ссылке`;
  return `https://t.me/share/url?url=${encodeURIComponent(link)}&text=${encodeURIComponent(text)}`;
}

export function FriendsPage() {
  const { site, me } = useCabinet();
  const [data, setData] = useState<Referral | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [amount, setAmount] = useState('');
  const [method, setMethod] = useState<PayoutMethod>(me.currency === 'RUB' ? 'sbp' : 'usdt_trc20');
  const [details, setDetails] = useState('');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api<Referral>('/api/client/m/referral/me'));
      setError(null);
    } catch (err) {
      if (err instanceof ApiError && (err.status === 404 || err.status === 403)) {
        setData(null);
      } else {
        setError(failText(err, 'Программа приглашений не загрузилась'));
        setData((d) => (d === undefined ? null : d));
      }
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const copy = async (text: string, key: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      setError('Браузер не дал скопировать: выделите ссылку вручную');
    }
  };

  if (data === undefined) {
    return <p className="cb-muted cb-pad">Загружаем…</p>;
  }

  if (data === null) {
    return (
      <div className="cb-history">
        <header className="cb-pay-head">
          <h1 className="cb-title">Друзья</h1>
        </header>
        {error ? (
          <p className="cb-error" role="alert">
            {error}
          </p>
        ) : (
          <Card label="Друзья">
            <p className="cb-ink2">Программа приглашений сейчас не работает.</p>
          </Card>
        )}
      </div>
    );
  }

  const first = data.levels[0] ?? 0;
  const second = data.levels[1] ?? 0;
  const siteLink = siteInviteLink(data.code);
  const balance = data.balance ?? me.referral_balance;
  const canWithdraw = data.withdrawals_enabled === true;
  const hint = METHODS.find((m) => m.key === method)?.hint ?? '';
  const withdrawals = data.withdrawals ?? [];

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
    try {
      await api(`/api/client/m/referral/withdrawals/${item.id}/cancel`, { method: 'POST', body: {} });
      await load();
    } catch (err) {
      setError(failText(err, 'Заявка не отменилась'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <h1 className="cb-title">Друзья</h1>
        <p className="cb-lead muted">
          {first > 0 ? `Пригласите друга и получайте ${first}% от его оплат.` : 'Пригласите друга по своей ссылке.'}
          {second > 0 ? ` Ещё ${second}% — от оплат тех, кого пригласят ваши друзья.` : ''}
        </p>
      </header>
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}

      <Card className="cb-friends" label="Ссылки для приглашения">
        <h2 className="cb-h2">Ваша ссылка</h2>
        <span className="cb-small cb-muted">На сайт</span>
        <div className="cb-linkrow">
          <span className="cb-link mono">{siteLink.replace(/^https?:\/\//, '')}</span>
          <button type="button" className="cb-second" onClick={() => void copy(siteLink, 'site')}>
            {copied === 'site' ? 'Скопировано' : 'Скопировать'}
          </button>
        </div>
        {data.bot_link ? (
          <>
            <span className="cb-small cb-muted">В Telegram-бота</span>
            <div className="cb-linkrow">
              <span className="cb-link mono">{data.bot_link.replace(/^https:\/\//, '')}</span>
              <button type="button" className="cb-second" onClick={() => void copy(data.bot_link ?? '', 'bot')}>
                {copied === 'bot' ? 'Скопировано' : 'Скопировать'}
              </button>
            </div>
          </>
        ) : null}
        <div className="cb-btns">
          <a className="cb-second" href={shareUrl(data.bot_link ?? siteLink, site.brand_name)} target="_blank" rel="noreferrer">
            Отправить в Telegram
          </a>
        </div>
      </Card>

      <Card className="cb-refstats" label="Итоги">
        <dl className="cb-facts">
          <div>
            <dt>Пришли по ссылке</dt>
            <dd className="num">{data.invited}</dd>
          </div>
          <div>
            <dt>Заработано</dt>
            <dd className="num">{money(data.earned)}</dd>
          </div>
          <div>
            <dt>Можно вывести</dt>
            <dd className="num">{money(balance)}</dd>
          </div>
        </dl>
      </Card>

      {canWithdraw && balance.minor > 0 ? (
        <Card className="cb-withdraw" label="Вывод">
          <h2 className="cb-h2">Вывести заработанное</h2>
          {data.withdraw_min ? <p className="cb-small cb-muted">От {money(data.withdraw_min)}. Заявку проверяют вручную, обычно за день-два.</p> : null}
          <label className="cb-field">
            Сумма
            <span className="cb-wamount">
              <input inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="0" disabled={busy} />
              <button type="button" className="cb-text" onClick={() => setAmount(String(balance.minor / 100))} disabled={busy || balance.minor <= 0}>
                Всё
              </button>
            </span>
          </label>
          <span className="cb-field">
            Куда
            <span role="group" aria-label="Способ вывода" className="cb-seg">
              {METHODS.map((m) => (
                <button key={m.key} type="button" aria-pressed={method === m.key} onClick={() => setMethod(m.key)} disabled={busy}>
                  {m.label}
                </button>
              ))}
            </span>
          </span>
          <label className="cb-field">
            Реквизиты
            <input value={details} onChange={(e) => setDetails(e.target.value)} placeholder={hint} maxLength={200} disabled={busy} />
          </label>
          {status ? (
            <p className={status.bad ? 'cb-error' : 'cb-small cb-muted'} role={status.bad ? 'alert' : 'status'}>
              {status.text}
            </p>
          ) : null}
          <div className="cb-btns">
            <button type="button" className="cb-main" onClick={() => void submit()} disabled={busy}>
              {busy ? 'Отправляем…' : 'Отправить заявку'}
            </button>
          </div>
        </Card>
      ) : balance.minor > 0 ? (
        <p className="cb-small cb-muted">Вывод заработанного сейчас не принимается.</p>
      ) : null}

      {withdrawals.length > 0 ? (
        <Card className="cb-paylist" label="Заявки на вывод">
          <ul className="cb-payrows">
            {withdrawals.map((w) => {
              const st = STATUS[w.status];
              const open = w.status === 'holding' || w.status === 'pending';
              return (
                <li key={w.id} className="cb-payrow">
                  <span className="cb-payrow-main">
                    <strong>
                      {methodLabel(w.method)} · {payoutDetails(w.details)}
                    </strong>
                    <span className="cb-small cb-muted">
                      {dateLong(w.created_at, true)}
                      {w.comment ? ` · ${w.comment}` : ''}
                    </span>
                  </span>
                  <span className="cb-payrow-side">
                    <strong className="num">{money(w.amount)}</strong>
                    <span className={`cb-small cb-status ${st.tone}`}>{st.text}</span>
                    {open ? (
                      <button type="button" className="cb-text" onClick={() => void cancel(w)} disabled={busy}>
                        Отменить
                      </button>
                    ) : null}
                  </span>
                </li>
              );
            })}
          </ul>
        </Card>
      ) : null}
    </div>
  );
}
