import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { count, dateLong, money, MINUS, type Money } from '../lib/format';
import { t } from '../lib/i18n';
import { supportTickets } from '../lib/support';
import { Card, useCabinet } from './CabinetApp';
import { gbText, ladderText, linesOf, resetPlan, stars, transitionText, type Catalog, type CheckoutMethod, type Quote, type QuoteLineView, type TariffOffer } from './model';
import { CABINET_SUB_KEY, useSubs } from './SubSwitch';
import { fullCategory, fullNote, NEWEST, newTargetNote, payTargets, remember, resolveTarget, subName, subState, targetView, type PayTarget } from './subs';
import { methodKey } from '../lib/methods';

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
  const subs = useSubs();
  const multi = me.multi_subscriptions === true;
  const [target, setTarget] = useState<PayTarget | null>(null);
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
      .catch((err) => setError(failText(err, t('Тарифы не загрузились'))));
  }, []);

  const list = subs.list ?? [];
  const offers = catalog?.tariffs ?? [];
  const picked = offers.find((o) => o.category.id === cat) ?? null;
  const full = useMemo(() => fullCategory(list, multi, picked?.category), [list, multi, picked]);
  const targets = useMemo(() => payTargets(list, multi, full), [list, multi, full]);
  const asked: PayTarget | null = params.get('sub') === 'new' ? 'new' : (subs.chosen?.subscription.id ?? null);

  useEffect(() => {
    if (subs.list !== undefined && target === null) {
      setTarget(resolveTarget(targets, subs.list, multi, asked));
    }
  }, [subs.list, target, targets, multi, asked]);

  const live = subs.list === undefined || target === null ? undefined : targetView(list, target, multi);

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

  const offer: TariffOffer | null = picked;
  const included = offer?.tariff.devices.included ?? 0;
  const extra = offer?.tariff.devices.extra ?? null;
  const maxDevices = included + (extra?.max ?? 0);
  const periodOffer = offer?.periods.find((p) => p.period_id === period) ?? null;
  const subscriptionId = typeof target === 'number' ? target : null;

  const chooseTarget = (next: PayTarget) => {
    if (next === target) {
      return;
    }
    setTarget(next);
    const view = targetView(list, next, multi);
    const own = view ? offers.find((o) => o.tariff.id === view.subscription.tariff_id) : undefined;
    if (view && own) {
      setCat(own.category.id);
      const p = own.periods.find((x) => x.days === view.subscription.period_days) ?? own.periods.find((x) => x.months === periodOffer?.months) ?? own.periods[Math.min(1, own.periods.length - 1)];
      setPeriod(p?.period_id ?? null);
      setDevices(own.tariff.devices.included + view.subscription.extra_devices);
    } else {
      setDevices(included);
    }
    if (typeof next === 'number') {
      remember(CABINET_SUB_KEY, next);
    }
  };

  const keptId = subs.chosen?.subscription.id ?? null;
  useEffect(() => {
    if (!full || target === null || full.some((v) => v.subscription.id === target)) {
      return;
    }
    const next = full.find((v) => v.subscription.id === keptId) ?? full[0];
    if (!next) {
      return;
    }
    setTarget(next.subscription.id);
    if (offer && next.subscription.tariff_id === offer.tariff.id) {
      setDevices(offer.tariff.devices.included + next.subscription.extra_devices);
    }
  }, [full, target, keptId, offer]);

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
        setQuoteError(failText(err, t('Сумма не посчиталась')));
      }
    }, 150);
    return () => window.clearTimeout(timer);
  }, [offer, periodOffer, devices, included, subscriptionId, promo]);

  const methods = quote?.methods ?? [];
  const chosen = methods.find((m) => methodKey(m) === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
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
    return <p className="cb-muted cb-pad">{t('Загружаем тарифы…')}</p>;
  }
  if (offers.length === 0) {
    return <p className="cb-muted cb-pad">{t('Тарифов пока нет: оператор ещё не открыл продажи.')}</p>;
  }

  const blocked = me.client.status === 'blocked' || live?.status === 'blocked';
  const fromTrial = !!live && live.subscription.kind === 'trial';
  const sameCategory = current && offer ? current.category.id === offer.category.id : null;
  const change = !!live && !!offer && (fromTrial || (sameCategory === null ? quote?.kind === 'change' : !sameCategory));
  const transition = change && !fromTrial;
  const expired = live?.status === 'expired';
  const expire = live?.subscription.mirror.expire_at ?? null;
  const another = !live && list.length > 0;
  const title = !live ? (another ? t('Новая подписка') : t('Покупка подписки')) : fromTrial ? t('Оплата подписки') : transition ? t('Переход на другой тариф') : t('Продление подписки');
  const currentName = live && list.length > 1 ? subName(live, list) : (current?.tariff.name ?? live?.tariff?.name ?? null);
  const nextName = offer?.tariff.name ?? t('новый тариф');
  const lead: { text: ReactNode; tone: string } = blocked
    ? { text: t('Подписка заблокирована, оплата недоступна. Напишите в поддержку.'), tone: 'bad' }
    : !live
      ? {
          text: another ? t('Подписка начнёт работать сразу после оплаты, у неё будет своя ссылка. Текущие подписки останутся как есть.') : t('Подписка начнёт работать сразу после оплаты.'),
          tone: 'muted',
        }
      : fromTrial
        ? { text: t('Пробный период закончится сразу после оплаты, тариф «{name}» начнёт действовать с даты оплаты.', { name: nextName }), tone: 'muted' }
        : transition
          ? {
              text: (
                <>
                  <strong>{t('Будет переход на тариф «{name}».', { name: nextName })}</strong> {transitionText(currentName)}
                </>
              ),
              tone: 'warn',
            }
          : expired
            ? { text: t('Подписка закончилась {date}. Новый срок начнётся с даты оплаты.', { date: expire ? dateLong(expire, false) : '' }), tone: 'muted' }
            : {
                text: expire
                  ? t('Сейчас: {name} до {date}. Новый срок добавится к текущему.', { name: currentName ?? t('текущий тариф'), date: dateLong(expire, true) })
                  : t('Сейчас: {name}. Новый срок добавится к текущему.', { name: currentName ?? t('текущий тариф') }),
                tone: 'muted',
              };

  const limitNote = fullNote(full, offer?.category);
  const limit = offer?.tariff.traffic.limit_bytes ?? 0;
  const trafficNote = limit ? t('Трафик: {traffic} {plan}.', { traffic: gbText(limit), plan: resetPlan(offer?.tariff.traffic.reset ?? 'no_reset') }) : null;

  const priceOf = (m: Money) => money(m);
  const extraMonthly: Money | null = extra ? { minor: catalog.currency === 'RUB' ? (extra.monthly.rub ?? 0) : (extra.monthly.usd ?? 0), currency: catalog.currency } : null;

  const q = quote?.quote ?? null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? priceOf(q.total) : '';
  const deviceCut = q ? (linesOf(q.lines, 'device_discount')[0] ?? null) : null;
  const ladder = ladderText(offer?.tariff.device_discounts);
  const cutText = (line: QuoteLineView) => `${MINUS}${starsMode && line.stars !== null ? stars(Math.abs(line.stars)) : priceOf({ minor: Math.abs(line.amount.minor), currency: line.amount.currency })}`;
  const after =
    quote?.expire_preview && !blocked
      ? change || !live
        ? t('{name} будет действовать до {date}.', { name: offer?.tariff.name ?? t('Подписка'), date: dateLong(quote.expire_preview, true) })
        : t('Подписка продлится до {date}.', { date: dateLong(quote.expire_preview, true) })
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
          method: chosen.method ?? undefined,
          return_url: `${window.location.origin}/cabinet/pay`,
        },
      });
      if (another) {
        remember(CABINET_SUB_KEY, NEWEST);
      }
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
      setError(failText(err, t('Оплата не началась')));
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
        <Card className="cb-wait" label={t('Оплата')}>
          <h1 className="cb-h1">{done ? t('Оплата прошла') : review ? t('Оплата принята') : failed ? t('Оплата не прошла') : t('Ждём оплату')}</h1>
          <p className="cb-ink2">
            {done
              ? t('Подписка обновлена.')
              : review
                ? t('Проверяем платёж, подписка обновится в ближайшее время.')
                : failed
                  ? t('Деньги не списаны или возвращены. Попробуйте ещё раз или выберите другой способ.')
                  : t('Как только платёжная система подтвердит оплату, подписка обновится сама.')}
          </p>
          <div className="cb-btns">
            <Link to="/cabinet" className="cb-main">
              {t('В кабинет')}
            </Link>
            {!done && !review && waiting.pay_url && !failed ? (
              <a className="cb-second" href={waiting.pay_url}>
                {t('Вернуться к оплате')}
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
                {t('Выбрать заново')}
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
        {limitNote ? <p className="cb-muted">{limitNote}</p> : null}
        {trafficNote ? <p className="cb-muted">{trafficNote}</p> : null}
      </header>
      <div className="cb-cols">
        <div className="cb-col left">
          {targets.length > 1 ? (
            <Card className="cb-pick" label={t('Что оплачиваем')}>
              <h2 className="cb-h2">{t('Что оплачиваем')}</h2>
              <div role="radiogroup" aria-label={t('Что оплачиваем')} className="cb-method-grid">
                {targets.map((item) => {
                  const view = typeof item === 'number' ? targetView(list, item, multi) : null;
                  const state = view ? subState(view, me.client.status === 'blocked') : null;
                  return (
                    <button key={String(item)} type="button" role="radio" aria-checked={target === item} className="cb-method cb-target" onClick={() => chooseTarget(item)}>
                      <span className="cb-radio" aria-hidden="true" />
                      <span className="cb-method-text">
                        <strong>{view ? t('Продлить «{name}»', { name: subName(view, list) }) : t('Новая подписка')}</strong>
                        <span className={`cb-target-note ${state?.tone ?? 'muted'}`}>{state ? state.text : newTargetNote(list, multi)}</span>
                      </span>
                    </button>
                  );
                })}
              </div>
            </Card>
          ) : null}
          <Card className="cb-pick" label={t('Тариф и срок')}>
            <div className="cb-row wrap">
              <h2 className="cb-h2">{t('Тариф и срок')}</h2>
              {categories.length > 1 ? (
                <div role="group" aria-label={t('Тариф')} className="cb-cats">
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
            <div role="radiogroup" aria-label={t('Срок')} className={starsMode ? 'cb-terms wide' : 'cb-terms'}>
              {offer?.periods.map((p) => {
                const save = starsMode ? p.stars_savings_percent : p.savings_percent;
                return (
                  <button key={p.period_id} type="button" role="radio" aria-checked={period === p.period_id} className="cb-term-card" onClick={() => setPeriod(p.period_id)}>
                    <span className="cb-term-name">{monthsLabel(p.months, p.days)}</span>
                    <span className="cb-term-price">{starsMode && p.stars ? stars(p.stars) : priceOf(p.price)}</span>
                    {save ? (
                      <span className="cb-term-save">
                        <span className="cb-muted">{t('{price} в месяц', { price: starsMode && p.stars_per_month ? stars(p.stars_per_month) : priceOf(p.per_month) })}</span>
                        <strong>{t('Выгоднее на {percent}%', { percent: save })}</strong>
                      </span>
                    ) : null}
                  </button>
                );
              })}
            </div>
          </Card>

          {extra && extra.max > 0 ? (
            <Card className="cb-devs" label={t('Устройства')}>
              <span className="cb-devs-text">
                <h2 className="cb-h2">{t('Устройства')}</h2>
                <span className="cb-muted">
                  {t('В тариф входят {included}, ещё {max} можно докупить по {price} в месяц.', {
                    included,
                    max: extra.max,
                    price: starsMode && extra.monthly_stars ? stars(extra.monthly_stars) : extraMonthly ? priceOf(extraMonthly) : '',
                  })}
                </span>
                {ladder ? <span className="cb-muted">{ladder}</span> : null}
                {deviceCut ? (
                  <span className="cb-devs-cut" aria-live="polite">
                    {deviceCut.label}
                  </span>
                ) : null}
              </span>
              <span className="cb-stepper">
                <button type="button" aria-label={t('Меньше устройств')} aria-disabled={devices <= included} onClick={() => devices > included && setDevices(devices - 1)}>
                  {MINUS}
                </button>
                <span aria-live="polite">{devices}</span>
                <button type="button" aria-label={t('Больше устройств')} aria-disabled={devices >= maxDevices} onClick={() => devices < maxDevices && setDevices(devices + 1)}>
                  +
                </button>
              </span>
            </Card>
          ) : null}

          <Card className="cb-methods" label={t('Способ оплаты')}>
            <h2 className="cb-h2">{t('Способ оплаты')}</h2>
            {methods.length === 0 && quote ? <p className="cb-muted">{t('Здесь пока нет способов оплаты. Напишите в поддержку — подскажем, как оплатить.')}</p> : null}
            <div role="radiogroup" aria-label={t('Способ оплаты')} className="cb-method-grid">
              {methods.map((m) => {
                const off = !!m.unavailable;
                const sel = chosen === m;
                const hint =
                  m.unavailable?.type === 'insufficient_balance'
                    ? t('Не хватает: на балансе {amount}', { amount: priceOf(m.unavailable.balance) })
                    : m.unavailable?.type === 'no_stars_price'
                      ? t('У этого срока нет цены в звёздах')
                      : m.kind === 'balance'
                        ? t('На балансе {amount}', { amount: priceOf(me.balance) })
                        : m.kind === 'stars' && methodAmount(m) !== null
                          ? t('{stars}, оплата в Telegram', { stars: stars(methodAmount(m) ?? 0) })
                          : m.hint;
                return (
                  <button
                    key={methodKey(m)}
                    type="button"
                    role="radio"
                    aria-checked={sel}
                    aria-disabled={off ? true : undefined}
                    className={off ? 'cb-method off' : 'cb-method'}
                    onClick={() => !off && setMethod(methodKey(m))}
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
          <Card className="cb-total" label={t('Итого')}>
            <h2 className="cb-h2">{t('Итого')}</h2>
            {q && offer && periodOffer
              ? (() => {
                  const periodLine = q.lines.find((l) => l.kind.type === 'period') ?? null;
                  const extraLines = linesOf(q.lines, 'extra_devices');
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
                          <span>{t('Доп. устройства: {n}', { n: Math.max(0, devices - included) })}</span>
                          <span className="num">{starsMode ? stars(extraStars) : priceOf({ minor: extraMinor, currency: q.currency })}</span>
                        </div>
                      ) : null}
                    </>
                  );
                })()
              : null}
            {q
              ? [...linesOf(q.lines, 'device_discount'), ...linesOf(q.lines, 'adjustment')]
                  .filter((line) => (starsMode ? (line.stars ?? 0) !== 0 : line.amount.minor !== 0))
                  .map((line) => (
                    <div key={`${line.kind.type}:${line.label}`} className="cb-line">
                      <span>{line.label}</span>
                      <span className="num">{cutText(line)}</span>
                    </div>
                  ))
              : null}
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
                <span className="sr-only">{t('Промокод')}</span>
                <input value={promoInput} onChange={(e) => setPromoInput(e.target.value.toUpperCase())} placeholder={t('Промокод')} maxLength={40} disabled={!!promo} />
              </label>
              <button type="submit" className="cb-second">
                {promo ? t('Убрать') : t('Применить')}
              </button>
            </form>
            {promoError ? <p className="cb-error">{promoError}</p> : null}
            <div className="cb-due">
              <strong>{t('К оплате')}</strong>
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
              supportTickets(site) ? (
                <Link className="cb-main big" to="/cabinet/help">
                  {t('Написать в поддержку')}
                </Link>
              ) : site.support_url ? (
                <a className="cb-main big" href={site.support_url} target="_blank" rel="noreferrer">
                  {t('Написать в поддержку')}
                </a>
              ) : null
            ) : (
              <button type="button" className="cb-main big" onClick={() => void pay()} disabled={busy || !chosen || !q}>
                {busy ? t('Открываем оплату…') : transition ? t('Перейти и оплатить {amount}', { amount: totalText }) : t('Оплатить {amount}', { amount: totalText })}
              </button>
            )}
            {after ? <p className="cb-small cb-muted">{after}</p> : null}
          </Card>
        </div>
      </div>
    </div>
  );
}
