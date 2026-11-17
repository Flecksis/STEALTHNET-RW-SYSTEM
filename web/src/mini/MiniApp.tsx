import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import { Link, Route, Routes, useLocation } from 'react-router';
import { api, ApiError, setBearer } from '../lib/api';
import { Icon, type IconName } from '../lib/icons';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { Site } from '../lib/types';
import type { ClientMe, Device, SubscriptionView } from '../cabinet/model';
import { initData, loadTelegram } from './telegram';
import { MiniConnect } from './MiniConnect';
import { MiniHome } from './MiniHome';
import { MiniPlans } from './MiniPlans';
import { MiniProfile } from './MiniProfile';
import './mini.css';

const TOKEN_KEY = 'sn.mini.token';
const THEME_KEY = 'sn.mini.theme';

type Login = { token: string };

export type MiniCtx = {
  me: ClientMe;
  site: Site;
  view: SubscriptionView | null;
  devices: Device[] | null;
  setView: (view: SubscriptionView | null) => void;
  setDevices: (devices: Device[] | null) => void;
  reload: () => Promise<void>;
  theme: Theme;
  flipTheme: (() => void) | null;
};

const Ctx = createContext<MiniCtx | null>(null);

export function useMini(): MiniCtx {
  const value = useContext(Ctx);
  if (!value) {
    throw new Error('нет мини-приложения');
  }
  return value;
}

function stored(): string | null {
  try {
    return window.sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

function remember(token: string | null) {
  try {
    if (token) {
      window.sessionStorage.setItem(TOKEN_KEY, token);
    } else {
      window.sessionStorage.removeItem(TOKEN_KEY);
    }
  } catch {
    return;
  }
}

type State = { kind: 'loading' } | { kind: 'outside' } | { kind: 'blocked' } | { kind: 'error'; message: string } | { kind: 'in'; me: ClientMe };

export function MiniApp() {
  const site = useSite() ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const [state, setState] = useState<State>({ kind: 'loading' });
  const [view, setView] = useState<SubscriptionView | null>(null);
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [tgScheme, setTgScheme] = useState<Theme | null>(null);
  const [personal, setPersonal] = useState<Theme | null>(() => {
    const v = readStored(THEME_KEY);
    return v === 'dark' || v === 'light' ? v : null;
  });

  const loadData = useCallback(async () => {
    const me = await api<ClientMe>('/api/client/me');
    const list = await api<SubscriptionView[]>('/api/client/subscriptions');
    const live = list.find((v) => !v.subscription.ended_at) ?? null;
    setState({ kind: 'in', me });
    setView(live);
    if (live) {
      const [detail, devs] = await Promise.all([
        api<SubscriptionView>(`/api/client/subscriptions/${live.subscription.id}`).catch(() => live),
        api<Device[]>(`/api/client/subscriptions/${live.subscription.id}/devices`).catch(() => null),
      ]);
      setView(detail);
      setDevices(devs);
    } else {
      setDevices(null);
    }
  }, []);

  const start = useCallback(async () => {
    const app = await loadTelegram();
    app?.ready();
    app?.expand?.();
    if (app?.colorScheme) {
      setTgScheme(app.colorScheme);
    }
    try {
      const data = initData();
      if (data) {
        const login = await api<Login>('/api/client/auth/miniapp', { method: 'POST', body: { init_data: data } });
        setBearer(login.token);
        remember(login.token);
      } else {
        const token = stored();
        if (!token) {
          setState({ kind: 'outside' });
          return;
        }
        setBearer(token);
      }
      await loadData();
    } catch (err) {
      if (err instanceof ApiError && err.status === 403) {
        setState({ kind: 'blocked' });
      } else if (err instanceof ApiError && err.status === 401) {
        remember(null);
        setState({ kind: 'outside' });
      } else {
        setState({ kind: 'error', message: err instanceof ApiError ? err.message : 'Сервер не ответил' });
      }
    }
  }, [loadData]);

  useEffect(() => {
    void start();
  }, [start]);

  const choice = site.appearance.client_theme;
  const fixed = choice === 'dark' || choice === 'light' ? choice : null;
  const theme: Theme = fixed ?? personal ?? tgScheme ?? system;
  useSurface('mini', theme, site.appearance.client === 'mono' ? 'b' : 'a');

  const flipTheme = useCallback(() => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    writeStored(THEME_KEY, next);
    setPersonal(next);
  }, [theme]);

  const reload = useCallback(async () => {
    try {
      await loadData();
    } catch {
      return;
    }
  }, [loadData]);

  const ctx = useMemo<MiniCtx | null>(
    () => (state.kind === 'in' ? { me: state.me, site, view, devices, setView, setDevices, reload, theme, flipTheme: fixed ? null : flipTheme } : null),
    [state, site, view, devices, reload, theme, flipTheme, fixed],
  );

  if (state.kind === 'loading') {
    return <div className="mn-blank" aria-busy="true" />;
  }
  if (!ctx) {
    const text =
      state.kind === 'outside'
        ? 'Откройте мини-приложение из бота в Telegram.'
        : state.kind === 'blocked'
          ? 'Аккаунт заблокирован. Напишите в поддержку.'
          : state.kind === 'error'
            ? state.message
            : '';
    return (
      <div className="mn mn-solo">
        <span className="mn-logo" aria-hidden="true">
          S
        </span>
        <h1>{site.brand_name}</h1>
        <p>{text}</p>
        {state.kind === 'error' ? (
          <button type="button" className="mn-main" onClick={() => void start()}>
            Повторить
          </button>
        ) : null}
        {state.kind === 'outside' && site.bot_username ? (
          <a className="mn-main" href={`https://t.me/${site.bot_username.replace(/^@/, '')}`}>
            Открыть бота
          </a>
        ) : null}
      </div>
    );
  }

  return (
    <Ctx.Provider value={ctx}>
      <div className="mn">
        <Routes>
          <Route index element={<MiniHome />} />
          <Route path="plans" element={<MiniPlans />} />
          <Route path="connect" element={<MiniConnect />} />
          <Route path="profile" element={<MiniProfile />} />
          <Route path="*" element={<MiniHome />} />
        </Routes>
        <MiniNav />
      </div>
    </Ctx.Provider>
  );
}

const TABS: Array<{ to: string; label: string; icon: IconName }> = [
  { to: '/app', label: 'Главная', icon: 'home' },
  { to: '/app/plans', label: 'Тарифы', icon: 'tag' },
  { to: '/app/connect', label: 'Подключить', icon: 'zap' },
  { to: '/app/profile', label: 'Профиль', icon: 'user' },
];

function MiniNav() {
  const { pathname } = useLocation();
  return (
    <nav aria-label="Меню" className="mn-nav">
      {TABS.map((t) => {
        const current = t.to === '/app' ? pathname === '/app' || pathname === '/app/' : pathname.startsWith(t.to);
        return (
          <Link key={t.to} to={t.to} className="mn-tab press" aria-current={current ? 'page' : undefined}>
            <Icon name={t.icon} size={23} />
            <span>{t.label}</span>
          </Link>
        );
      })}
    </nav>
  );
}

export function ThemeCirc() {
  const { theme, flipTheme } = useMini();
  if (!flipTheme) {
    return null;
  }
  const label = theme === 'dark' ? 'Светлая тема' : 'Тёмная тема';
  return (
    <button type="button" className="mn-circ press" aria-label={label} onClick={flipTheme}>
      <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={19} />
    </button>
  );
}

export function BackHead({ title }: { title: string }) {
  return (
    <header className="mn-head">
      <Link to="/app" className="mn-circ press back" aria-label="Назад">
        <Icon name="chevron-left" size={20} />
      </Link>
      <h1 className="mn-h1">{title}</h1>
      <ThemeCirc />
    </header>
  );
}

