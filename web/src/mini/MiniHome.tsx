import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { BrandMark } from '../lib/brand';
import { api } from '../lib/api';
import { dateLong, money, plural } from '../lib/format';
import { Icon, type IconName } from '../lib/icons';
import { resetWhen, trafficMeter, type Referral } from '../cabinet/model';
import { MiniSubSwitch, ThemeCirc, useMini } from './MiniApp';
import { openExternal } from './telegram';
import { supportLink, supportTickets } from '../lib/support';

const R = 90;
const LEN = 2 * Math.PI * R;

export function MiniHome() {
  const { me, site, view, devices } = useMini();
  const [referral, setReferral] = useState<Referral | null>(null);
  const [gifts, setGifts] = useState(false);

  useEffect(() => {
    api<Referral>('/api/client/m/referral/me')
      .then(setReferral)
      .catch(() => setReferral(null));
    api('/api/client/m/gifts/offers')
      .then(() => setGifts(true))
      .catch(() => setGifts(false));
  }, []);

  const blocked = me.client.status === 'blocked' || view?.status === 'blocked';
  const expired = view?.status === 'expired';
  const limited = view?.status === 'limited';
  const left = view?.progress?.days_left ?? 0;
  const total = view?.progress?.days_total ?? 0;
  const share = !view || expired || !total ? 0 : Math.min(1, left / total);
  const expire = view?.subscription.mirror.expire_at ?? null;
  const name = view?.tariff?.name ?? (view?.subscription.kind === 'trial' ? 'Пробный период' : 'Подписка');
  const meter = view && !expired && !blocked ? trafficMeter(view) : null;
  const tight = !!meter && (meter.exhausted || meter.share >= 0.9);
  const reset = view?.tariff?.traffic.reset ?? 'no_reset';
  const mode = site.connect?.mode ?? 'ours';
  const url = view?.subscription_url ?? null;
  const limit = view?.subscription.mirror.hwid_limit ?? view?.tariff?.devices.included ?? null;
  const devCount = devices?.length ?? view?.subscription.mirror.devices_count ?? 0;

  const line2 = !view
    ? { text: 'Подписки пока нет', tone: 'muted' }
    : blocked
      ? { text: 'Подписка заблокирована', tone: 'bad' }
      : limited
        ? { text: `Трафик закончился${reset !== 'no_reset' ? `, ${resetWhen(reset, view.subscription).tail}` : ''}`, tone: 'warn' }
        : expired
          ? { text: 'Истекла', tone: 'muted' }
          : { text: view.subscription.autorenew.enabled ? 'Активна, автопродление включено' : 'Активна', tone: 'ink2' };

  const connect =
    mode === 'remnawave' && url
      ? { label: 'Открыть страницу подключения', icon: 'external' as IconName, run: () => openExternal(url), to: undefined }
      : { label: 'Подключить', icon: 'zap' as IconName, run: undefined, to: '/app/connect' };

  const support = supportTickets(site) && site.bot_username ? `https://t.me/${site.bot_username}?start=support` : supportLink(site);

  type Btn = { label: string; icon: IconName; to?: string; run?: () => void; href?: string };
  const main: Btn = !view
    ? { label: 'Выбрать тариф', icon: 'tag', to: '/app/plans' }
    : blocked
      ? { label: 'Написать в поддержку', icon: 'chat', run: support ? () => openExternal(support) : undefined }
      : expired
        ? { label: 'Продлить', icon: 'refresh', to: '/app/plans' }
        : connect;
  const left2: Btn = expired ? { ...connect, label: 'Подключить' } : { label: 'Продлить', icon: 'refresh', to: '/app/plans' };

  const button = (b: Btn, cls: string, size: number) =>
    b.to ? (
      <Link to={b.to} className={cls}>
        <Icon name={b.icon} size={size} />
        {b.label}
      </Link>
    ) : (
      <button type="button" className={cls} onClick={b.run}>
        <Icon name={b.icon} size={size} />
        {b.label}
      </button>
    );

  const rows: Array<{ label: string; icon: IconName; value?: string; to?: string; run?: () => void }> = [];
  if (referral) {
    rows.push({ label: 'Друзья', icon: 'share', value: referral.invited ? `${referral.invited} ${plural(referral.invited, 'пришёл', 'пришли', 'пришли')}` : undefined, to: '/app/profile' });
  }
  if (view && !expired && !blocked && !view.subscription.ended_at) {
    rows.push({ label: 'Докупить устройства или трафик', icon: 'plus', to: '/app/addons' });
  }
  if (gifts) {
    rows.push({ label: 'Подарить подписку', icon: 'gift', to: '/app/gifts' });
  }
  rows.push({ label: 'Промокод', icon: 'percent', to: '/app/plans' });
  rows.push({ label: 'История оплат', icon: 'receipt', value: me.payments_count ? String(me.payments_count) : undefined, to: '/app/payments' });
  rows.push({ label: 'Инструкции', icon: 'book', to: '/app/connect' });
  if (support) {
    rows.push({ label: 'Поддержка', icon: 'chat', run: () => openExternal(support) });
  }

  return (
    <main className="mn-page home">
      <header className="mn-head">
        <BrandMark name={site.brand_name} logo={site.logo_url} className="mn-logo" />
        <h1 className="mn-brand">{site.brand_name.toUpperCase()}</h1>
        {me.balance_enabled !== false || me.balance.minor > 0 ? (
          <Link to={me.balance_enabled !== false ? '/app/topup' : '/app/plans'} className="mn-balance press" aria-label={me.balance_enabled !== false ? `Баланс ${money(me.balance)}, пополнить` : `Остаток на балансе ${money(me.balance)}`}>
            <span>{money(me.balance)}</span>
          </Link>
        ) : null}
        <ThemeCirc />
      </header>

      <MiniSubSwitch />

      <section aria-label="Подписка" className="mn-sub">
        <div className="mn-ring" role="img" aria-label={expired && expire ? `Подписка закончилась ${dateLong(expire, false)}` : `Осталось ${left} из ${total}`}>
          <svg viewBox="0 0 208 208" width="208" height="208" aria-hidden="true">
            <circle cx="104" cy="104" r={R} fill="none" stroke="var(--track)" strokeWidth="12" />
            {share > 0 ? (
              <circle cx="104" cy="104" r={R} fill="none" stroke="var(--accent)" strokeWidth="12" strokeLinecap="round" strokeDasharray={LEN} strokeDashoffset={LEN * (1 - share)} transform="rotate(-90 104 104)" />
            ) : null}
          </svg>
          <span className="mn-ring-in">
            <strong>{view && !expired ? left : 0}</strong>
            <span>{plural(view && !expired ? left : 0, 'день остался', 'дня осталось', 'дней осталось')}</span>
          </span>
        </div>
        <span className="mn-lines">
          <strong>{view ? `${name} · ${expire ? (expired ? `закончилась ${dateLong(expire, false)}` : `до ${dateLong(expire, false)}`) : ''}` : 'Выберите тариф'}</strong>
          <span className={`mn-tone ${line2.tone}`}>{line2.text}</span>
        </span>
      </section>

      {meter ? (
        <section aria-label="Трафик" className="mn-traffic">
          <span className="mn-traffic-top">
            <span className="mn-traffic-name">Трафик</span>
            <strong className={tight ? 'warn' : undefined}>{meter.exhausted ? 'закончился' : `осталось ${meter.left}`}</strong>
          </span>
          <span
            className={tight ? 'mn-traffic-bar warn' : 'mn-traffic-bar'}
            role="progressbar"
            aria-label="Израсходовано трафика"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(meter.share * 100)}
            aria-valuetext={`${meter.used} из ${meter.limit}`}
          >
            <span style={{ width: `${meter.share > 0 ? Math.max(meter.share * 100, 2) : 0}%` }} />
          </span>
          <span className="mn-traffic-sub">
            {meter.used} из {meter.limit} · {meter.tail}
          </span>
          {tight ? (
            <Link to="/app/addons?kind=traffic" className="mn-link press">
              Докупить трафик
            </Link>
          ) : null}
        </section>
      ) : null}

      {button(main, 'mn-main press', 20)}
      {view ? (
        <div className="mn-pair">
          {button(left2, 'mn-pairbtn press', 18)}
          <Link to="/app/connect" className="mn-pairbtn press">
            <Icon name="phone" size={18} />
            Устройства {devCount}
            {limit ? `/${limit}` : ''}
          </Link>
        </div>
      ) : null}

      <nav aria-label="Ещё" className="mn-list">
        {rows.map((r) =>
          r.to ? (
            <Link key={r.label} to={r.to} className="mn-row press">
              <Icon name={r.icon} size={21} />
              <span className="mn-row-name">{r.label}</span>
              {r.value ? <span className="mn-row-val num">{r.value}</span> : null}
              <Icon name="chevron-right" size={16} />
            </Link>
          ) : (
            <button key={r.label} type="button" className="mn-row press" onClick={r.run}>
              <Icon name={r.icon} size={21} />
              <span className="mn-row-name">{r.label}</span>
              {r.value ? <span className="mn-row-val num">{r.value}</span> : null}
              <Icon name="chevron-right" size={16} />
            </button>
          ),
        )}
      </nav>
    </main>
  );
}
