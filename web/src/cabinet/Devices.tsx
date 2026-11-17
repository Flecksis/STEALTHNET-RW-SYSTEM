import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { count, money } from '../lib/format';
import { Card, useCabinet } from './CabinetApp';
import { DevicesCard, deviceLimit } from './DevicesCard';
import { stars, type Device, type SubscriptionView } from './model';

function pickLive(list: SubscriptionView[]): SubscriptionView | null {
  return list.find((v) => !v.subscription.ended_at) ?? null;
}

export function DevicesPage() {
  const { me } = useCabinet();
  const [view, setView] = useState<SubscriptionView | null | undefined>(undefined);
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const live = pickLive(await api<SubscriptionView[]>('/api/client/subscriptions'));
      if (!live) {
        setView(null);
        return;
      }
      const [detail, list] = await Promise.all([
        api<SubscriptionView>(`/api/client/subscriptions/${live.subscription.id}`).catch(() => live),
        api<Device[]>(`/api/client/subscriptions/${live.subscription.id}/devices`).catch(() => null),
      ]);
      setView(detail);
      setDevices(list);
      setError(null);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Устройства не загрузились');
      setView((v) => (v === undefined ? null : v));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (view === undefined) {
    return <p className="cb-muted cb-pad">Загружаем устройства…</p>;
  }

  const limit = deviceLimit(view);
  const extra = view?.tariff?.devices.extra ?? null;
  const included = view?.tariff?.devices.included ?? null;
  const used = devices?.length ?? view?.subscription.mirror.devices_count ?? 0;
  const monthly = extra ? (me.currency === 'RUB' ? extra.monthly.rub : extra.monthly.usd) : undefined;
  const price = monthly !== undefined ? money({ minor: monthly, currency: me.currency }) : extra?.monthly_stars ? stars(extra.monthly_stars) : null;
  const ended = view?.status === 'expired' || view?.status === 'blocked';

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <h1 className="cb-title">Устройства</h1>
        {view && limit && used >= limit ? <p className="cb-lead muted">Все места заняты: чтобы подключить новое устройство, отключите одно из старых.</p> : null}
      </header>
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
      <DevicesCard view={view} devices={devices} onDevices={setDevices} onError={setError} />
      {view ? (
        <Card className="cb-devhelp" label="Новое устройство">
          <h2 className="cb-h2">Новое устройство</h2>
          <p className="cb-ink2">Установите приложение и добавьте в него подписку: ссылка, QR-код и шаги — на главной странице кабинета. Устройство появится в списке после первого подключения.</p>
          <div className="cb-btns">
            <Link to="/cabinet#connect" className="cb-second">
              Как подключить
            </Link>
            {extra && !ended ? (
              <Link to="/cabinet/pay" className="cb-second">
                Добавить устройства
              </Link>
            ) : null}
          </div>
          {included !== null ? (
            <p className="cb-small cb-muted">
              {`В тариф входит ${count(included, 'устройство', 'устройства', 'устройств')}.`}
              {extra && extra.max > 0 ? ` Можно добавить ещё до ${extra.max}${price ? `, ${price} в месяц за каждое` : ''}.` : ''}
            </p>
          ) : null}
        </Card>
      ) : null}
    </div>
  );
}
