import { useEffect, useMemo, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { money } from '../lib/format';
import { Icon } from '../lib/icons';
import { Card, useCabinet } from './CabinetApp';
import { finished, limitsText, parseAmount, wireAmount, type PaymentProgress, type TopUpOptions } from './payments';

const TOPUP_KEY = 'sn.cabinet.topup';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function readKey(): string | null {
  try {
    return window.sessionStorage.getItem(TOPUP_KEY);
  } catch {
    return null;
  }
}

function forgetKey(): void {
  try {
    window.sessionStorage.removeItem(TOPUP_KEY);
  } catch {
    return;
  }
}

export function TopUpPage() {
  const { me, reload } = useCabinet();
  const [params, setParams] = useSearchParams();
  const [asked] = useState(() => params.get('amount') ?? '');
  const [options, setOptions] = useState<TopUpOptions | null>(null);
  const [loadError, setLoadError] = useState<{ code: string; message: string } | null>(null);
  const [preset, setPreset] = useState<number | null>(null);
  const [custom, setCustom] = useState('');
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [waiting, setWaiting] = useState<PaymentProgress | null>(null);
  const off = me.balance_enabled === false;

  useEffect(() => {
    if (off) {
      return;
    }
    let alive = true;
    api<TopUpOptions>('/api/client/topup')
      .then((next) => {
        if (!alive) {
          return;
        }
        setOptions(next);
        const wanted = parseAmount(asked, next.currency);
        if (wanted !== null) {
          setCustom(wireAmount(wanted, next.currency));
        } else {
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
  }, [off, asked]);

  useEffect(() => {
    const id = params.get('payment') ?? readKey();
    if (!id) {
      return;
    }
    let alive = true;
    const tick = async (attempt: number) => {
      try {
        const progress = await api<PaymentProgress>(`/api/client/payments/${id}`);
        if (!alive) {
          return;
        }
        setWaiting(progress);
        if (finished(progress)) {
          forgetKey();
          if (progress.order_status === 'fulfilled') {
            await reload();
          }
          return;
        }
        if (attempt < 60) {
          window.setTimeout(() => void tick(attempt + 1), 3000);
        }
      } catch {
        forgetKey();
        setWaiting(null);
      }
    };
    void tick(0);
    return () => {
      alive = false;
    };
  }, [params, reload]);

  const currency = options?.currency ?? me.currency;
  const typed = custom.trim() ? parseAmount(custom, currency) : null;
  const amount = custom.trim() ? typed : preset;
  const inRange = !!options && amount !== null && amount >= options.min.minor && amount <= options.max.minor;
  const amountError =
    options && custom.trim()
      ? typed === null
        ? currency === 'RUB'
          ? 'Укажите сумму целым числом рублей, например 700'
          : 'Укажите сумму числом, например 15 или 12.50'
        : !inRange
          ? `Сумма должна быть ${limitsText(options)}`
          : null
      : null;
  const methods = useMemo(() => options?.methods ?? [], [options]);
  const chosen = methods.find((m) => m.provider === method) ?? methods[0] ?? null;
  const sum = amount !== null && inRange ? money({ minor: amount, currency }) : null;
  const after = amount !== null && inRange ? money({ minor: me.balance.minor + amount, currency }) : null;

  const pay = async () => {
    if (!options || amount === null || !inRange || !chosen) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await api<{ type: 'redirect'; payment_id: string; pay_url: string } | { type: 'completed'; order_id: string }>('/api/client/topup', {
        method: 'POST',
        body: { amount: wireAmount(amount, currency), provider: chosen.provider, return_url: `${window.location.origin}/cabinet/topup` },
      });
      if (res.type === 'redirect') {
        try {
          window.sessionStorage.setItem(TOPUP_KEY, res.payment_id);
        } catch {
          setParams({ payment: res.payment_id });
        }
        window.location.href = res.pay_url;
        return;
      }
      await reload();
    } catch (err) {
      setError(failText(err, 'Оплата не началась'));
    } finally {
      setBusy(false);
    }
  };

  if (waiting) {
    const done = waiting.order_status === 'fulfilled';
    const review = waiting.order_status === 'review';
    const failed = waiting.order_status === 'canceled' || waiting.order_status === 'refunded';
    return (
      <div className="cb-stack cb-narrow">
        <Card className="cb-wait" label="Пополнение">
          <h1 className="cb-h1">{done ? 'Баланс пополнен' : review ? 'Оплата принята' : failed ? 'Оплата не прошла' : 'Ждём оплату'}</h1>
          <p className="cb-ink2">
            {done
              ? `На балансе ${money(me.balance)}.`
              : review
                ? 'Проверяем платёж, деньги поступят на баланс в ближайшее время.'
                : failed
                  ? 'Деньги не списаны или возвращены. Попробуйте ещё раз или выберите другой способ.'
                  : 'Как только платёжная система подтвердит оплату, деньги поступят на баланс сами.'}
          </p>
          <div className="cb-btns">
            <Link to="/cabinet" className="cb-main">
              В кабинет
            </Link>
            {!done && !review && !failed && waiting.pay_url ? (
              <a className="cb-second" href={waiting.pay_url}>
                Вернуться к оплате
              </a>
            ) : null}
            {failed ? (
              <button
                type="button"
                className="cb-second"
                onClick={() => {
                  forgetKey();
                  setWaiting(null);
                  setParams({});
                }}
              >
                Выбрать заново
              </button>
            ) : (
              <Link to="/cabinet/payments" className="cb-second">
                История оплат
              </Link>
            )}
          </div>
        </Card>
      </div>
    );
  }

  if (off || loadError?.code === 'balance.disabled') {
    return (
      <div className="cb-stack cb-narrow">
        <Card label="Пополнение">
          <h1 className="cb-h1">Пополнение выключено</h1>
          <p className="cb-ink2">Пополнить баланс сейчас нельзя. Оплатите подписку напрямую одним из способов оплаты.</p>
          <div className="cb-btns">
            <Link to="/cabinet/pay" className="cb-main">
              К тарифам
            </Link>
            <Link to="/cabinet/payments" className="cb-second">
              История оплат
            </Link>
          </div>
        </Card>
      </div>
    );
  }

  if (loadError) {
    return (
      <p className="cb-error cb-pad" role="alert">
        {loadError.message}
      </p>
    );
  }

  if (!options) {
    return <p className="cb-muted cb-pad">Загружаем способы пополнения…</p>;
  }

  return (
    <div className="cb-pay">
      <header className="cb-pay-head">
        <h1 className="cb-title">Пополнение баланса</h1>
        <p className="cb-lead muted">Сейчас на балансе {money(options.balance)}. С баланса можно оплатить подписку и продление, если денег хватает на всю сумму.</p>
      </header>
      <div className="cb-cols">
        <div className="cb-col left">
          <Card className="cb-pick" label="Сумма">
            <h2 className="cb-h2">Сумма</h2>
            {options.presets.length > 0 ? (
              <div role="radiogroup" aria-label="Сумма пополнения" className="cb-amounts">
                {options.presets.map((p) => (
                  <button
                    key={p.minor}
                    type="button"
                    role="radio"
                    aria-checked={!custom.trim() && preset === p.minor}
                    className="cb-term-card cb-amount"
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
            <label className="cb-field">
              <span>{options.presets.length > 0 ? 'Другая сумма' : 'Сумма пополнения'}</span>
              <input
                value={custom}
                onChange={(e) => setCustom(e.target.value)}
                inputMode={currency === 'RUB' ? 'numeric' : 'decimal'}
                placeholder={currency === 'RUB' ? 'Например, 700' : 'Например, 15'}
                aria-invalid={amountError ? true : undefined}
                aria-describedby="topup-hint"
                maxLength={12}
              />
            </label>
            <p id="topup-hint" className={amountError ? 'cb-error cb-hint' : 'cb-small cb-muted cb-hint'}>
              {amountError ?? `Можно ${limitsText(options)}`}
            </p>
          </Card>

          <Card className="cb-methods" label="Способ оплаты">
            <h2 className="cb-h2">Способ оплаты</h2>
            {methods.length === 0 ? <p className="cb-muted">Способов пополнения пока нет. Напишите в поддержку — подскажем, как пополнить баланс.</p> : null}
            <div role="radiogroup" aria-label="Способ оплаты" className="cb-method-grid">
              {methods.map((m) => (
                <button key={m.provider} type="button" role="radio" aria-checked={chosen?.provider === m.provider} className="cb-method" onClick={() => setMethod(m.provider)}>
                  <span className="cb-radio" aria-hidden="true" />
                  <span className="cb-method-text">
                    <strong>{m.title}</strong>
                    {m.hint ? <span>{m.hint}</span> : null}
                  </span>
                </button>
              ))}
            </div>
          </Card>
        </div>

        <div className="cb-col right">
          <Card className="cb-total" label="Итого">
            <h2 className="cb-h2">Итого</h2>
            <div className="cb-line">
              <span>Пополнение баланса</span>
              <span className="num">{sum ?? ''}</span>
            </div>
            <div className="cb-due">
              <strong>К оплате</strong>
              <span className="cb-due-sum">{sum ?? ''}</span>
            </div>
            {error ? (
              <p className="cb-error" role="alert">
                {error}
              </p>
            ) : null}
            <button type="button" className="cb-main big" onClick={() => void pay()} disabled={busy || !sum || !chosen}>
              {busy ? 'Открываем оплату…' : sum ? `Пополнить на ${sum}` : 'Пополнить'}
            </button>
            {after ? <p className="cb-small cb-muted">После оплаты на балансе будет {after}.</p> : null}
          </Card>
          <Link to="/cabinet/payments" className="cb-card cb-help hov">
            <span>
              <strong>История оплат</strong>
              <span className="cb-small cb-muted">Оплаты, пополнения и возвраты</span>
            </span>
            <Icon name="chevron-right" size={20} />
          </Link>
        </div>
      </div>
    </div>
  );
}
