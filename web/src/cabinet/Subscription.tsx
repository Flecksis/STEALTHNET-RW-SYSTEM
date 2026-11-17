import { useEffect, useState } from 'react';
import { Link, useLocation, useNavigate } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, days, money, type Money } from '../lib/format';
import { countryName } from '../lib/geo';
import { Icon } from '../lib/icons';
import { QrCode } from '../lib/qr';
import { siteInviteLink } from '../lib/referral';
import { supportLink, supportTickets } from '../lib/support';
import { Card, useCabinet } from './CabinetApp';
import { DevicesCard } from './DevicesCard';
import { SubSwitch, useSubDetail, useSubs } from './SubSwitch';
import { APPS, explanation, guessPlatform, periodTitle, statusWord, trafficLine, type Catalog, type Platform, type Referral } from './model';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function SubscriptionPage() {
  const { me, site, phone } = useCabinet();
  const location = useLocation();
  const navigate = useNavigate();
  const subs = useSubs();
  const detail = useSubDetail(subs.chosen);
  const view = subs.list === undefined ? undefined : detail.view;
  const devices = detail.devices;
  const chosenId = subs.chosen?.subscription.id ?? null;
  const [referral, setReferral] = useState<Referral | null>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [pageError, setError] = useState<string | null>(null);
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
  const error = pageError ?? subs.error;

  useEffect(() => {
    api<Referral>('/api/client/m/referral/me')
      .then(setReferral)
      .catch(() => setReferral(null));
    api<Catalog>('/api/client/catalog')
      .then(setCatalog)
      .catch(() => setCatalog(null));
  }, []);

  useEffect(() => {
    setAsk('');
    setReissued(false);
    setQr(false);
    setCopied(null);
    setError(null);
  }, [chosenId]);

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
      detail.setView({ ...view, subscription_url: res.subscription_url });
      detail.setDevices([]);
      setReissued(true);
      setAsk('');
    } catch (err) {
      setError(failText(err, 'Новая ссылка не выпущена'));
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
    detail.setView({ ...view, subscription: { ...view.subscription, autorenew: { ...view.subscription.autorenew, enabled } } });
    try {
      await api(`/api/client/subscriptions/${view.subscription.id}/autorenew`, {
        method: 'PUT',
        body: { enabled, period_id: view.subscription.autorenew.period_id },
      });
    } catch (err) {
      detail.setView(prev);
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
        if ((await subs.reload()).length > 0) {
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

  const mayReissue = site.client_access?.reissue !== false;
  const reissueBlock = !mayReissue ? null : reissued ? (
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
          <Link to={subs.link('/cabinet/pay')} className="cb-main">
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
            <Link to={subs.link('/cabinet/pay')} className="cb-second">
              Продлить
            </Link>
            <Link to={subs.link('/cabinet/addons')} className="cb-second">
              Докупить
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
            <span className="cb-link mono">{url ?? (detail.ready ? 'ссылка появится, когда подписка заработает' : 'загружаем ссылку…')}</span>
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

  const devicesCard = <DevicesCard view={view} devices={devices} loading={!detail.ready} onDevices={detail.setDevices} onError={setError} link={subs.link} addLink />;

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
        <SubSwitch subs={subs} />
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
      <SubSwitch subs={subs} />
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
            <div className="cb-row">
              <h2 className="cb-h2">Друзья</h2>
              <Link to="/cabinet/friends" className="cb-accent-link cb-small">
                Подробнее
              </Link>
            </div>
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
        {supportTickets(site) ? (
          <Link id="help" className="cb-card cb-help hov" to="/cabinet/help">
            <span>
              <strong>Нужна помощь?</strong>
              <span className="cb-small cb-muted">Напишите нам — ответ появится здесь, в кабинете</span>
            </span>
            <Icon name="chevron-right" size={20} />
          </Link>
        ) : supportLink(site) ? (
          <a id="help" className="cb-card cb-help hov" href={supportLink(site) ?? undefined} target="_blank" rel="noreferrer">
            <span>
              <strong>Нужна помощь?</strong>
              <span className="cb-small cb-muted">Откроется чат поддержки</span>
            </span>
            <Icon name="chevron-right" size={20} />
          </a>
        ) : site.bot_username ? (
          <a id="help" className="cb-card cb-help hov" href={`https://t.me/${site.bot_username}`} target="_blank" rel="noreferrer">
            <span>
              <strong>Нужна помощь?</strong>
              <span className="cb-small cb-muted">Напишите в Telegram-бота: раздел «Поддержка»</span>
            </span>
            <Icon name="chevron-right" size={20} />
          </a>
        ) : null}
      </div>
    </div>
  );
}

