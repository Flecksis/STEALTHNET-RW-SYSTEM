import { useEffect, useMemo, useRef, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { count, money, MINUS, type Money } from '../lib/format';
import { gb, resetPlan, stars, type Catalog, type Quote, type TariffOffer } from '../cabinet/model';
import { BackHead, useMini } from './MiniApp';
import { openExternal } from './telegram';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function termName(months: number, daysCount: number): string {
  return daysCount >= 28 ? count(months, 'месяц', 'месяца', 'месяцев') : count(daysCount, 'день', 'дня', 'дней');
}

export function MiniPlans() {
  const { me, view, reload } = useMini();
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [cat, setCat] = useState<number | null>(null);
  const [period, setPeriod] = useState<number | null>(null);
  const [devices, setDevices] = useState(0);
  const [promoOpen, setPromoOpen] = useState(false);
  const [promoInput, setPromoInput] = useState('');
  const [promo, setPromo] = useState<string | null>(null);
  const [promoError, setPromoError] = useState<string | null>(null);
  const [quote, setQuote] = useState<Quote | null>(null);
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const seq = useRef(0);

  useEffect(() => {
    api<Catalog>('/api/client/catalog')
      .then(setCatalog)
      .catch((err) => setError(failText(err, 'Тарифы не загрузились')));
  }, []);

  const offers = catalog?.tariffs ?? [];
  const categories = useMemo(() => {
    const seen = new Map<number, { id: number; name: string; sort: number }>();
    for (const o of offers) {
      seen.set(o.category.id, o.category);
    }
    return [...seen.values()].sort((a, b) => a.sort - b.sort);
  }, [offers]);
  const current = view ? offers.find((o) => o.tariff.id === view.subscription.tariff_id) ?? null : null;

  useEffect(() => {
    if (!catalog || cat !== null) {
      return;
    }
    const startOffer = current ?? offers[0] ?? null;
    if (!startOffer) {
      return;
    }
    setCat(startOffer.category.id);
    setPeriod(startOffer.periods[Math.min(1, startOffer.periods.length - 1)]?.period_id ?? null);
    setDevices(startOffer.tariff.devices.included + (startOffer === current && view ? view.subscription.extra_devices : 0));
  }, [catalog, cat, current, offers, view]);

  const offer: TariffOffer | null = offers.find((o) => o.category.id === cat) ?? null;
  const included = offer?.tariff.devices.included ?? 0;
  const extra = offer?.tariff.devices.extra ?? null;
  const maxDevices = included + (extra?.max ?? 0);
  const periodOffer = offer?.periods.find((p) => p.period_id === period) ?? null;
  const subscriptionId = view && !view.subscription.ended_at ? view.subscription.id : null;

  useEffect(() => {
    if (!offer || !periodOffer) {
      return;
    }
    const mine = ++seq.current;
    const timer = window.setTimeout(async () => {
      try {
        const q = await api<Quote>('/api/client/quote', {
          method: 'POST',
          body: { tariff_id: offer.tariff.id, period_id: periodOffer.period_id, extra_devices: Math.max(0, devices - included), subscription_id: subscriptionId, promo_code: promo },
        });
        if (mine === seq.current) {
          setQuote(q);
          setPromoError(null);
        }
      } catch (err) {
        if (mine !== seq.current) {
          return;
        }
        if (promo) {
          setPromoError(failText(err, 'Промокод не подошёл'));
          setPromo(null);
          return;
        }
        setError(failText(err, 'Сумма не посчиталась'));
      }
    }, 150);
    return () => window.clearTimeout(timer);
  }, [offer, periodOffer, devices, included, subscriptionId, promo]);

  const methods = quote?.methods ?? [];
  const chosen = methods.find((m) => m.provider === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
  const starsMode = chosen?.kind === 'stars';
  const balanceShort = methods.find((m) => m.kind === 'balance' && m.unavailable?.type === 'insufficient_balance');
  const q = quote?.quote ?? null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? money(q.total) : '';
  const change = !!current && !!offer && current.category.id !== offer.category.id;
  const limitBytes = offer?.tariff.traffic.limit_bytes ?? 0;
  const extraMonthly: Money | null = extra && catalog ? { minor: catalog.currency === 'RUB' ? (extra.monthly.rub ?? 0) : (extra.monthly.usd ?? 0), currency: catalog.currency } : null;

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
        },
      });
      if (res.type === 'redirect') {
        openExternal(res.pay_url);
        setNote('Оплата открылась в новом окне. Когда она пройдёт, подписка обновится сама.');
        return;
      }
      setNote('Оплачено. Обновляем подписку…');
      window.setTimeout(() => void reload(), 2000);
    } catch (err) {
      setError(failText(err, 'Оплата не началась'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="mn-page plans">
      <BackHead title={view ? 'Продление' : 'Тарифы'} />
      {!catalog ? <p className="mn-muted">{error ?? 'Загружаем тарифы…'}</p> : null}
      {catalog && offers.length === 0 ? <p className="mn-muted">Тарифов пока нет.</p> : null}
      {categories.length > 1 ? (
        <div role="group" aria-label="Тариф" className="mn-seg" style={{ gridTemplateColumns: `repeat(${categories.length}, minmax(0, 1fr))` }}>
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
      {change || limitBytes ? (
        <div className="mn-notes">
          {change && offer ? (
            <span>
              Смена тарифа: {offer.tariff.name} начнётся с даты оплаты, оставшиеся дни тарифа {current?.tariff.name} не переносятся
            </span>
          ) : null}
          {limitBytes ? (
            <span>
              Трафик: {gb(limitBytes)} ГБ {resetPlan(offer?.tariff.traffic.reset ?? 'no_reset')}
            </span>
          ) : null}
        </div>
      ) : null}
      {offer ? (
        <div role="radiogroup" aria-label="Срок" className="mn-terms">
          {offer.periods.map((p) => {
            const save = starsMode ? p.stars_savings_percent : p.savings_percent;
            return (
              <button key={p.period_id} type="button" role="radio" aria-checked={period === p.period_id} className="mn-term press" onClick={() => setPeriod(p.period_id)}>
                <span className="mn-dot" aria-hidden="true" />
                <span className="mn-term-text">
                  <span className="mn-term-top">
                    <strong>{termName(p.months, p.days)}</strong>
                    <strong className="num">{starsMode && p.stars ? stars(p.stars) : money(p.price)}</strong>
                  </span>
                  <span className="mn-term-sub">
                    {starsMode && p.stars_per_month ? `${stars(p.stars_per_month)} в месяц` : `${money(p.per_month)} в месяц`}
                    {save ? <span className="mn-save"> · выгоднее на {save}%</span> : null}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      ) : null}
      {extra && extra.max > 0 ? (
        <div className="mn-card mn-devs">
          <span className="mn-devs-text">
            <strong>Устройства</strong>
            <span>
              {included} в тарифе, ещё до {extra.max}{' '}
              <span style={{ whiteSpace: 'nowrap' }}>
                по {starsMode && extra.monthly_stars ? stars(extra.monthly_stars) : extraMonthly ? money(extraMonthly) : ''} в месяц
              </span>
            </span>
          </span>
          <span className="mn-stepper">
            <button type="button" className="mn-circ press" aria-label="Меньше устройств" aria-disabled={devices <= included} onClick={() => devices > included && setDevices(devices - 1)}>
              {MINUS}
            </button>
            <span aria-live="polite">{devices}</span>
            <button type="button" className="mn-circ press" aria-label="Больше устройств" aria-disabled={devices >= maxDevices} onClick={() => devices < maxDevices && setDevices(devices + 1)}>
              +
            </button>
          </span>
        </div>
      ) : null}
      <div className="mn-pay">
        <div className="mn-pay-head">
          <h2>Оплата</h2>
          <button type="button" className="mn-link" aria-expanded={promoOpen} onClick={() => setPromoOpen(!promoOpen)}>
            {promo ? `Промокод ${promo}` : 'Есть промокод?'}
          </button>
        </div>
        {promoOpen ? (
          <form
            className="mn-promo"
            onSubmit={(e) => {
              e.preventDefault();
              if (promo) {
                setPromo(null);
                setPromoInput('');
                return;
              }
              const code = promoInput.trim().toUpperCase();
              if (code) {
                setPromo(code);
              }
            }}
          >
            <input value={promoInput} onChange={(e) => setPromoInput(e.target.value.toUpperCase())} placeholder="Промокод" aria-label="Промокод" disabled={!!promo} />
            <button type="submit" className="mn-btn2 press">
              {promo ? 'Убрать' : 'Применить'}
            </button>
          </form>
        ) : null}
        {promoError ? <p className="mn-error">{promoError}</p> : null}
        {q && (starsMode ? (q.stars?.discount ?? 0) > 0 : q.discount.minor > 0) ? (
          <p className="mn-muted small">
            Скидка по промокоду: {MINUS}
            {starsMode && q.stars ? stars(q.stars.discount) : money(q.discount)}
          </p>
        ) : null}
        <div role="radiogroup" aria-label="Способ оплаты" className="mn-chips">
          {methods.map((m) => (
            <button key={m.provider} type="button" role="radio" aria-checked={chosen?.provider === m.provider} className="mn-chip press" disabled={!!m.unavailable} onClick={() => setMethod(m.provider)}>
              {m.title}
            </button>
          ))}
        </div>
        {balanceShort?.unavailable?.type === 'insufficient_balance' ? <p className="mn-muted small">Не хватает: на балансе {money(me.balance)}</p> : null}
        {error && catalog ? <p className="mn-error">{error}</p> : null}
        {note ? (
          <p role="status" className="mn-note">
            {note}
          </p>
        ) : null}
      </div>
      <button type="button" className="mn-main mn-paybtn press" onClick={() => void pay()} disabled={busy || !chosen || !q}>
        {busy ? 'Открываем оплату…' : `Оплатить ${totalText}`}
      </button>
    </main>
  );
}
