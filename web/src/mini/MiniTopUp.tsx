import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { money } from '../lib/format';
import { finished, limitsText, parseAmount, wireAmount, type PaymentProgress, type TopUpOptions } from '../cabinet/payments';
import { BackHead, useMini } from './MiniApp';
import { openExternal, webApp } from './telegram';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniTopUp() {
  const { me, reload } = useMini();
  const [options, setOptions] = useState<TopUpOptions | null>(null);
  const [loadError, setLoadError] = useState<{ code: string; message: string } | null>(null);
  const [preset, setPreset] = useState<number | null>(null);
  const [custom, setCustom] = useState('');
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [otherWallet, setOtherWallet] = useState<string | null>(null);
  const [watch, setWatch] = useState<string | null>(null);
  const off = me.balance_enabled === false;

  useEffect(() => {
    if (off) {
      return;
    }
    let alive = true;
    api<TopUpOptions>('/api/client/topup')
      .then((next) => {
        if (alive) {
          setOptions(next);
          setPreset(next.presets[Math.min(1, next.presets.length - 1)]?.minor ?? null);
        }
      })
      .catch((err) => {
        if (alive) {
          setLoadError(err instanceof ApiError ? { code: err.code, message: err.message } : { code: 'network', message: 'Сервер не ответил' });
        }
      });
    return () => {
      alive = false;
    };
  }, [off]);

  useEffect(() => {
    if (!watch) {
      return;
    }
    let alive = true;
    let timer = 0;
    const tick = async (attempt: number) => {
      try {
        const progress = await api<PaymentProgress>(`/api/client/payments/${watch}`);
        if (!alive) {
          return;
        }
        if (finished(progress)) {
          setWatch(null);
          setOtherWallet(null);
          if (progress.order_status === 'fulfilled') {
            await reload();
            setNote('Баланс пополнен.');
          } else if (progress.order_status === 'review') {
            setNote('Оплата принята. Проверяем платёж, деньги поступят на баланс в ближайшее время.');
          } else {
            setNote(null);
            setError('Оплата не прошла. Попробуйте ещё раз или выберите другой способ.');
          }
          return;
        }
      } catch {
        if (!alive) {
          return;
        }
      }
      if (attempt < 75) {
        timer = window.setTimeout(() => void tick(attempt + 1), 4000);
      }
    };
    timer = window.setTimeout(() => void tick(0), 4000);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [watch, reload]);

  const currency = options?.currency ?? me.currency;
  const typed = custom.trim() ? parseAmount(custom, currency) : null;
  const amount = custom.trim() ? typed : preset;
  const inRange = !!options && amount !== null && amount >= options.min.minor && amount <= options.max.minor;
  const amountError =
    options && custom.trim()
      ? typed === null
        ? currency === 'RUB'
          ? 'Сумма — целое число рублей, например 700'
          : 'Сумма — число, например 15 или 12.50'
        : !inRange
          ? `Сумма должна быть ${limitsText(options)}`
          : null
      : null;
  const methods = options?.methods ?? [];
  const chosen = methods.find((m) => m.provider === method) ?? methods[0] ?? null;
  const inTelegramWallet = chosen?.provider === 'gram' && webApp() !== null;
  const sum = amount !== null && inRange ? money({ minor: amount, currency }) : null;

  const pay = async () => {
    if (!options || amount === null || !inRange || !chosen) {
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    setOtherWallet(null);
    try {
      const res = await api<{ type: 'redirect'; payment_id: string; pay_url: string } | { type: 'completed'; order_id: string }>('/api/client/topup', {
        method: 'POST',
        body: { amount: wireAmount(amount, currency), provider: chosen.provider },
      });
      if (res.type !== 'redirect') {
        await reload();
        setNote('Баланс пополнен.');
        return;
      }
      setWatch(res.payment_id);
      if (inTelegramWallet) {
        try {
          const sent = await api<{ chat_url: string }>(`/api/client/payments/${res.payment_id}/telegram`, { method: 'POST' });
          setOtherWallet(res.pay_url);
          setNote('Счёт отправлен в чат с ботом. Нажмите там «Оплатить в Telegram» — кошелёк откроется с готовой суммой.');
          openExternal(sent.chat_url);
          return;
        } catch (err) {
          setError(failText(err, 'Счёт в чат не отправился, открываем другой кошелёк'));
        }
      }
      openExternal(res.pay_url);
      setNote('Оплата открылась в новом окне. Деньги поступят на баланс сами, как только платёжная система подтвердит оплату.');
    } catch (err) {
      setError(failText(err, 'Оплата не началась'));
    } finally {
      setBusy(false);
    }
  };

  if (off || loadError?.code === 'balance.disabled') {
    return (
      <main className="mn-page">
        <BackHead title="Пополнение" />
        <div className="mn-card mn-pad">
          <strong>Пополнение выключено</strong>
          <span className="mn-ink2">Пополнить баланс сейчас нельзя. Оплатите подписку напрямую одним из способов оплаты.</span>
          <Link to="/app/plans" className="mn-main sm press">
            К тарифам
          </Link>
        </div>
      </main>
    );
  }

  return (
    <main className="mn-page topup">
      <BackHead title="Пополнение" />
      <p className="mn-notes">
        Сейчас на балансе {money(me.balance)}. С баланса можно оплатить подписку и продление, если денег хватает на всю сумму.
      </p>
      {!options ? <p className="mn-muted">{loadError?.message ?? 'Загружаем…'}</p> : null}
      {options && options.presets.length > 0 ? (
        <div role="radiogroup" aria-label="Сумма пополнения" className="mn-amounts">
          {options.presets.map((p) => (
            <button
              key={p.minor}
              type="button"
              role="radio"
              aria-checked={!custom.trim() && preset === p.minor}
              className="mn-amount press num"
              onClick={() => {
                setPreset(p.minor);
                setCustom('');
              }}
            >
              {money(p)}
            </button>
          ))}
        </div>
      ) : null}
      {options ? (
        <div className="mn-field">
          <input
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            inputMode={currency === 'RUB' ? 'numeric' : 'decimal'}
            placeholder={options.presets.length > 0 ? 'Другая сумма' : 'Сумма пополнения'}
            aria-label={options.presets.length > 0 ? 'Другая сумма' : 'Сумма пополнения'}
            aria-invalid={amountError ? true : undefined}
            aria-describedby="mini-topup-hint"
            maxLength={12}
          />
          <span id="mini-topup-hint" className={amountError ? 'mn-error' : 'mn-muted small'}>
            {amountError ?? `Можно ${limitsText(options)}`}
          </span>
        </div>
      ) : null}
      {options ? (
        <div className="mn-pay">
          <div className="mn-pay-head">
            <h2>Оплата</h2>
          </div>
          {methods.length === 0 ? (
            <p className="mn-muted small">
              В мини-приложении пополнить сейчас нельзя.{' '}
              <button type="button" className="mn-link press" onClick={() => openExternal(`${window.location.origin}/cabinet/topup`)}>
                Пополнить в кабинете на сайте
              </button>
            </p>
          ) : null}
          <div role="radiogroup" aria-label="Способ оплаты" className="mn-chips">
            {methods.map((m) => (
              <button key={m.provider} type="button" role="radio" aria-checked={chosen?.provider === m.provider} className="mn-chip press" onClick={() => setMethod(m.provider)}>
                {m.title}
              </button>
            ))}
          </div>
          {error ? <p className="mn-error">{error}</p> : null}
          {note ? (
            <p role="status" className="mn-note">
              {note}
              {otherWallet ? (
                <>
                  {' '}
                  <button type="button" className="mn-link press" onClick={() => openExternal(otherWallet)}>
                    Оплатить другим кошельком
                  </button>
                </>
              ) : null}
            </p>
          ) : null}
        </div>
      ) : null}
      {options ? (
        <button type="button" className="mn-main mn-paybtn press" onClick={() => void pay()} disabled={busy || !sum || !chosen}>
          {busy ? 'Открываем оплату…' : sum ? `Пополнить на ${sum}${inTelegramWallet ? ' в Telegram' : ''}` : 'Пополнить'}
        </button>
      ) : null}
    </main>
  );
}
