import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, Route, Routes, useLocation } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { avatarColor, money } from '../lib/format';
import { Icon } from '../lib/icons';
import { usePhone } from '../lib/media';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { Site } from '../lib/types';
import { Login } from './Login';
import type { ClientMe } from './model';
import { Pay } from './Pay';
import { SecurityPage } from './Security';
import { SubscriptionPage } from './Subscription';
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
            <Route path="security" element={<SecurityPage />} />
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
  { key: 'devices', label: 'Устройства', to: '/cabinet#devices' },
  { key: 'friends', label: 'Друзья', to: '/cabinet#friends' },
  { key: 'help', label: 'Помощь', to: '/cabinet#help' },
];

function currentTab(path: string, hash: string): string {
  if (path.startsWith('/cabinet/security')) {
    return 'security';
  }
  if (path.startsWith('/cabinet/pay')) {
    return 'pay';
  }
  if (hash === '#devices') {
    return 'devices';
  }
  if (hash === '#friends') {
    return 'friends';
  }
  if (hash === '#help') {
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
  const tab = currentTab(location.pathname, location.hash);
  return (
    <header className="cb-head">
      <div className="cb-head-in">
        <Brand />
        <nav aria-label="Кабинет" className="cb-tabs">
          {TABS.map((t) => (
            <Link key={t.key} to={t.to} className="cb-tab hov" aria-current={tab === t.key ? 'page' : undefined}>
              {t.label}
            </Link>
          ))}
        </nav>
        <div className="cb-head-end">
          <Link to="/cabinet/pay" className="cb-balance hov" aria-label={`Баланс ${money(me.balance)}, пополнить`}>
            <span>{money(me.balance)}</span>
            <span className="cb-accent">Пополнить</span>
          </Link>
          <ThemeButton className="cb-icon hov" />
          <div className="cb-prof" ref={ref}>
            <button type="button" className="cb-ava" aria-label="Профиль" aria-expanded={open} onClick={() => setOpen((v) => !v)} style={{ background: avatarColor(me.client.id) }}>
              <Icon name="user" size={20} />
              {photo ? <img className="cb-ava-img" src="/api/client/photo" alt="" decoding="async" onError={() => setPhoto(false)} /> : null}
            </button>
            {open ? (
              <div className="cb-pop" role="menu">
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
  const tab = currentTab(location.pathname, location.hash);
  return (
    <header className="cb-phead">
      <Brand small />
      <span className="cb-grow" />
      <Link to="/cabinet/pay" className="cb-chip" aria-label={`Баланс ${money(me.balance)}, пополнить`}>
        <span>{money(me.balance)}</span>
      </Link>
      <ThemeButton className="cb-circ" />
      <button type="button" className="cb-circ" aria-label="Меню" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <Icon name={open ? 'close' : 'menu'} size={20} />
      </button>
      {open ? (
        <nav aria-label="Разделы кабинета" className="cb-menu">
          {TABS.map((t) => (
            <Link key={t.key} to={t.to} className="cb-menu-item" aria-current={tab === t.key ? 'page' : undefined}>
              {t.label}
            </Link>
          ))}
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
  return (
    <footer className="cb-foot">
      {site.support_url ? <a href={site.support_url}>Помощь</a> : <Link to="/cabinet#help">Помощь</Link>}
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
