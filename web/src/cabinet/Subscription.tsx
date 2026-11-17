import { Fragment, useCallback, useEffect, useState } from 'react';
import { Link, useLocation, useNavigate } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, days, money, type Money } from '../lib/format';
import { countryName } from '../lib/geo';
import { Icon } from '../lib/icons';
import { QrCode } from '../lib/qr';
import { siteInviteLink } from '../lib/referral';
import { Card, useCabinet } from './CabinetApp';
import {
  APPS,
  appName,
  deviceIcon,
  explanation,
  guessPlatform,
  periodTitle,
  statusWord,
  trafficLine,
  type Catalog,
  type Device,
  type Platform,
  type Referral,
  type SubscriptionView,
} from './model';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function pickLive(list: SubscriptionView[]): SubscriptionView | null {
  return list.find((v) => !v.subscription.ended_at) ?? null;
}

export function SubscriptionPage() {
  const { me, site, phone } = useCabinet();
  const location = useLocation();
  const navigate = useNavigate();
  const [view, setView] = useState<SubscriptionView | null | undefined>(undefined);
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [referral, setReferral] = useState<Referral | null>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ask, setAsk] = useState<string>('');
  const [reissued, setReissued] = useState(false);
  const [busy, setBusy] = useState(false);
  const [plat, setPlat] = useState<Platform>(() => {
    const guess = guessPlatform();
    return phone ? (guess === 'ios' || guess === 'android' ? guess : 'pc') : guess;
  });
  const [copied, setCopied] = useState<string | null>(null);
  const [qr, setQr] = useState(false);
  const [trialBusy, setTrialBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const list = await api<SubscriptionView[]>('/api/client/subscriptions');
      const live = pickLive(list);
      if (!live) {
        setView(null);
        return;
      }
      setView(live);
      const [detail, devs] = await Promise.all([
        api<SubscriptionView>(`/api/client/subscriptions/${live.subscription.id}`).catch(() => live),
        api<Device[]>(`/api/client/subscriptions/${live.subscription.id}/devices`).catch(() => null),
      ]);
      setView(detail);
      setDevices(devs);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Подписка не загрузилась'));
      setView((v) => (v === undefined ? null : v));
    }
  }, []);

  useEffect(() => {
    void load();
    api<Referral>('/api/client/m/referral/me')
      .then(setReferral)
      .catch(() => setReferral(null));
    api<Catalog>('/api/client/catalog')
      .then(setCatalog)
      .catch(() => setCatalog(null));
  }, [load]);

  useEffect(() => {
    const id = location.hash.replace('#', '');
    if (id && view !== undefined) {
      window.setTimeout(() => document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' }), 50);
    }
  }, [location.hash, view]);

  const copy = async (text: string, key: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      setError('Браузер не дал скопировать: выделите ссылку вручную');
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
      setError(failText(err, 'Новая ссылка не выпущена'));
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async (device: Device) => {
    if (!view) {
      return;
    }
    setBusy(true);
    try {
      await api(`/api/client/subscriptions/${view.subscription.id}/devices/${encodeURIComponent(device.hwid)}`, { method: 'DELETE' });
      setDevices((list) => (list ?? []).filter((d) => d.hwid !== device.hwid));
      setAsk('');
    } catch (err) {
      setError(failText(err, 'Устройство не отключилось'));
    } finally {
      setBusy(false);
    }
  };

  const toggleAuto = async () => {
    if (!view) {
      return;
    }
    const enabled = !view.subscription.autorenew.enabled;
    const prev = view;
    setView({ ...view, subscription: { ...view.subscription, autorenew: { ...view.subscription.autorenew, enabled } } });
    try {
      await api(`/api/client/subscriptions/${view.subscription.id}/autorenew`, {
        method: 'PUT',
        body: { enabled, period_id: view.subscription.autorenew.period_id },
      });
    } catch (err) {
      setView(prev);
      setError(failText(err, 'Автопродление не переключилось'));
    }
  };

  const startTrial = async () => {
    setTrialBusy(true);
    setError(null);
    try {
      await api('/api/client/trial', { method: 'POST', body: {} });
      for (let i = 0; i < 10; i += 1) {
        await new Promise((r) => window.setTimeout(r, 1500));
        const list = await api<SubscriptionView[]>('/api/client/subscriptions');
        if (pickLive(list)) {
          await load();
          break;
        }
      }
    } catch (err) {
      setError(failText(err, 'Пробный период не включился'));
    } finally {
      setTrialBusy(false);
    }
  };

  if (view === undefined) {
    return <p className="cb-muted cb-pad">Загружаем подписку…</p>;
  }

  const blocked = me.client.status === 'blocked';
  const status = statusWord(view, blocked);
  const note = explanation(view, blocked);
  const traffic = view ? trafficLine(view) : null;
  const expire = view?.subscription.mirror.expire_at ?? null;
  const expired = view?.status === 'expired';
  const left = view?.progress?.days_left ?? 0;
  const total = view?.progress?.days_total ?? 0;
  const share = expired || !total ? 0 : Math.min(1, left / total);
  const mode = site.connect?.mode ?? 'ours';
  const url = view?.subscription_url ?? null;
  const app = APPS[plat];
  const limit = view?.subscription.mirror.hwid_limit ?? view?.tariff?.devices.included ?? null;
  const offer = view && catalog ? catalog.tariffs.find((t) => t.tariff.id === view.subscription.tariff_id) : null;
  const renewPeriod = offer?.periods.find((p) => p.period_id === view?.subscription.autorenew.period_id) ?? offer?.periods.find((p) => p.days === view?.subscription.period_days) ?? null;
  const renewPrice: Money | null = renewPeriod?.price ?? null;
  const chargeDate = expire ? new Date(Date.parse(expire) - (site.autorenew_days_before ?? 1) * 86_400_000).toISOString() : null;
  const autoOn = view?.subscription.autorenew.enabled ?? false;
  const balanceOff = me.balance_enabled === false;
  const showBalance = !balanceOff || me.balance.minor > 0;
  const canCharge = !balanceOff || me.autorenew_card === true || (renewPrice !== null && me.balance.minor >= renewPrice.minor);
  const autoText = !view
    ? ''
    : blocked
      ? autoOn
        ? 'На время блокировки не списываем'
        : 'Выключено'
      : !autoOn
        ? expired
          ? 'Выключено'
          : 'Выключено, напомним за 3 дня'
        : !canCharge
          ? expired
            ? 'Продлите подписку вручную'
            : chargeDate
              ? `Напомним продлить ${dateLong(chargeDate, false)}`
              : 'Напомним продлить перед окончанием'
          : expired
            ? balanceOff
              ? renewPrice
                ? `Спишем ${money(renewPrice)} при следующей попытке`
                : 'Спишем при следующей попытке'
              : renewPrice
                ? `Спишем ${money(renewPrice)}, когда хватит баланса`
                : 'Спишем, когда хватит баланса'
            : renewPrice && chargeDate
              ? phone
                ? `Спишем ${money(renewPrice)} ${dateLong(chargeDate, false)}`
                : `${dateLong(chargeDate, false)} спишем ${money(renewPrice)}`
              : balanceOff
                ? 'Спишем перед окончанием'
                : 'Спишем с баланса перед окончанием';

  const platforms: Array<[Platform, string]> = phone
    ? [
        ['ios', 'iPhone'],
        ['android', 'Android'],
        ['pc', 'Компьютер'],
      ]
    : [
        ['ios', 'iPhone'],
        ['android', 'Android'],
        ['win', 'Windows'],
        ['mac', 'macOS'],
        ['linux', 'Linux'],
      ];

  const reissueBlock = reissued ? (
    <p role="status" className="cb-result">
      <strong>Новая ссылка выпущена.</strong> Подключите нужные устройства заново.
    </p>
  ) : ask === 'link' ? (
    <div className="cb-ask">
      <p>Старая ссылка перестанет работать, а с ней все устройства. Нужные устройства подключите заново по новой ссылке.</p>
      <span className={phone ? 'cb-ask-btns col' : 'cb-ask-btns'}>
        <button type="button" className="cb-main sm" onClick={() => void reissue()} disabled={busy}>
          Выпустить новую ссылку
        </button>
        <button type="button" className="cb-cancel" onClick={() => setAsk('')} disabled={busy}>
          Отменить
        </button>
      </span>
    </div>
  ) : (
    <button type="button" className="cb-text" onClick={() => setAsk('link')} disabled={!view || busy}>
      Выпустить новую ссылку
    </button>
  );

  const subCard = !view ? (
    <Card className="cb-sub" label="Подписка">
      <h1 className="cb-h1">Подписки пока нет</h1>
      <p className="cb-ink2">Выберите тариф — подписка начнёт работать сразу после оплаты.</p>
      <div className={phone ? 'cb-btns col' : 'cb-btns'}>
        <Link to="/cabinet/pay" className="cb-main">
          Выбрать тариф
        </Link>
        {site.trial?.enabled ? (
          <button type="button" className="cb-second" onClick={() => void startTrial()} disabled={trialBusy}>
            {trialBusy ? 'Включаем…' : `Попробовать бесплатно ${days(site.trial.days)}`}
          </button>
        ) : null}
      </div>
    </Card>
  ) : (
    <Card className="cb-sub" label="Подписка">
      <div className="cb-row">
        <h1 className="cb-h1">{periodTitle(view)}</h1>
        <span className={`cb-status ${status.tone}`}>{status.text}</span>
      </div>
      <div className="cb-term">
        <span className="cb-days">{expired ? days(0) : days(left)}</span>
        <span className="cb-muted">{expire ? (expired ? `закончилась ${dateLong(expire, false)}` : `до ${dateLong(expire, true)}`) : ''}</span>
      </div>
      <div className="cb-bar" role="img" aria-label={expired && expire ? `Подписка закончилась ${dateLong(expire, false)}` : `Осталось ${days(left)} из ${total}`}>
        <span style={{ width: `${Math.round(share * 100)}%` }} />
      </div>
      {phone ? (
        traffic || note ? (
          <div className="cb-meta">
            {traffic ? (
              <span>
                Трафик: <span className={traffic.exhausted ? 'cb-warn' : undefined}>{traffic.used}</span>
                {traffic.tail}
              </span>
            ) : null}
            {note ? <span>{note}</span> : null}
          </div>
        ) : null
      ) : (
        <>
          <div className="cb-facts">
            <span>
              Трафик:{' '}
              {traffic ? (
                <>
                  <strong className={traffic.exhausted ? 'cb-warn' : undefined}>{traffic.used}</strong>
                  {traffic.tail}
                </>
              ) : (
                <strong>без лимита</strong>
              )}
            </span>
            {view.countries.length ? (
              <span>
                Серверы: <strong>{view.countries.map(countryName).filter(Boolean).join(', ')}</strong>
              </span>
            ) : null}
          </div>
          {note ? <p className="cb-note">{note}</p> : null}
        </>
      )}
      <div className={phone ? 'cb-btns col' : 'cb-btns'}>
        {blocked ? (
          site.support_url ? (
            <a className="cb-main" href={site.support_url}>
              Написать в поддержку
            </a>
          ) : null
        ) : expired ? (
          <Link to="/cabinet/pay" className="cb-main">
            Продлить
          </Link>
        ) : (
          <>
            {mode === 'remnawave' && url ? (
              <a className="cb-main" href={url} target="_blank" rel="noreferrer">
                Открыть страницу подключения
                <Icon name="external" size={18} />
              </a>
            ) : (
              <button type="button" className="cb-main" onClick={() => document.getElementById('connect')?.scrollIntoView({ behavior: 'smooth' })}>
                Подключить устройство
              </button>
            )}
            <Link to="/cabinet/pay" className="cb-second">
              Продлить
            </Link>
          </>
        )}
      </div>
    </Card>
  );

  const connectCard = view ? (
    <Card id="connect" className="cb-connect" label="Подключение">
      <h2 className="cb-h2">Подключение</h2>
      {mode === 'remnawave' ? (
        <>
          <p className="cb-ink2">
            На странице подключения есть приложения для вашего устройства и кнопки, которые сами добавят подписку.{phone ? '' : ' Чтобы подключить телефон, отсканируйте QR-код.'}
          </p>
          <div className={phone ? 'cb-linkrow col' : 'cb-linkrow'}>
            <span className="cb-link mono">{url ?? 'ссылка появится, когда подписка заработает'}</span>
            {phone && url ? (
              <a className="cb-main" href={url} target="_blank" rel="noreferrer">
                Открыть страницу подключения
                <Icon name="external" size={18} />
              </a>
            ) : null}
            <button type="button" className="cb-second" onClick={() => url && void copy(url, 'url')} disabled={!url}>
              {copied === 'url' ? 'Скопировано' : 'Скопировать'}
            </button>
            {!phone ? (
              <button type="button" className="cb-second" aria-expanded={qr} onClick={() => setQr(!qr)} disabled={!url}>
                QR-код
              </button>
            ) : null}
          </div>
          {qr && url ? <QrCode text={url} size={200} label="QR-код ссылки подписки" /> : null}
          {reissueBlock}
        </>
      ) : (
        <>
          <div role="group" aria-label="Устройство" className={phone ? 'cb-plats seg' : 'cb-plats'}>
            {platforms.map(([key, label]) => (
              <button key={key} type="button" aria-pressed={plat === key} onClick={() => setPlat(key)}>
                {label}
              </button>
            ))}
          </div>
          <ol className="cb-steps">
            <li>
              <span className="cb-num">1</span>
              <span className="cb-step">
                <span className="cb-step-row">
                  <span className="cb-step-text">
                    <strong>Установите {app.app}</strong>
                    <span className="cb-muted">{phone ? (plat === 'pc' ? app.store : 'Бесплатно') : app.store}</span>
                  </span>
                  <a className="cb-second sm" href={app.download} target="_blank" rel="noreferrer">
                    {phone ? app.storeShort : 'Скачать'}
                  </a>
                </span>
              </span>
            </li>
            <li>
              <span className="cb-num">2</span>
              <span className="cb-step">
                <span className="cb-step-text">
                  <strong>Добавьте подписку</strong>
                  {phone ? null : <span className="cb-muted">Одна кнопка, ссылка откроется в приложении</span>}
                </span>
                <span className={phone ? 'cb-btns col' : 'cb-btns'}>
                  <a className={url ? 'cb-main sm' : 'cb-main sm off'} href={url ? app.add(url) : undefined} aria-disabled={url ? undefined : true}>
                    Добавить в {app.app}
                  </a>
                  <button type="button" className="cb-second sm" onClick={() => url && void copy(url, 'url')} disabled={!url}>
                    {copied === 'url' ? 'Скопировано' : 'Скопировать'}
                  </button>
                  {!phone ? (
                    <button type="button" className="cb-second sm" aria-expanded={qr} onClick={() => setQr(!qr)} disabled={!url}>
                      QR-код
                    </button>
                  ) : null}
                </span>
                {qr && url ? <QrCode text={url} size={200} label="QR-код ссылки подписки" /> : null}
                {phone ? reissueBlock : null}
              </span>
            </li>
            <li>
              <span className="cb-num">3</span>
              <span className="cb-step-text">
                <strong>Включите VPN</strong>
                <span className="cb-muted">{phone ? 'Сервер выберется сам' : `Нажмите кнопку подключения в ${app.app}, сервер выберется сам`}</span>
              </span>
            </li>
          </ol>
          {!phone ? (
            <>
              <p className="cb-muted">Другие приложения: {app.others}</p>
              {reissueBlock}
            </>
          ) : null}
        </>
      )}
    </Card>
  ) : null;

  const devicesCard = view ? (
    <Card id="devices" className="cb-devices" label="Устройства">
      <div className="cb-row">
        <h2 className="cb-h2">
          Устройства{' '}
          <span className="cb-count">
            {devices ? devices.length : (view.subscription.mirror.devices_count ?? 0)}
            {limit ? ` из ${limit}` : ''}
          </span>
        </h2>
        {!phone ? (
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
  ) : null;

  const autoSwitch = view ? (
    <button type="button" role="switch" className={phone ? 'cb-switch big' : 'cb-switch'} aria-checked={autoOn} aria-label="Автопродление" onClick={() => void toggleAuto()} disabled={blocked}>
      <span />
    </button>
  ) : null;

  const errorLine = error ? (
    <p className="cb-error" role="alert">
      {error}
    </p>
  ) : null;

  if (phone) {
    return (
      <div className="cb-stack">
        {errorLine}
        {subCard}
        {connectCard}
        {devicesCard}
        {view ? (
          <Card className="cb-auto" label="Автопродление">
            <span className="cb-auto-text">
              <strong>Автопродление</strong>
              <span className="cb-small cb-muted">{autoText}</span>
            </span>
            {autoSwitch}
          </Card>
        ) : null}
      </div>
    );
  }

  return (
    <div className="cb-cols">
      <div className="cb-col left">
        {errorLine}
        {subCard}
        {connectCard}
        {devicesCard}
      </div>
      <div className="cb-col right">
        {showBalance || view ? (
          <Card className="cb-balance-card" label={showBalance ? 'Баланс' : 'Автопродление'}>
            {showBalance ? (
              <>
                <span className="cb-row">
                  <span className="cb-label">{balanceOff ? 'Остаток на балансе' : 'Баланс'}</span>
                  <Link to="/cabinet/payments" className="cb-accent-link cb-small">
                    История оплат
                  </Link>
                </span>
                <span className="cb-sum">{money(me.balance)}</span>
                {balanceOff ? (
                  <span className="cb-small cb-muted">Пополнить баланс нельзя. Остатком можно оплатить подписку, если его хватает на всю сумму</span>
                ) : (
                  <button type="button" className="cb-main wide" onClick={() => navigate('/cabinet/topup')}>
                    Пополнить
                  </button>
                )}
              </>
            ) : null}
            {view ? (
              <div className={showBalance ? 'cb-auto-row' : 'cb-auto-row solo'}>
                <span className="cb-auto-text">
                  <strong>Автопродление</strong>
                  <span className="cb-small cb-muted">{autoText}</span>
                </span>
                {autoSwitch}
              </div>
            ) : null}
          </Card>
        ) : null}
        {referral ? (
          <Card id="friends" className="cb-friends" label="Друзья">
            <h2 className="cb-h2">Друзья</h2>
            <p className="cb-ink2">Пригласите друга и получайте {referral.levels[0] ?? 0}% от его оплат на баланс.</p>
            <span className="cb-small cb-muted">Ссылка на сайт</span>
            <div className="cb-linkrow">
              <span className="cb-link mono">{siteInviteLink(referral.code).replace(/^https?:\/\//, '')}</span>
              <button type="button" className="cb-second" onClick={() => void copy(siteInviteLink(referral.code), 'ref-site')}>
                {copied === 'ref-site' ? 'Скопировано' : 'Скопировать'}
              </button>
            </div>
            {referral.bot_link ? (
              <>
                <span className="cb-small cb-muted">Ссылка на Telegram-бота</span>
                <div className="cb-linkrow">
                  <span className="cb-link mono">{referral.bot_link.replace(/^https:\/\//, '')}</span>
                  <button type="button" className="cb-second" onClick={() => void copy(referral.bot_link ?? '', 'ref')}>
                    {copied === 'ref' ? 'Скопировано' : 'Скопировать'}
                  </button>
                </div>
              </>
            ) : null}
            <span className="cb-small cb-muted">
              Пришли по ссылке: {referral.invited} · заработано {money(referral.earned)}
            </span>
          </Card>
        ) : null}
        {site.support_url ? (
          <a id="help" className="cb-card cb-help hov" href={site.support_url}>
            <span>
              <strong>Нужна помощь?</strong>
              <span className="cb-small cb-muted">Откроется чат поддержки</span>
            </span>
            <Icon name="chevron-right" size={20} />
          </a>
        ) : null}
      </div>
    </div>
  );
}

