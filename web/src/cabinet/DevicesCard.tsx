import { Fragment, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong } from '../lib/format';
import { Icon } from '../lib/icons';
import { Card, useCabinet } from './CabinetApp';
import { appName, deviceIcon, type Device, type SubscriptionView } from './model';

export function deviceLimit(view: SubscriptionView | null): number | null {
  return view?.subscription.mirror.hwid_limit ?? view?.tariff?.devices.included ?? null;
}

export function DevicesCard({
  view,
  devices,
  onDevices,
  onError,
  addLink,
}: {
  view: SubscriptionView | null;
  devices: Device[] | null;
  onDevices: (list: Device[]) => void;
  onError: (text: string) => void;
  addLink?: boolean;
}) {
  const { site, phone } = useCabinet();
  const [ask, setAsk] = useState('');
  const [busy, setBusy] = useState(false);
  const mayDisconnect = site.client_access?.devices !== false;

  if (!view) {
    return (
      <Card id="devices" className="cb-devices" label="Устройства">
        <h2 className="cb-h2">Устройства</h2>
        <p className="cb-ink2">Подключить телефон или компьютер можно сразу после покупки подписки.</p>
        <div className="cb-btns">
          <Link to="/cabinet/pay" className="cb-second">
            Выбрать тариф
          </Link>
        </div>
      </Card>
    );
  }

  const limit = deviceLimit(view);

  const disconnect = async (device: Device) => {
    setBusy(true);
    try {
      await api(`/api/client/subscriptions/${view.subscription.id}/devices/${encodeURIComponent(device.hwid)}`, { method: 'DELETE' });
      onDevices((devices ?? []).filter((d) => d.hwid !== device.hwid));
      setAsk('');
    } catch (err) {
      onError(err instanceof ApiError ? err.message : 'Устройство не отключилось');
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card id="devices" className="cb-devices" label="Устройства">
      <div className="cb-row">
        <h2 className="cb-h2">
          Устройства{' '}
          <span className="cb-count">
            {devices ? devices.length : (view.subscription.mirror.devices_count ?? 0)}
            {limit ? ` из ${limit}` : ''}
          </span>
        </h2>
        {addLink && !phone ? (
          <Link to="/cabinet/pay" className="cb-accent-link">
            Добавить ещё
          </Link>
        ) : null}
      </div>
      {devices && devices.length === 0 ? <p className="cb-muted">Устройств нет: появятся, когда вы добавите подписку в приложение.</p> : null}
      {devices === null ? <p className="cb-muted">Список устройств сейчас недоступен</p> : null}
      {devices?.map((d) => (
        <Fragment key={d.hwid}>
          <div className="cb-device">
            <Icon name={deviceIcon(d)} size={22} />
            <span className="cb-device-text">
              {phone ? (
                <span>
                  <strong>{d.title ?? d.platform ?? 'Устройство'}</strong>
                  {appName(d) ? <span className="cb-ink2"> · {appName(d)}</span> : null}
                </span>
              ) : (
                <strong>{d.title ?? d.platform ?? 'Устройство'}</strong>
              )}
              <span className="cb-small cb-muted">
                {phone
                  ? d.added_at
                    ? `Добавлено ${dateLong(d.added_at, false)}`
                    : ''
                  : [appName(d), d.added_at ? `добавлено ${dateLong(d.added_at, false)}` : null].filter(Boolean).join(' · ')}
              </span>
            </span>
            {mayDisconnect ? (
              <button
                type="button"
                className={phone ? 'cb-text' : 'cb-second sm'}
                aria-label={`Отключить ${d.title ?? 'устройство'}`}
                aria-expanded={ask === d.hwid}
                onClick={() => setAsk(ask === d.hwid ? '' : d.hwid)}
                disabled={busy}
              >
                Отключить
              </button>
            ) : null}
          </div>
          {ask === d.hwid ? (
            <div className="cb-ask">
              <p>
                <strong>Отключить {d.title ?? 'устройство'}?</strong> Место освободится, но если подписка на нём осталась, при обновлении он займёт его снова. Чтобы закрыть доступ, выпустите новую ссылку.
              </p>
              <span className="cb-ask-btns">
                <button type="button" className="cb-main sm" onClick={() => void disconnect(d)} disabled={busy}>
                  Отключить
                </button>
                <button type="button" className="cb-cancel" onClick={() => setAsk('')} disabled={busy}>
                  Отменить
                </button>
              </span>
            </div>
          ) : null}
        </Fragment>
      ))}
    </Card>
  );
}
