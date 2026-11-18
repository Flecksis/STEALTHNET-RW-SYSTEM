import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, Route, Routes, useLocation } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { avatarColor, money } from '../lib/format';
import { Icon } from '../lib/icons';
import { usePhone } from '../lib/media';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { supportLink, supportTickets } from '../lib/support';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { Site } from '../lib/types';
import { AddonsPage } from './Addons';
import { DevicesPage } from './Devices';
import { FriendsPage } from './Friends';
import { GiftsPage } from './Gifts';
import { HelpPage, type TicketBrief } from './Help';
import { Login } from './Login';
import type { ClientMe } from './model';
import { Pay } from './Pay';
import { PaymentsHistory } from './PaymentHistory';
import { SecurityPage } from './Security';
import { SubscriptionPage } from './Subscription';
import { TopUpPage } from './TopUp';
import './cabinet.css';

const THEME_KEY = 'sn.client.theme';

type State = { kind: 'loading' } | { kind: 'guest' } | { kind: 'blocked' } | { kind: 'in'; me: ClientMe } | { kind: 'error'; message: string };

export type CabinetCtx = {
  me: ClientMe;
  site: Site;
  reload: () => Promise<void>;
  logout: () => Promise<void>;
  theme: Theme;
  flipTheme: () => void;
  phone: boolean;
};

const Ctx = createContext<CabinetCtx | null>(null);

export function useCabinet(): CabinetCtx {
  const value = useContext(Ctx);
  if (!value) {
    throw new Error('нет кабинета');
  }
  return value;
}

function storedTheme(): Theme | null {
  const value = readStored(THEME_KEY);
  return value === 'dark' || value === 'light' ? value : null;
}

export function CabinetApp() {
  const site = useSite() ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const phone = usePhone();
  const [state, setState] = useState<State>({ kind: 'loading' });
  const [personal, setPersonal] = useState<Theme | null>(storedTheme);

  const load = useCallback(async () => {
    try {
      const me = await api<ClientMe>('/api/client/me');
      setState(me.client.status === 'blocked' ? { kind: 'blocked' } : { kind: 'in', me });
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        setState({ kind: 'guest' });
      } else if (err instanceof ApiError && err.status === 403) {
        setState({ kind: 'blocked' });
      } else {
        setState({ kind: 'error', message: err instanceof ApiError ? err.message : 'Сервер не ответил' });
      }
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const choice = site.appearance.client_theme;
  const fixed = choice === 'dark' || choice === 'light' ? choice : null;
  const theme: Theme = fixed ?? personal ?? system;
  useSurface(phone ? 'cabinet-phone' : 'cabinet', theme, site.appearance.client === 'mono' ? 'b' : 'a');

  const flipTheme = useCallback(() => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    writeStored(THEME_KEY, next);
    setPersonal(next);
  }, [theme]);

  const logout = useCallback(async () => {
    try {
      await api('/api/client/auth/logout', { method: 'POST', body: {} });
    } finally {
      setState({ kind: 'guest' });
    }
  }, []);

  const ctx = useMemo<CabinetCtx | null>(
    () => (state.kind === 'in' ? { me: state.me, site, reload: load, logout, theme, flipTheme: fixed ? () => undefined : flipTheme, phone } : null),
    [state, site, load, logout, theme, flipTheme, fixed, phone],
  );

  if (state.kind === 'loading') {
    return <div className="cb-blank" aria-busy="true" />;
  }
  if (state.kind === 'guest') {
    return <Login site={site} onDone={() => void load()} />;
  }
  if (state.kind === 'blocked') {
    return (
      <div className="cb-solo">
        <div className="cb-solo-box">
          <h1>Доступ закрыт</h1>
          <p>Аккаунт заблокирован. Если это ошибка, напишите в поддержку.</p>
          {site.support_url ? (
            <a className="cb-main" href={site.support_url}>
              Написать в поддержку
            </a>
          ) : null}
        </div>
      </div>
    );
  }
  if (state.kind === 'error' || !ctx) {
    return (
      <div className="cb-solo">
        <div className="cb-solo-box" role="alert">
          <h1>Кабинет не открылся</h1>
          <p>{state.kind === 'error' ? state.message : 'Сервер не ответил'}</p>
          <button type="button" className="cb-main" onClick={() => void load()}>
            Повторить
          </button>
        </div>
      </div>
    );
  }

  return (
    <Ctx.Provider value={ctx}>
      <div className={phone ? 'cb cb-phone' : 'cb'}>
        {phone ? <PhoneHeader /> : <Header />}
        <main className="cb-main-area">
          <Routes>
            <Route index element={<SubscriptionPage />} />
            <Route path="pay" element={<Pay />} />
            <Route path="topup" element={<TopUpPage />} />
            <Route path="addons" element={<AddonsPage />} />
            <Route path="payments" element={<PaymentsHistory />} />
            <Route path="security" element={<SecurityPage />} />
            <Route path="devices" element={<DevicesPage />} />
            <Route path="friends" element={<FriendsPage />} />
            <Route path="gifts" element={<GiftsPage />} />
            <Route path="help" element={<HelpPage />} />
            <Route path="help/:id" element={<HelpPage />} />
            <Route path="*" element={<SubscriptionPage />} />
          </Routes>
        </main>
        <Footer />
      </div>
    </Ctx.Provider>
  );
}

const TABS: Array<{ key: string; label: string; to: string }> = [
  { key: 'sub', label: 'Подписка', to: '/cabinet' },
  { key: 'pay', label: 'Тарифы', to: '/cabinet/pay' },
  { key: 'devices', label: 'Устройства', to: '/cabinet/devices' },
  { key: 'friends', label: 'Друзья', to: '/cabinet/friends' },
  { key: 'gifts', label: 'Подарки', to: '/cabinet/gifts' },
  { key: 'help', label: 'Помощь', to: '/cabinet/help' },
];

let referralKnown: boolean | null = null;
let giftsKnown: boolean | null = null;

function useProbe(path: string, known: () => boolean | null, remember: (value: boolean) => void): boolean | null {
  const [value, setValue] = useState<boolean | null>(known());
  useEffect(() => {
    if (known() !== null) {
      return;
    }
    let alive = true;
    api(path)
      .then(() => {
        remember(true);
        if (alive) {
          setValue(true);
        }
      })
      .catch(() => {
        remember(false);
        if (alive) {
          setValue(false);
        }
      });
    return () => {
      alive = false;
    };
  }, [path, known, remember]);
  return value;
}

const knowReferral = () => referralKnown;
const rememberReferral = (value: boolean) => {
  referralKnown = value;
};
const knowGifts = () => giftsKnown;
const rememberGifts = (value: boolean) => {
  giftsKnown = value;
};

function useTabs() {
  const { site } = useCabinet();
  const referral = useProbe('/api/client/m/referral/me', knowReferral, rememberReferral);
  const gifts = useProbe('/api/client/m/gifts/offers', knowGifts, rememberGifts);
  const help = supportTickets(site) || supportLink(site) !== null;
  return TABS.filter((t) => (t.key === 'friends' ? referral === true : t.key === 'gifts' ? gifts === true : t.key === 'help' ? help : true));
}

function useAnswered(): number {
  const { site } = useCabinet();
  const location = useLocation();
  const [answered, setAnswered] = useState(0);
  const on = supportTickets(site);
  useEffect(() => {
    if (!on) {
      setAnswered(0);
      return;
    }
    let alive = true;
    const check = () => {
      if (document.visibilityState !== 'visible') {
        return;
      }
      api<TicketBrief[]>('/api/client/support/tickets')
        .then((list) => {
          if (alive) {
            setAnswered(list.filter((t) => t.status === 'open' && t.last_author !== 'client').length);
          }
        })
        .catch(() => undefined);
    };
    check();
    const timer = window.setInterval(check, 60_000);
    document.addEventListener('visibilitychange', check);
    return () => {
      alive = false;
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', check);
    };
  }, [on, location.pathname]);
  return answered;
}

function TabLabel({ tab, answered }: { tab: { key: string; label: string }; answered: number }) {
  if (tab.key !== 'help' || answered === 0) {
    return <>{tab.label}</>;
  }
  return (
    <>
      {tab.label}
      <span className="cb-tabdot" aria-label={answered === 1 ? ', есть ответ поддержки' : `, ответов поддержки: ${answered}`} />
    </>
  );
}

function currentTab(path: string): string {
  if (path.startsWith('/cabinet/security')) {
    return 'security';
  }
  if (path.startsWith('/cabinet/topup') || path.startsWith('/cabinet/payments')) {
    return 'money';
  }
  if (path.startsWith('/cabinet/pay')) {
    return 'pay';
  }
  if (path.startsWith('/cabinet/devices')) {
    return 'devices';
  }
  if (path.startsWith('/cabinet/friends')) {
    return 'friends';
  }
  if (path.startsWith('/cabinet/gifts')) {
    return 'gifts';
  }
  if (path.startsWith('/cabinet/help')) {
    return 'help';
  }
  return 'sub';
}

function ThemeButton({ className }: { className: string }) {
  const { theme, flipTheme, site } = useCabinet();
  const choice = site.appearance.client_theme;
  if (choice === 'dark' || choice === 'light') {
    return null;
  }
  const label = theme === 'dark' ? 'Светлая тема' : 'Тёмная тема';
  return (
    <button type="button" className={className} aria-label={label} title={label} onClick={flipTheme}>
      <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={20} />
    </button>
  );
}

function Brand({ small }: { small?: boolean }) {
  const { site } = useCabinet();
  return (
    <Link to="/" className={small ? 'cb-brand small' : 'cb-brand'} aria-label={`${site.brand_name}, на главную`}>
      <BrandMark name={site.brand_name} logo={site.logo_url} className="cb-mark" />
      <span className="cb-brand-name">{site.brand_name.toUpperCase()}</span>
    </Link>
  );
}

function Header() {
  const { me, logout } = useCabinet();
  const location = useLocation();
  const [open, setOpen] = useState(false);
  const [photo, setPhoto] = useState(true);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);
  const tab = currentTab(location.pathname);
  const tabs = useTabs();
  const answered = useAnswered();
  return (
    <header className="cb-head">
      <div className="cb-head-in">
        <Brand />
        <nav aria-label="Кабинет" className="cb-tabs">
          {tabs.map((t) => (
            <Link key={t.key} to={t.to} className="cb-tab hov" aria-current={tab === t.key ? 'page' : undefined}>
              <TabLabel tab={t} answered={answered} />
            </Link>
          ))}
        </nav>
        <div className="cb-head-end">
          {me.balance_enabled !== false ? (
            <Link to="/cabinet/topup" className="cb-balance hov" aria-label={`Баланс ${money(me.balance)}, пополнить`}>
              <span>{money(me.balance)}</span>
              <span className="cb-accent">Пополнить</span>
            </Link>
          ) : me.balance.minor > 0 ? (
            <Link to="/cabinet/pay" className="cb-balance hov" aria-label={`Остаток на балансе ${money(me.balance)}`}>
              <span>{money(me.balance)}</span>
            </Link>
          ) : null}
          <ThemeButton className="cb-icon hov" />
          <div className="cb-prof" ref={ref}>
            <button type="button" className="cb-ava" aria-label="Профиль" aria-expanded={open} onClick={() => setOpen((v) => !v)} style={{ background: avatarColor(me.client.id) }}>
              <Icon name="user" size={20} />
              {photo ? <img className="cb-ava-img" src="/api/client/photo" alt="" decoding="async" onError={() => setPhoto(false)} /> : null}
            </button>
            {open ? (
              <div className="cb-pop" role="menu">
                <Link to="/cabinet/payments" role="menuitem" className="cb-pop-item plain hov" onClick={() => setOpen(false)}>
                  <Icon name="receipt" size={18} />
                  История оплат
                </Link>
                <Link to="/cabinet/security" role="menuitem" className="cb-pop-item plain hov" onClick={() => setOpen(false)}>
                  <Icon name="sliders" size={18} />
                  Безопасность
                </Link>
                <button type="button" role="menuitem" className="cb-pop-item hov" onClick={() => void logout()}>
                  <Icon name="logout" size={18} />
                  Выйти
                </button>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </header>
  );
}

function PhoneHeader() {
  const { me, logout } = useCabinet();
  const location = useLocation();
  const [open, setOpen] = useState(false);
  useEffect(() => {
    setOpen(false);
  }, [location.pathname, location.hash]);
  const tab = currentTab(location.pathname);
  const tabs = useTabs();
  const answered = useAnswered();
  return (
    <header className="cb-phead">
      <Brand small />
      <span className="cb-grow" />
      {me.balance_enabled !== false || me.balance.minor > 0 ? (
        <Link to={me.balance_enabled !== false ? '/cabinet/topup' : '/cabinet/pay'} className="cb-chip" aria-label={me.balance_enabled !== false ? `Баланс ${money(me.balance)}, пополнить` : `Остаток на балансе ${money(me.balance)}`}>
          <span>{money(me.balance)}</span>
        </Link>
      ) : null}
      <ThemeButton className="cb-circ" />
      <button type="button" className="cb-circ cb-menu-btn" aria-label={answered > 0 ? 'Меню, есть ответ поддержки' : 'Меню'} aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <Icon name={open ? 'close' : 'menu'} size={20} />
        {answered > 0 && !open ? <span className="cb-tabdot on-btn" aria-hidden="true" /> : null}
      </button>
      {open ? (
        <nav aria-label="Разделы кабинета" className="cb-menu">
          {tabs.map((t) => (
            <Link
              key={t.key}
              to={t.to}
              className="cb-menu-item"
              aria-current={tab === t.key ? 'page' : undefined}
              onClick={() => setOpen(false)}
            >
              <TabLabel tab={t} answered={answered} />
            </Link>
          ))}
          <Link to="/cabinet/payments" className="cb-menu-item" aria-current={location.pathname.startsWith('/cabinet/payments') ? 'page' : undefined}>
            История оплат
          </Link>
          <Link to="/cabinet/security" className="cb-menu-item" aria-current={location.pathname.startsWith('/cabinet/security') ? 'page' : undefined}>
            Безопасность
          </Link>
          <button type="button" className="cb-menu-item" onClick={() => void logout()}>
            Выйти
          </button>
        </nav>
      ) : null}
    </header>
  );
}

function Footer() {
  const { site } = useCabinet();
  const link = supportLink(site);
  return (
    <footer className="cb-foot">
      {supportTickets(site) ? (
        <Link to="/cabinet/help">Помощь</Link>
      ) : link ? (
        <a href={link} target="_blank" rel="noreferrer">
          Помощь
        </a>
      ) : site.bot_username ? (
        <a href={`https://t.me/${site.bot_username}`} target="_blank" rel="noreferrer">
          Помощь
        </a>
      ) : null}
      {(site.documents ?? []).map((doc) => (
        <a key={doc.url} href={doc.url} target="_blank" rel="noreferrer noopener">
          {doc.title}
        </a>
      ))}
    </footer>
  );
}

export function Card({ id, className, children, label }: { id?: string; className?: string; children: ReactNode; label?: string }) {
  return (
    <section id={id} aria-label={label} className={className ? `cb-card ${className}` : 'cb-card'}>
      {children}
    </section>
  );
}
