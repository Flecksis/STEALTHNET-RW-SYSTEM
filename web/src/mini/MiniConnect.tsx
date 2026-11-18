import { Fragment, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong } from '../lib/format';
import { t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { QrCode } from '../lib/qr';
import { APPS, appName, deviceIcon, guessPlatform, type Device, type Platform } from '../cabinet/model';
import { BackHead, MiniSubSwitch, useMini } from './MiniApp';
import { openExternal } from './telegram';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniConnect() {
  const { site, view, ready, setView, devices, setDevices } = useMini();
  const [plat, setPlat] = useState<Platform>(() => {
    const g = guessPlatform();
    return g === 'ios' || g === 'android' ? g : 'pc';
  });
  const [ask, setAsk] = useState('');
  const [reissued, setReissued] = useState(false);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [qr, setQr] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const chosenId = view?.subscription.id ?? null;

  useEffect(() => {
    setAsk('');
    setReissued(false);
    setCopied(false);
    setQr(false);
    setError(null);
  }, [chosenId]);

  const url = view?.subscription_url ?? null;
  const mode = site.connect?.mode ?? 'ours';
  const app = APPS[plat];
  const limit = view?.subscription.mirror.hwid_limit ?? view?.tariff?.devices.included ?? null;

  const copy = async () => {
    if (!url) {
      return;
    }
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setError(t('Не получилось скопировать: выделите ссылку вручную'));
    }
  };

  const reissue = async () => {
    if (!view) {
      return;
    }
    setBusy(true);
    try {
      const res = await api<{ subscription_url: string }>(`/api/client/subscriptions/${view.subscription.id}/reissue`, { method: 'POST', body: {} });
      setView({ ...view, subscription_url: res.subscription_url });
      setDevices([]);
      setReissued(true);
      setAsk('');
    } catch (err) {
      setError(failText(err, t('Новая ссылка не выпущена')));
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async (d: Device) => {
    if (!view) {
      return;
    }
    setBusy(true);
    try {
      await api(`/api/client/subscriptions/${view.subscription.id}/devices/${encodeURIComponent(d.hwid)}`, { method: 'DELETE' });
      setDevices((devices ?? []).filter((x) => x.hwid !== d.hwid));
      setAsk('');
    } catch (err) {
      setError(failText(err, t('Устройство не отключилось')));
    } finally {
      setBusy(false);
    }
  };

  const reissueBlock = site.client_access?.reissue === false ? null : reissued ? (
    <p role="status" className="mn-ask">
      <span>
        <strong>{t('Новая ссылка выпущена.')}</strong> {t('Подключите нужные устройства заново.')}
      </span>
    </p>
  ) : ask === 'link' ? (
    <div className="mn-ask">
      <span>{t('Старая ссылка перестанет работать, а с ней все устройства. Нужные устройства подключите заново по новой ссылке.')}</span>
      <span className="mn-ask-col">
        <button type="button" className="mn-ask-ok press" onClick={() => void reissue()} disabled={busy}>
          {t('Выпустить новую ссылку')}
        </button>
        <button type="button" className="mn-ask-cancel press" onClick={() => setAsk('')} disabled={busy}>
          {t('Отменить')}
        </button>
      </span>
    </div>
  ) : (
    <button type="button" className="mn-text press" aria-expanded={false} onClick={() => setAsk('link')} disabled={!view}>
      {t('Выпустить новую ссылку')}
    </button>
  );

  if (!view) {
    return (
      <main className="mn-page connect">
        <BackHead title={t('Подключение')} />
        <div className="mn-card mn-pad">
          <p>{t('Сначала оформите подписку — потом здесь появятся шаги подключения.')}</p>
          <Link to="/app/plans" className="mn-main press">
            {t('Выбрать тариф')}
          </Link>
        </div>
      </main>
    );
  }

  const devCount = devices?.length ?? view.subscription.mirror.devices_count ?? 0;

  return (
    <main className="mn-page connect">
      <BackHead title={t('Подключение')} />
      <MiniSubSwitch />
      {mode === 'remnawave' ? (
        <div className="mn-card mn-remna">
          <strong className="mn-big">{t('Всё на одной странице')}</strong>
          <span className="mn-ink2">{t('На странице подключения есть приложения для вашего устройства и кнопки, которые сами добавят подписку.')}</span>
          <span className="mn-url mono">{url ?? (ready ? t('ссылка появится, когда подписка заработает') : t('загружаем ссылку…'))}</span>
          <button type="button" className="mn-main sm press" onClick={() => url && openExternal(url)} disabled={!url}>
            {t('Открыть страницу подключения')}
            <Icon name="external" size={18} />
          </button>
          <span className="mn-two">
            <button type="button" className="mn-btn2 press" onClick={() => void copy()} disabled={!url}>
              {copied ? t('Скопировано') : t('Скопировать')}
            </button>
            <button type="button" className="mn-btn2 press" aria-expanded={qr} onClick={() => setQr(!qr)} disabled={!url}>
              {t('QR-код')}
            </button>
          </span>
          {qr && url ? <QrCode text={url} size={200} label={t('QR-код ссылки подписки')} /> : null}
          {reissueBlock}
        </div>
      ) : (
        <div className="mn-steps">
          <div role="group" aria-label={t('Устройство')} className="mn-seg three">
            {(
              [
                ['ios', 'iPhone'],
                ['android', 'Android'],
                ['pc', t('Компьютер')],
              ] as Array<[Platform, string]>
            ).map(([key, label]) => (
              <button key={key} type="button" aria-pressed={plat === key} onClick={() => setPlat(key)}>
                {label}
              </button>
            ))}
          </div>
          <div className="mn-card mn-step">
            <span className="mn-num">1</span>
            <span className="mn-step-text">
              <strong>{t('Установите {app}', { app: app.app })}</strong>
              <span>{t(app.store)}</span>
            </span>
            <button type="button" className="mn-btn2 press" onClick={() => openExternal(app.download)}>
              {t(app.storeShort)}
            </button>
          </div>
          <div className="mn-card mn-step col">
            <span className="mn-step-row">
              <span className="mn-num">2</span>
              <span className="mn-step-text">
                <strong>{t('Добавьте подписку')}</strong>
                <span>{t('Ссылка сама откроется в приложении')}</span>
              </span>
            </span>
            <button type="button" className="mn-main sm press" onClick={() => url && openExternal(app.add(url))} disabled={!url}>
              {t('Добавить в {app}', { app: app.app })}
            </button>
            <span className="mn-two">
              <button type="button" className="mn-btn2 press" onClick={() => void copy()} disabled={!url}>
                {copied ? t('Скопировано') : t('Скопировать')}
              </button>
              <button type="button" className="mn-btn2 press" aria-expanded={qr} onClick={() => setQr(!qr)} disabled={!url}>
                {t('QR-код')}
              </button>
            </span>
            {qr && url ? <QrCode text={url} size={200} label={t('QR-код ссылки подписки')} /> : null}
            {reissueBlock}
          </div>
          <div className="mn-card mn-step">
            <span className="mn-num">3</span>
            <span className="mn-step-text">
              <strong>{t('Включите VPN')}</strong>
              <span>{t('Кнопка подключения в {app}, сервер выберется сам', { app: app.app })}</span>
            </span>
          </div>
        </div>
      )}
      {error ? <p className="mn-error">{error}</p> : null}
      <section aria-label={t('Устройства')} className="mn-devsec">
        <div className="mn-devsec-head">
          <h2>
            {limit ? t('Устройства {count} из {limit}', { count: devCount, limit }) : t('Устройства {count}', { count: devCount })}
          </h2>
          <Link to="/app/plans" className="mn-link" aria-label={t('Добавить устройство')}>
            {t('Добавить')}
          </Link>
        </div>
        <div className="mn-list">
          {devices && devices.length === 0 ? <p className="mn-empty">{t('Устройств нет: появятся, когда вы добавите подписку в приложение.')}</p> : null}
          {devices === null ? <p className="mn-empty">{ready ? t('Список устройств сейчас недоступен') : t('Загружаем устройства…')}</p> : null}
          {devices?.map((d) => (
            <Fragment key={d.hwid}>
              <div className="mn-dev">
                <Icon name={deviceIcon(d)} size={22} />
                <span className="mn-dev-text">
                  <strong>{d.title ?? d.platform ?? t('Устройство')}</strong>
                  <span>{[appName(d), d.added_at ? t('добавлено {date}', { date: dateLong(d.added_at, false) }) : null].filter(Boolean).join(' · ')}</span>
                </span>
                <button type="button" className="mn-x press" aria-label={t('Отключить {device}', { device: d.title ?? t('устройство') })} aria-expanded={ask === d.hwid} onClick={() => setAsk(ask === d.hwid ? '' : d.hwid)}>
                  <Icon name="close" size={18} />
                </button>
              </div>
              {ask === d.hwid ? (
                <div className="mn-ask dev">
                  <span>
                    <strong>{t('Отключить {device}?', { device: d.title ?? t('устройство') })}</strong> {t('Место освободится, но если подписка на нём осталась, при обновлении он займёт его снова. Чтобы закрыть доступ, выпустите новую ссылку.')}
                  </span>
                  <span className="mn-ask-grid">
                    <button type="button" className="mn-ask-ok press" onClick={() => void disconnect(d)} disabled={busy}>
                      {t('Отключить')}
                    </button>
                    <button type="button" className="mn-ask-cancel press" onClick={() => setAsk('')} disabled={busy}>
                      {t('Отменить')}
                    </button>
                  </span>
                </div>
              ) : null}
            </Fragment>
          ))}
        </div>
      </section>
    </main>
  );
}
