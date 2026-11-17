import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { count, dateLong, money, MINUS, type Money } from '../lib/format';
import { Card, useCabinet } from './CabinetApp';
import { gb, resetPlan, stars, type Catalog, type CheckoutMethod, type Quote, type SubscriptionView, type TariffOffer } from './model';

type Payment = { payment_id: string; status: string; order_status: string; pay_url: string | null };

const PAYMENT_KEY = 'sn.cabinet.payment';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function monthsLabel(months: number, daysCount: number): string {
  return daysCount >= 28 ? count(months, 'месяц', 'месяца', 'месяцев') : count(daysCount, 'день', 'дня', 'дней');
}

function methodAmount(method: CheckoutMethod): number | null {
  const a = method.amount;
  if (typeof a === 'number') {
    return a;
  }
  if (a && typeof a === 'object' && typeof a.stars === 'number') {
    return a.stars;
  }
  return null;
}

export function Pay() {
  const { me, site, reload } = useCabinet();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [live, setLive] = useState<SubscriptionView | null | undefined>(undefined);
  const [cat, setCat] = useState<number | null>(null);
  const [period, setPeriod] = useState<number | null>(null);
  const [devices, setDevices] = useState<number>(0);
  const [promoInput, setPromoInput] = useState('');
  const [promo, setPromo] = useState<string | null>(null);
  const [promoError, setPromoError] = useState<string | null>(null);
  const [quote, setQuote] = useState<Quote | null>(null);
  const [quoteError, setQuoteError] = useState<string | null>(null);
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [waiting, setWaiting] = useState<Payment | null>(null);
  const seq = useRef(0);

  useEffect(() => {
    api<Catalog>('/api/client/catalog')
      .then(setCatalog)
      .catch((err) => setError(failText(err, 'Тарифы не загрузились')));
    api<SubscriptionView[]>('/api/client/subscriptions')
      .then((list) => setLive(list.find((v) => !v.subscription.ended_at) ?? null))
      .catch(() => setLive(null));
  }, []);

  const offers = catalog?.tariffs ?? [];
  const categories = useMemo(() => {
    const seen = new Map<number, { id: number; name: string; sort: number }>();
    for (const o of offers) {
      seen.set(o.category.id, o.category);
    }
    return [...seen.values()].sort((a, b) => a.sort - b.sort);
  }, [offers]);

  const current = live ? offers.find((o) => o.tariff.id === live.subscription.tariff_id) ?? null : null;

  useEffect(() => {
    if (!catalog || live === undefined || cat !== null) {
      return;
    }
    const wanted = Number(params.get('tariff'));
    const fromLink = offers.find((o) => o.tariff.id === wanted);
    const start = fromLink ?? current ?? offers[0] ?? null;
    if (!start) {
      return;
    }
    setCat(start.category.id);
    const wantedPeriod = Number(params.get('period'));
    const p =
      start.periods.find((x) => x.period_id === wantedPeriod) ??
      start.periods.find((x) => live && x.days === live.subscription.period_days && start === current) ??
      start.periods[Math.min(1, start.periods.length - 1)] ??
      null;
    setPeriod(p?.period_id ?? null);
    const included = start.tariff.devices.included;
    setDevices(start === current && live ? included + live.subscription.extra_devices : included);
  }, [catalog, live, cat, offers, current, params]);

  const offer: TariffOffer | null = offers.find((o) => o.category.id === cat) ?? null;
  const included = offer?.tariff.devices.included ?? 0;
  const extra = offer?.tariff.devices.extra ?? null;
  const maxDevices = included + (extra?.max ?? 0);
  const periodOffer = offer?.periods.find((p) => p.period_id === period) ?? null;
  const subscriptionId = live && !live.subscription.ended_at ? live.subscription.id : null;

  useEffect(() => {
    if (!offer || !periodOffer) {
      return;
    }
    const mine = ++seq.current;
    const timer = window.setTimeout(async () => {
      try {
        const q = await api<Quote>('/api/client/quote', {
          method: 'POST',
          body: {
            tariff_id: offer.tariff.id,
            period_id: periodOffer.period_id,
            extra_devices: Math.max(0, devices - included),
            subscription_id: subscriptionId,
            promo_code: promo,
          },
        });
        if (mine !== seq.current) {
          return;
        }
        setQuote(q);
        setQuoteError(null);
        setPromoError(null);
      } catch (err) {
        if (mine !== seq.current) {
          return;
        }
        if (promo && err instanceof ApiError && (err.code.startsWith('promo') || err.code === 'module.invalid')) {
          setPromoError(err.message);
          setPromo(null);
          return;
        }
        setQuoteError(failText(err, 'Сумма не посчиталась'));
      }
    }, 150);
    return () => window.clearTimeout(timer);
  }, [offer, periodOffer, devices, included, subscriptionId, promo]);

  const methods = quote?.methods ?? [];
  const chosen = methods.find((m) => m.provider === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
  const starsMode = chosen?.kind === 'stars';

  useEffect(() => {
    const id = params.get('payment') ?? (() => {
      try {
        return window.sessionStorage.getItem(PAYMENT_KEY);
      } catch {
        return null;
      }
    })();
    if (!id) {
      return;
    }
    let alive = true;
    const tick = async (attempt: number) => {
      try {
        const p = await api<Payment>(`/api/client/payments/${id}`);
        if (!alive) {
          return;
        }
        setWaiting(p);
        if (p.order_status === 'fulfilled' || p.order_status === 'review' || p.order_status === 'canceled' || p.order_status === 'refunded') {
          try {
            window.sessionStorage.removeItem(PAYMENT_KEY);
          } catch {
            return;
          }
          if (p.order_status === 'fulfilled') {
            await reload();
          }
          return;
        }
        if (attempt < 60) {
          window.setTimeout(() => void tick(attempt + 1), 3000);
        }
      } catch {
        setWaiting(null);
      }
    };
    void tick(0);
    return () => {
      alive = false;
    };
  }, [params, reload]);

  if (error && !catalog) {
    return (
      <p className="cb-error cb-pad" role="alert">
        {error}
      </p>
    );
  }
  if (!catalog || live === undefined) {
    return <p className="cb-muted cb-pad">Загружаем тарифы…</p>;
  }
  if (offers.length === 0) {
    return <p className="cb-muted cb-pad">Тарифов пока нет: оператор ещё не открыл продажи.</p>;
  }

  const blocked = me.client.status === 'blocked' || live?.status === 'blocked';
  const change = !!current && !!offer && current.category.id !== offer.category.id;
  const expired = live?.status === 'expired';
  const expire = live?.subscription.mirror.expire_at ?? null;
  const title = !live ? 'Покупка подписки' : change ? 'Смена тарифа' : 'Продление подписки';
  const currentName = current?.tariff.name ?? live?.tariff?.name ?? 'текущий';
  const lead = blocked
    ? { text: 'Подписка заблокирована, оплата недоступна. Напишите в поддержку.', tone: 'bad' }
    : !live
      ? { text: 'Подписка начнёт работать сразу после оплаты.', tone: 'muted' }
      : expired
        ? {
            text: change
              ? `Подписка закончилась ${expire ? dateLong(expire, false) : ''}. ${offer?.tariff.name ?? 'Новый тариф'} начнётся с даты оплаты.`
              : `Подписка закончилась ${expire ? dateLong(expire, false) : ''}. Новый срок начнётся с даты оплаты.`,
            tone: 'muted',
          }
        : change
          ? {
              text: `Сейчас: ${currentName}${expire ? ` до ${dateLong(expire, true)}` : ''}. ${offer?.tariff.name ?? 'Новый тариф'} начнётся с даты оплаты, оставшиеся дни тарифа ${currentName} не переносятся.`,
              tone: 'warn',
            }
          : { text: `Сейчас: ${currentName}${expire ? ` до ${dateLong(expire, true)}` : ''}. Новый срок добавится к текущему.`, tone: 'muted' };

  const limit = offer?.tariff.traffic.limit_bytes ?? 0;
  const trafficNote = limit ? `Трафик: ${gb(limit)} ГБ ${resetPlan(offer?.tariff.traffic.reset ?? 'no_reset')}.` : null;

  const priceOf = (m: Money) => money(m);
  const extraMonthly: Money | null = extra ? { minor: catalog.currency === 'RUB' ? (extra.monthly.rub ?? 0) : (extra.monthly.usd ?? 0), currency: catalog.currency } : null;

  const q = quote?.quote ?? null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? priceOf(q.total) : '';
  const after =
    quote?.expire_preview && !blocked
      ? change || !live
        ? `${offer?.tariff.name ?? 'Подписка'} будет действовать до ${dateLong(quote.expire_preview, true)}.`
        : `Подписка продлится до ${dateLong(quote.expire_preview, true)}.`
      : null;

  const pay = async () => {
    if (!offer || !periodOffer || !chosen) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await api<{ type: 'redirect'; payment_id: string; pay_url: string } | { type: 'completed'; order_id: string }>('/api/client/checkout', {
        method: 'POST',
        body: {
          cart: { tariff_id: offer.tariff.id, period_id: periodOffer.period_id, extra_devices: Math.max(0, devices - included), subscription_id: subscriptionId, promo_code: promo },
          provider: chosen.provider,
          return_url: `${window.location.origin}/cabinet/pay`,
        },
      });
      if (res.type === 'redirect') {
        try {
          window.sessionStorage.setItem(PAYMENT_KEY, res.payment_id);
        } catch {
          setParams({ payment: res.payment_id });
        }
        window.location.href = res.pay_url;
        return;
      }
      await reload();
      navigate('/cabinet');
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
      <div className="cb-stack">
        <Card className="cb-wait" label="Оплата">
          <h1 className="cb-h1">{done ? 'Оплата прошла' : review ? 'Оплата принята' : failed ? 'Оплата не прошла' : 'Ждём оплату'}</h1>
          <p className="cb-ink2">
            {done
              ? 'Подписка обновлена.'
              : review
                ? 'Проверяем платёж, подписка обновится в ближайшее время.'
                : failed
                  ? 'Деньги не списаны или возвращены. Попробуйте ещё раз или выберите другой способ.'
                  : 'Как только платёжная система подтвердит оплату, подписка обновится сама.'}
          </p>
          <div className="cb-btns">
            <Link to="/cabinet" className="cb-main">
              В кабинет
            </Link>
            {!done && !review && waiting.pay_url && !failed ? (
              <a className="cb-second" href={waiting.pay_url}>
                Вернуться к оплате
              </a>
            ) : null}
            {failed ? (
              <button
                type="button"
                className="cb-second"
                onClick={() => {
                  setWaiting(null);
                  setParams({});
                }}
              >
                Выбрать заново
              </button>
            ) : null}
          </div>
        </Card>
      </div>
    );
  }

  return (
    <div className="cb-pay">
      <header className="cb-pay-head">
        <h1 className="cb-title">{title}</h1>
        <p className={`cb-lead ${lead.tone}`}>{lead.text}</p>
        {trafficNote ? <p className="cb-muted">{trafficNote}</p> : null}
      </header>
      <div className="cb-cols">
        <div className="cb-col left">
          <Card className="cb-pick" label="Тариф и срок">
            <div className="cb-row wrap">
              <h2 className="cb-h2">Тариф и срок</h2>
              {categories.length > 1 ? (
                <div role="group" aria-label="Тариф" className="cb-cats">
                  {categories.map((c) => (
                    <button
                      key={c.id}
                      type="button"
                      aria-pressed={cat === c.id}
                      onClick={() => {
                        const next = offers.find((o) => o.category.id === c.id);
                        setCat(c.id);
                        if (next) {
                          const keep = next.periods.find((p) => p.months === periodOffer?.months) ?? next.periods[Math.min(1, next.periods.length - 1)];
                          setPeriod(keep?.period_id ?? null);
                          setDevices(next.tariff.devices.included);
                        }
                      }}
                    >
                      {c.name}
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
            <div role="radiogroup" aria-label="Срок" className={starsMode ? 'cb-terms wide' : 'cb-terms'}>
              {offer?.periods.map((p) => {
                const save = starsMode ? p.stars_savings_percent : p.savings_percent;
                return (
                  <button key={p.period_id} type="button" role="radio" aria-checked={period === p.period_id} className="cb-term-card" onClick={() => setPeriod(p.period_id)}>
                    <span className="cb-term-name">{monthsLabel(p.months, p.days)}</span>
                    <span className="cb-term-price">{starsMode && p.stars ? stars(p.stars) : priceOf(p.price)}</span>
                    {save ? (
                      <span className="cb-term-save">
                        <span className="cb-muted">{starsMode && p.stars_per_month ? `${stars(p.stars_per_month)} в месяц` : `${priceOf(p.per_month)} в месяц`}</span>
                        <strong>Выгоднее на {save}%</strong>
                      </span>
                    ) : null}
                  </button>
                );
              })}
            </div>
          </Card>

          {extra && extra.max > 0 ? (
            <Card className="cb-devs" label="Устройства">
              <span className="cb-devs-text">
                <h2 className="cb-h2">Устройства</h2>
                <span className="cb-muted">
                  В тариф входят {included}, ещё {extra.max} можно докупить по{' '}
                  {starsMode && extra.monthly_stars ? stars(extra.monthly_stars) : extraMonthly ? priceOf(extraMonthly) : ''} в месяц.
                </span>
              </span>
              <span className="cb-stepper">
                <button type="button" aria-label="Меньше устройств" aria-disabled={devices <= included} onClick={() => devices > included && setDevices(devices - 1)}>
                  {MINUS}
                </button>
                <span aria-live="polite">{devices}</span>
                <button type="button" aria-label="Больше устройств" aria-disabled={devices >= maxDevices} onClick={() => devices < maxDevices && setDevices(devices + 1)}>
                  +
                </button>
              </span>
            </Card>
          ) : null}

          <Card className="cb-methods" label="Способ оплаты">
            <h2 className="cb-h2">Способ оплаты</h2>
            {methods.length === 0 && quote ? <p className="cb-muted">Здесь пока нет способов оплаты. Напишите в поддержку — подскажем, как оплатить.</p> : null}
            <div role="radiogroup" aria-label="Способ оплаты" className="cb-method-grid">
              {methods.map((m) => {
                const off = !!m.unavailable;
                const sel = chosen?.provider === m.provider;
                const hint =
                  m.unavailable?.type === 'insufficient_balance'
                    ? `Не хватает: на балансе ${priceOf(m.unavailable.balance)}`
                    : m.unavailable?.type === 'no_stars_price'
                      ? 'У этого срока нет цены в звёздах'
                      : m.kind === 'balance'
                        ? `На балансе ${priceOf(me.balance)}`
                        : m.kind === 'stars' && methodAmount(m) !== null
                          ? `${stars(methodAmount(m) ?? 0)}, оплата в Telegram`
                          : m.hint;
                return (
                  <button
                    key={m.provider}
                    type="button"
                    role="radio"
                    aria-checked={sel}
                    aria-disabled={off ? true : undefined}
                    className={off ? 'cb-method off' : 'cb-method'}
                    onClick={() => !off && setMethod(m.provider)}
                  >
                    <span className="cb-radio" aria-hidden="true" />
                    <span className="cb-method-text">
                      <strong>{m.title}</strong>
                      <span>{hint}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          </Card>
        </div>

        <div className="cb-col right">
          <Card className="cb-total" label="Итого">
            <h2 className="cb-h2">Итого</h2>
            {q && offer && periodOffer
              ? (() => {
                  const periodLine = q.lines.find((l) => l.kind.type === 'period') ?? null;
                  const extraLines = q.lines.filter((l) => l.kind.type !== 'period');
                  const extraStars = extraLines.reduce((sum, l) => sum + (l.stars ?? 0), 0);
                  const extraMinor = extraLines.reduce((sum, l) => sum + l.amount.minor, 0);
                  return (
                    <>
                      <div className="cb-line">
                        <span>
                          {offer.tariff.name} · {monthsLabel(periodOffer.months, periodOffer.days)}
                        </span>
                        <span className="num">{periodLine ? (starsMode && periodLine.stars !== null ? stars(periodLine.stars) : priceOf(periodLine.amount)) : ''}</span>
                      </div>
                      {extra && extra.max > 0 ? (
                        <div className="cb-line">
                          <span>Доп. устройства: {Math.max(0, devices - included)}</span>
                          <span className="num">{starsMode ? stars(extraStars) : priceOf({ minor: extraMinor, currency: q.currency })}</span>
                        </div>
                      ) : null}
                    </>
                  );
                })()
              : null}
            {q && (starsMode ? (q.stars?.discount ?? 0) > 0 : q.discount.minor > 0) ? (
              <div className="cb-line">
                <span>Промокод {promo}</span>
                <span className="num">
                  {MINUS}
                  {starsMode && q.stars ? stars(q.stars.discount) : priceOf(q.discount)}
                </span>
              </div>
            ) : null}
            <form
              className="cb-promo"
              onSubmit={(e) => {
                e.preventDefault();
                if (promo) {
                  setPromo(null);
                  setPromoInput('');
                  return;
                }
                const code = promoInput.trim().toUpperCase();
                if (code) {
                  setPromoError(null);
                  setPromo(code);
                }
              }}
            >
              <label className="cb-promo-field">
                <span className="sr-only">Промокод</span>
                <input value={promoInput} onChange={(e) => setPromoInput(e.target.value.toUpperCase())} placeholder="Промокод" maxLength={40} disabled={!!promo} />
              </label>
              <button type="submit" className="cb-second">
                {promo ? 'Убрать' : 'Применить'}
              </button>
            </form>
            {promoError ? <p className="cb-error">{promoError}</p> : null}
            <div className="cb-due">
              <strong>К оплате</strong>
              <span className="cb-due-sum">{totalText}</span>
            </div>
            {quoteError ? (
              <p className="cb-error" role="alert">
                {quoteError}
              </p>
            ) : null}
            {error ? (
              <p className="cb-error" role="alert">
                {error}
              </p>
            ) : null}
            {blocked ? (
              site.support_url ? (
                <a className="cb-main big" href={site.support_url}>
                  Написать в поддержку
                </a>
              ) : null
            ) : (
              <button type="button" className="cb-main big" onClick={() => void pay()} disabled={busy || !chosen || !q}>
                {busy ? 'Открываем оплату…' : `Оплатить ${totalText}`}
              </button>
            )}
            {after ? <p className="cb-small cb-muted">{after}</p> : null}
          </Card>
        </div>
      </div>
    </div>
  );
}
