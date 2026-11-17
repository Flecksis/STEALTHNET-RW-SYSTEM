import { useEffect, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, money, MINUS, traffic } from '../lib/format';
import { stars } from '../cabinet/model';
import { addonItem, daysText, doneText, firstKind, leftoverText, offerUrl, type AddonChoice, type AddonKind, type AddonsOffer, type ItemQuote, type PayResult } from '../cabinet/extras';
import { BackHead, useMini } from './MiniApp';
import { openExternal, webApp } from './telegram';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniAddons() {
  const { me, view, reload } = useMini();
  const [params] = useSearchParams();
  const [offer, setOffer] = useState<AddonsOffer | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [kind, setKind] = useState<AddonKind>('devices');
  const [count, setCount] = useState(1);
  const [pack, setPack] = useState<number | null>(null);
  const [quote, setQuote] = useState<ItemQuote | null>(null);
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [otherWallet, setOtherWallet] = useState<string | null>(null);
  const seq = useRef(0);
  const subscriptionId = view && !view.subscription.ended_at ? view.subscription.id : null;
  const asked = params.get('kind');

  const load = async (id: number, keep: boolean) => {
    try {
      const next = await api<AddonsOffer>(offerUrl(id));
      setOffer(next);
      setLoadError(null);
      if (!keep) {
        setKind(firstKind(next, asked));
        setCount(1);
        setPack(next.traffic.offer?.packs[0]?.gb ?? null);
      }
    } catch (err) {
      setLoadError(failText(err, 'Не удалось загрузить предложения'));
    }
  };

  useEffect(() => {
    if (subscriptionId !== null) {
      void load(subscriptionId, false);
    }
  }, [subscriptionId]);

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
  const chosen = methods.find((m) => m.provider === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
  const starsMode = chosen?.kind === 'stars';
  const inTelegramWallet = chosen?.provider === 'gram' && webApp() !== null;
  const balanceShort = methods.find((m) => m.kind === 'balance' && m.unavailable?.type === 'insufficient_balance');
  const q = quote?.quote ?? null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? money(q.total) : '';

  const pay = async () => {
    if (subscriptionId === null || !choice || !chosen) {
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    setOtherWallet(null);
    try {
      const res = await api<PayResult>('/api/client/items/checkout', {
        method: 'POST',
        body: { item: addonItem(subscriptionId, choice), provider: chosen.provider },
      });
      if (res.type === 'redirect') {
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
        setNote('Оплата открылась в новом окне. Когда она пройдёт, лимит обновится сам.');
        return;
      }
      setNote(doneText(choice));
      window.setTimeout(() => {
        void reload();
        void load(subscriptionId, true);
      }, 1500);
    } catch (err) {
      setError(failText(err, 'Оплата не началась'));
    } finally {
      setBusy(false);
    }
  };

  if (subscriptionId === null || view?.status === 'expired') {
    return (
      <main className="mn-page">
        <BackHead title="Докупить" />
        <div className="mn-card mn-pad">
          <strong>Нужна действующая подписка</strong>
          <span className="mn-ink2">Устройства и трафик докупаются к подписке, пока она действует.</span>
          <Link to="/app/plans" className="mn-main sm press">
            {view ? 'Продлить' : 'Выбрать тариф'}
          </Link>
        </div>
      </main>
    );
  }

  if (!offer) {
    return (
      <main className="mn-page">
        <BackHead title="Докупить" />
        <p className={loadError ? 'mn-error' : 'mn-muted'}>{loadError ?? 'Загружаем…'}</p>
      </main>
    );
  }

  if (!devices && !trafficOffer) {
    const reason = offer.devices.unavailable?.message ?? offer.traffic.unavailable?.message ?? 'Для этой подписки докупить нечего.';
    return (
      <main className="mn-page">
        <BackHead title="Докупить" />
        <div className="mn-card mn-pad">
          <strong>Докупить нечего</strong>
          <span className="mn-ink2">{reason}</span>
        </div>
      </main>
    );
  }

  return (
    <main className="mn-page plans">
      <BackHead title="Докупить" />
      {devices && trafficOffer ? (
        <div role="group" aria-label="Что докупить" className="mn-seg" style={{ gridTemplateColumns: 'repeat(2, minmax(0, 1fr))' }}>
          <button type="button" aria-pressed={kind === 'devices'} onClick={() => setKind('devices')}>
            Устройства
          </button>
          <button type="button" aria-pressed={kind === 'traffic'} onClick={() => setKind('traffic')}>
            Трафик
          </button>
        </div>
      ) : null}

      {kind === 'devices' && devices ? (
        <>
          <div className="mn-card mn-devs">
            <span className="mn-devs-text">
              <strong>Устройства</strong>
              <span>
                {devices.included} в тарифе
                {devices.extra_now > 0 ? `, ${devices.extra_now} докуплено` : ''}
              </span>
            </span>
            {devices.can_add > 0 ? (
              <span className="mn-stepper">
                <button type="button" className="mn-circ press" aria-label="Меньше устройств" aria-disabled={count <= 1} onClick={() => count > 1 && setCount(count - 1)}>
                  {MINUS}
                </button>
                <span aria-live="polite">+{Math.min(count, devices.can_add)}</span>
                <button type="button" className="mn-circ press" aria-label="Больше устройств" aria-disabled={count >= devices.can_add} onClick={() => count < devices.can_add && setCount(count + 1)}>
                  +
                </button>
              </span>
            ) : null}
          </div>
          <p className="mn-notes">
            {devices.can_add > 0 ? (
              <span>
                До конца подписки {daysText(devices.days)}, до {dateLong(devices.expire_at, false)}. Одно устройство на этот срок стоит{' '}
                {starsMode && devices.per_device_stars ? stars(devices.per_device_stars) : money(devices.per_device)}. При продлении устройство стоит{' '}
                {starsMode && devices.monthly_stars ? stars(devices.monthly_stars) : money(devices.monthly)} в месяц.
              </span>
            ) : (
              <span>К этой подписке больше устройств не докупить.</span>
            )}
          </p>
        </>
      ) : null}

      {kind === 'traffic' && trafficOffer ? (
        <>
          <p className="mn-notes">
            <span>
              Израсходовано {traffic(trafficOffer.used_bytes ?? 0)} из {traffic(trafficOffer.limit_bytes ?? 0)}. Докупленный трафик расходуется после основного.
            </span>
            <span>{leftoverText(trafficOffer)}</span>
          </p>
          <div role="radiogroup" aria-label="Пакет трафика" className="mn-terms">
            {trafficOffer.packs.map((p) => (
              <button key={p.gb} type="button" role="radio" aria-checked={pack === p.gb} className="mn-term press" onClick={() => setPack(p.gb)}>
                <span className="mn-dot" aria-hidden="true" />
                <span className="mn-term-text">
                  <span className="mn-term-top">
                    <strong>+{p.gb} ГБ</strong>
                    <strong className="num">{starsMode && p.stars ? stars(p.stars) : money(p.price)}</strong>
                  </span>
                </span>
              </button>
            ))}
          </div>
        </>
      ) : null}

      <div className="mn-pay">
        <div className="mn-pay-head">
          <h2>Оплата</h2>
        </div>
        {q && methods.length === 0 ? <p className="mn-muted small">В мини-приложении оплатить сейчас нельзя. Докупить можно в боте или в кабинете на сайте.</p> : null}
        <div role="radiogroup" aria-label="Способ оплаты" className="mn-chips">
          {methods.map((m) => (
            <button key={m.provider} type="button" role="radio" aria-checked={chosen?.provider === m.provider} className="mn-chip press" disabled={!!m.unavailable} onClick={() => setMethod(m.provider)}>
              {m.title}
            </button>
          ))}
        </div>
        {balanceShort?.unavailable?.type === 'insufficient_balance' ? <p className="mn-muted small">Не хватает: на балансе {money(me.balance)}</p> : null}
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
      <button type="button" className="mn-main mn-paybtn press" onClick={() => void pay()} disabled={busy || !chosen || !q || !choice}>
        {busy ? 'Открываем оплату…' : `Оплатить ${totalText}${inTelegramWallet ? ' в Telegram' : ''}`}
      </button>
    </main>
  );
}
