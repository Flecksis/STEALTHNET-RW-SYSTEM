import { useEffect, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, money, MINUS, traffic } from '../lib/format';
import { addonItem, daysText, doneText, firstKind, leftoverText, offerUrl, type AddonChoice, type AddonKind, type AddonsOffer, type ItemQuote, type PayResult } from './extras';
import { Card, useCabinet } from './CabinetApp';
import { stars } from './model';
import { finished, type PaymentProgress } from './payments';
import { SubSwitch, useSubs } from './SubSwitch';
import { subName } from './subs';
import { methodKey } from '../lib/methods';

const ADDON_KEY = 'sn.cabinet.addon';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function readKey(): string | null {
  try {
    return window.sessionStorage.getItem(ADDON_KEY);
  } catch {
    return null;
  }
}

function forgetKey(): void {
  try {
    window.sessionStorage.removeItem(ADDON_KEY);
  } catch {
    return;
  }
}

export function AddonsPage() {
  const { me, reload } = useCabinet();
  const [params, setParams] = useSearchParams();
  const [asked] = useState(() => params.get('kind'));
  const subs = useSubs();
  const live = subs.list === undefined ? undefined : subs.chosen;
  const [offer, setOffer] = useState<AddonsOffer | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [kind, setKind] = useState<AddonKind>('devices');
  const [count, setCount] = useState(1);
  const [pack, setPack] = useState<number | null>(null);
  const [quote, setQuote] = useState<ItemQuote | null>(null);
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [waiting, setWaiting] = useState<PaymentProgress | null>(null);
  const seq = useRef(0);

  const subscriptionId = live && live.status !== 'expired' ? live.subscription.id : null;

  useEffect(() => {
    if (subscriptionId === null) {
      return;
    }
    let alive = true;
    setOffer(null);
    setLoadError(null);
    setQuote(null);
    setCount(1);
    setError(null);
    setDone(null);
    api<AddonsOffer>(offerUrl(subscriptionId))
      .then((next) => {
        if (!alive) {
          return;
        }
        setOffer(next);
        setKind(firstKind(next, asked));
        setPack(next.traffic.offer?.packs[0]?.gb ?? null);
      })
      .catch((err) => {
        if (alive) {
          setLoadError(failText(err, 'Не удалось загрузить предложения'));
        }
      });
    return () => {
      alive = false;
    };
  }, [subscriptionId, asked]);

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

  const devices = offer?.devices.offer ?? null;
  const trafficOffer = offer?.traffic.offer ?? null;
  const choice: AddonChoice | null =
    kind === 'devices' ? (devices && devices.can_add > 0 ? { kind: 'devices', count: Math.min(count, devices.can_add) } : null) : trafficOffer && pack !== null ? { kind: 'traffic', gb: pack } : null;
  const choiceKey = choice ? JSON.stringify(choice) : '';

  useEffect(() => {
    if (subscriptionId === null || !choice) {
      setQuote(null);
      return;
    }
    const mine = ++seq.current;
    const timer = window.setTimeout(async () => {
      try {
        const next = await api<ItemQuote>('/api/client/items/quote', { method: 'POST', body: addonItem(subscriptionId, choice) });
        if (mine === seq.current) {
          setQuote(next);
          setError(null);
        }
      } catch (err) {
        if (mine === seq.current) {
          setQuote(null);
          setError(failText(err, 'Сумма не посчиталась'));
        }
      }
    }, 150);
    return () => window.clearTimeout(timer);
  }, [subscriptionId, choiceKey]);

  const methods = quote?.methods ?? [];
  const chosen = methods.find((m) => methodKey(m) === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
  const starsMode = chosen?.kind === 'stars';
  const q = quote?.quote ?? null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? money(q.total) : '';

  const pay = async () => {
    if (subscriptionId === null || !choice || !chosen) {
      return;
    }
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const res = await api<PayResult>('/api/client/items/checkout', {
        method: 'POST',
        body: { item: addonItem(subscriptionId, choice), provider: chosen.provider, method: chosen.method ?? undefined, return_url: `${window.location.origin}/cabinet/addons` },
      });
      if (res.type === 'redirect') {
        try {
          window.sessionStorage.setItem(ADDON_KEY, res.payment_id);
        } catch {
          setParams({ payment: res.payment_id });
        }
        window.location.href = res.pay_url;
        return;
      }
      setDone(doneText(choice));
      await reload();
      const next = await api<AddonsOffer>(offerUrl(subscriptionId));
      setOffer(next);
      setCount(1);
    } catch (err) {
      setError(failText(err, 'Оплата не началась'));
    } finally {
      setBusy(false);
    }
  };

  if (waiting) {
    const ok = waiting.order_status === 'fulfilled';
    const review = waiting.order_status === 'review';
    const failed = waiting.order_status === 'canceled' || waiting.order_status === 'refunded';
    return (
      <div className="cb-stack cb-narrow">
        <Card className="cb-wait" label="Докупка">
          <h1 className="cb-h1">{ok ? 'Оплата прошла' : review ? 'Оплата принята' : failed ? 'Оплата не прошла' : 'Ждём оплату'}</h1>
          <p className="cb-ink2">
            {ok
              ? 'Лимит подписки обновлён.'
              : review
                ? 'Проверяем платёж, лимит обновится в ближайшее время.'
                : failed
                  ? 'Деньги не списаны или возвращены. Попробуйте ещё раз или выберите другой способ.'
                  : 'Как только платёжная система подтвердит оплату, лимит обновится сам.'}
          </p>
          <div className="cb-btns">
            <Link to="/cabinet" className="cb-main">
              В кабинет
            </Link>
            {!ok && !review && !failed && waiting.pay_url ? (
              <a className="cb-second" href={waiting.pay_url}>
                Вернуться к оплате
              </a>
            ) : (
              <button
                type="button"
                className="cb-second"
                onClick={() => {
                  forgetKey();
                  setWaiting(null);
                  setParams({});
                }}
              >
                Докупить ещё
              </button>
            )}
          </div>
        </Card>
      </div>
    );
  }

  if (live === undefined) {
    return <p className="cb-muted cb-pad">Загружаем…</p>;
  }

  const list = subs.list ?? [];
  const switcher = <SubSwitch subs={subs} />;

  if (subscriptionId !== null && !offer && !loadError) {
    return (
      <div className="cb-stack cb-narrow">
        {switcher}
        <p className="cb-muted cb-pad">Загружаем…</p>
      </div>
    );
  }

  if (live === null || subscriptionId === null) {
    return (
      <div className="cb-stack cb-narrow">
        {switcher}
        <Card label="Докупка">
          <h1 className="cb-h1">Нужна действующая подписка</h1>
          <p className="cb-ink2">Устройства и трафик докупаются к подписке, пока она действует.</p>
          <div className="cb-btns">
            <Link to={subs.link('/cabinet/pay')} className="cb-main">
              К тарифам
            </Link>
          </div>
        </Card>
      </div>
    );
  }

  if (loadError || !offer) {
    return (
      <div className="cb-stack cb-narrow">
        {switcher}
        <p className="cb-error cb-pad" role="alert">
          {loadError ?? 'Не удалось загрузить предложения'}
        </p>
      </div>
    );
  }

  if (!devices && !trafficOffer) {
    return (
      <div className="cb-stack cb-narrow">
        {switcher}
        <Card label="Докупка">
          <h1 className="cb-h1">Докупить нечего</h1>
          <p className="cb-ink2">{offer.devices.unavailable?.message ?? offer.traffic.unavailable?.message ?? 'Для этой подписки докупить нечего.'}</p>
          <div className="cb-btns">
            <Link to="/cabinet" className="cb-main">
              В кабинет
            </Link>
          </div>
        </Card>
      </div>
    );
  }

  const expire = devices?.expire_at ?? trafficOffer?.expire_at ?? null;

  return (
    <div className="cb-pay">
      <header className="cb-pay-head">
        <h1 className="cb-title">Докупить к подписке</h1>
        <p className="cb-lead muted">
          {list.length > 1 ? subName(live, list) : (live.tariff?.name ?? 'Подписка')}
          {expire ? ` до ${dateLong(expire, true)}` : ''}. Докупка действует сразу и не меняет дату окончания.
        </p>
      </header>
      {switcher}
      <div className="cb-cols">
        <div className="cb-col left">
          {devices && trafficOffer ? (
            <div role="group" aria-label="Что докупить" className="cb-seg">
              <button type="button" aria-pressed={kind === 'devices'} onClick={() => setKind('devices')}>
                Устройства
              </button>
              <button type="button" aria-pressed={kind === 'traffic'} onClick={() => setKind('traffic')}>
                Трафик
              </button>
            </div>
          ) : null}

          {kind === 'devices' && devices ? (
            <Card className="cb-devs" label="Устройства">
              <span className="cb-devs-text">
                <h2 className="cb-h2">Устройства</h2>
                <span className="cb-muted">
                  {devices.can_add > 0
                    ? `Сейчас ${devices.included} в тарифе${devices.extra_now > 0 ? ` и ${devices.extra_now} докуплено` : ''}. До конца подписки ${daysText(devices.days)}: одно устройство на этот срок стоит ${
                        starsMode && devices.per_device_stars ? stars(devices.per_device_stars) : money(devices.per_device)
                      }, при продлении — ${starsMode && devices.monthly_stars ? stars(devices.monthly_stars) : money(devices.monthly)} в месяц.`
                    : 'К этой подписке больше устройств не докупить.'}
                </span>
              </span>
              {devices.can_add > 0 ? (
                <span className="cb-stepper">
                  <button type="button" aria-label="Меньше устройств" aria-disabled={count <= 1} onClick={() => count > 1 && setCount(count - 1)}>
                    {MINUS}
                  </button>
                  <span aria-live="polite">+{Math.min(count, devices.can_add)}</span>
                  <button type="button" aria-label="Больше устройств" aria-disabled={count >= devices.can_add} onClick={() => count < devices.can_add && setCount(count + 1)}>
                    +
                  </button>
                </span>
              ) : null}
            </Card>
          ) : null}

          {kind === 'traffic' && trafficOffer ? (
            <Card className="cb-pick" label="Трафик">
              <h2 className="cb-h2">Трафик</h2>
              <p className="cb-muted">
                Израсходовано {traffic(trafficOffer.used_bytes ?? 0)} из {traffic(trafficOffer.limit_bytes ?? 0)}. Докупленный трафик расходуется после основного. {leftoverText(trafficOffer)}
              </p>
              <div role="radiogroup" aria-label="Пакет трафика" className="cb-terms">
                {trafficOffer.packs.map((p) => (
                  <button key={p.gb} type="button" role="radio" aria-checked={pack === p.gb} className="cb-term-card" onClick={() => setPack(p.gb)}>
                    <span className="cb-term-name">+{p.gb} ГБ</span>
                    <span className="cb-term-price">{starsMode && p.stars ? stars(p.stars) : money(p.price)}</span>
                  </button>
                ))}
              </div>
            </Card>
          ) : null}

          <Card className="cb-methods" label="Способ оплаты">
            <h2 className="cb-h2">Способ оплаты</h2>
            {methods.length === 0 && quote ? <p className="cb-muted">Здесь пока нет способов оплаты. Докупить можно в боте.</p> : null}
            <div role="radiogroup" aria-label="Способ оплаты" className="cb-method-grid">
              {methods.map((m) => {
                const off = !!m.unavailable;
                const hint =
                  m.unavailable?.type === 'insufficient_balance'
                    ? `Не хватает: на балансе ${money(m.unavailable.balance)}`
                    : m.unavailable?.type === 'no_stars_price'
                      ? 'Нет цены в звёздах'
                      : m.kind === 'balance'
                        ? `На балансе ${money(me.balance)}`
                        : m.hint;
                return (
                  <button
                    key={methodKey(m)}
                    type="button"
                    role="radio"
                    aria-checked={chosen === m}
                    aria-disabled={off ? true : undefined}
                    className={off ? 'cb-method off' : 'cb-method'}
                    onClick={() => !off && setMethod(methodKey(m))}
                  >
                    <span className="cb-radio" aria-hidden="true" />
                    <span className="cb-method-text">
                      <strong>{m.title}</strong>
                      {hint ? <span>{hint}</span> : null}
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
            {q?.lines.map((line) => (
              <div key={line.label} className="cb-line">
                <span>{line.label}</span>
                <span className="num">{starsMode && line.stars !== null ? stars(line.stars) : money(line.amount)}</span>
              </div>
            ))}
            <div className="cb-due">
              <strong>К оплате</strong>
              <span className="cb-due-sum">{totalText}</span>
            </div>
            {error ? (
              <p className="cb-error" role="alert">
                {error}
              </p>
            ) : null}
            {done ? (
              <p className="cb-note" role="status">
                {done}
              </p>
            ) : null}
            <button type="button" className="cb-main big" onClick={() => void pay()} disabled={busy || !chosen || !q || !choice}>
              {busy ? 'Открываем оплату…' : `Оплатить ${totalText}`}
            </button>
          </Card>
        </div>
      </div>
    </div>
  );
}
