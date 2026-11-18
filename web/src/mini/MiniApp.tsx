import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Link, Route, Routes, useLocation, useNavigate } from 'react-router';
import { api, ApiError, setBearer } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { applyLang, guessLang, msg, otherLang, rememberLang, siteLangs, t, useApplied, useLang, type Lang } from '../lib/i18n';
import { Icon, type IconName } from '../lib/icons';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { Site } from '../lib/types';
import type { ClientMe, Device, SubscriptionView } from '../cabinet/model';
import { liveSubs, pickSub, readRemembered, remember as rememberSub, subName, subState, useShowPressed } from '../cabinet/subs';
import { initData, loadTelegram, telegramLanguage, webApp } from './telegram';
import { backTarget } from './back';
import { MiniConnect } from './MiniConnect';
import { MiniHome } from './MiniHome';
import { MiniPayments } from './MiniPayments';
import { MiniAddons } from './MiniAddons';
import { MiniGifts } from './MiniGifts';
import { MiniPlans } from './MiniPlans';
import { MiniProfile } from './MiniProfile';
import { MiniPromo } from './MiniPromo';
import { MiniEmail, MiniLanguage, MiniPassword, MiniTotp } from './MiniAccount';
import { MiniTopUp } from './MiniTopUp';
import { MiniRoulette } from './MiniRoulette';
import { MiniWithdraw } from './MiniWithdraw';
import { MiniSupport, MiniTicket } from './MiniSupport';
import './mini.css';

const TOKEN_KEY = 'sn.mini.token';
const THEME_KEY = 'sn.mini.theme';
export const MINI_SUB_KEY = 'sn.mini.sub';

type Login = { token: string };

type Detail = { view: SubscriptionView; devices: Device[] | null };

export type MiniCtx = {
  me: ClientMe;
  site: Site;
  subs: SubscriptionView[];
  view: SubscriptionView | null;
  devices: Device[] | null;
  ready: boolean;
  choose: (id: number) => void;
  setView: (view: SubscriptionView) => void;
  setDevices: (devices: Device[]) => void;
  reload: () => Promise<void>;
  theme: Theme;
  flipTheme: (() => void) | null;
  lang: Lang;
  nextLang: Lang | null;
  switchLang: (next: Lang) => void;
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
  const loadedSite = useSite();
  const site = loadedSite ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const [state, setState] = useState<State>({ kind: 'loading' });
  const [subs, setSubs] = useState<SubscriptionView[]>([]);
  const [chosenId, setChosenId] = useState<number | null>(null);
  const cache = useRef(new Map<number, Detail>());
  const [, setTick] = useState(0);
  const [tgScheme, setTgScheme] = useState<Theme | null>(null);
  const [personal, setPersonal] = useState<Theme | null>(() => {
    const v = readStored(THEME_KEY);
    return v === 'dark' || v === 'light' ? v : null;
  });
  const [picked, setPicked] = useState<Lang | null>(null);
  const langs = siteLangs(loadedSite);
  const account = state.kind === 'in' && langs.includes(state.me.locale as Lang) ? (state.me.locale as Lang) : null;
  const wanted: Lang | null =
    loadedSite && state.kind !== 'loading' ? (picked && langs.includes(picked) ? picked : (account ?? guessLang(loadedSite, telegramLanguage()))) : null;
  const applied = useApplied(wanted);
  const lang = useLang();

  const fetchDetail = useCallback(async (live: SubscriptionView) => {
    const id = live.subscription.id;
    const [view, devices] = await Promise.all([
      api<SubscriptionView>(`/api/client/subscriptions/${id}`).catch(() => live),
      api<Device[]>(`/api/client/subscriptions/${id}/devices`).catch(() => null),
    ]);
    cache.current.set(id, { view, devices });
    setTick((n) => n + 1);
  }, []);

  const loadData = useCallback(async () => {
    const me = await api<ClientMe>('/api/client/me');
    const list = liveSubs(await api<SubscriptionView[]>('/api/client/subscriptions'));
    const chosen = pickSub(list, readRemembered(MINI_SUB_KEY));
    setState({ kind: 'in', me });
    setSubs(list);
    setChosenId(chosen?.subscription.id ?? null);
    if (chosen) {
      await fetchDetail(chosen);
    }
  }, [fetchDetail]);

  const choose = useCallback(
    (id: number) => {
      const next = subs.find((v) => v.subscription.id === id);
      if (!next) {
        return;
      }
      rememberSub(MINI_SUB_KEY, id);
      setChosenId(id);
      void fetchDetail(next);
    },
    [subs, fetchDetail],
  );

  const base = subs.find((v) => v.subscription.id === chosenId) ?? null;
  const hit = chosenId === null ? undefined : cache.current.get(chosenId);

  const putDetail = useCallback(
    (patch: Partial<Detail>) => {
      if (!base) {
        return;
      }
      const id = base.subscription.id;
      cache.current.set(id, { ...(cache.current.get(id) ?? { view: base, devices: null }), ...patch });
      setTick((n) => n + 1);
    },
    [base],
  );
  const setView = useCallback((next: SubscriptionView) => putDetail({ view: next }), [putDetail]);
  const setDevices = useCallback((next: Device[]) => putDetail({ devices: next }), [putDetail]);

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
        setState({ kind: 'error', message: err instanceof ApiError ? err.message : t('Сервер не ответил') });
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

  const switchLang = useCallback(
    (next: Lang) => {
      rememberLang(next);
      setPicked(next);
      void applyLang(next);
      api('/api/client/locale', { method: 'PUT', body: { locale: next } })
        .then(() => reload())
        .catch(() => undefined);
    },
    [reload],
  );

  const view = hit?.view ?? base;
  const devices = hit?.devices ?? null;
  const ready = hit !== undefined;
  const nextLang = otherLang(lang, langs);
  const ctx = useMemo<MiniCtx | null>(
    () =>
      state.kind === 'in'
        ? { me: state.me, site, subs, view, devices, ready, choose, setView, setDevices, reload, theme, flipTheme: fixed ? null : flipTheme, lang, nextLang, switchLang }
        : null,
    [state, site, subs, view, devices, ready, choose, setView, setDevices, reload, theme, flipTheme, fixed, lang, nextLang, switchLang],
  );

  if (state.kind === 'loading' || !applied) {
    return <div className="mn-blank" aria-busy="true" />;
  }
  if (!ctx) {
    const text =
      state.kind === 'outside'
        ? t('Откройте мини-приложение из бота в Telegram.')
        : state.kind === 'blocked'
          ? t('Аккаунт заблокирован. Напишите в поддержку.')
          : state.kind === 'error'
            ? state.message
            : '';
    return (
      <div className="mn mn-solo">
        <BrandMark name={site.brand_name} logo={site.logo_url} className="mn-logo" />
        <h1>{site.brand_name}</h1>
        <p>{text}</p>
        {state.kind === 'error' ? (
          <button type="button" className="mn-main" onClick={() => void start()}>
            {t('Повторить')}
          </button>
        ) : null}
        {state.kind === 'outside' && site.bot_username ? (
          <a className="mn-main" href={`https://t.me/${site.bot_username.replace(/^@/, '')}`}>
            {t('Открыть бота')}
          </a>
        ) : null}
      </div>
    );
  }

  return (
    <Ctx.Provider value={ctx}>
      <div key={lang} className="mn">
        <Routes>
          <Route index element={<MiniHome />} />
          <Route path="plans" element={<MiniPlans />} />
          <Route path="connect" element={<MiniConnect />} />
          <Route path="profile" element={<MiniProfile />} />
          <Route path="profile/email" element={<MiniEmail />} />
          <Route path="profile/password" element={<MiniPassword />} />
          <Route path="profile/totp" element={<MiniTotp />} />
          <Route path="profile/language" element={<MiniLanguage />} />
          <Route path="topup" element={<MiniTopUp />} />
          <Route path="payments" element={<MiniPayments />} />
          <Route path="addons" element={<MiniAddons />} />
          <Route path="gifts" element={<MiniGifts />} />
          <Route path="promo" element={<MiniPromo />} />
          <Route path="roulette" element={<MiniRoulette />} />
          <Route path="withdraw" element={<MiniWithdraw />} />
          <Route path="support" element={<MiniSupport />} />
          <Route path="support/:id" element={<MiniTicket />} />
          <Route path="*" element={<MiniHome />} />
        </Routes>
        <MiniNav />
        <TelegramBack />
      </div>
    </Ctx.Provider>
  );
}

function TelegramBack() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  useEffect(() => {
    const back = webApp()?.BackButton;
    if (!back) {
      return;
    }
    const target = backTarget(pathname);
    if (!target) {
      back.hide();
      return;
    }
    const go = () => {
      const idx = (window.history.state as { idx?: number } | null)?.idx ?? 0;
      if (idx > 0) {
        navigate(-1);
      } else {
        navigate(target, { replace: true });
      }
    };
    back.onClick(go);
    back.show();
    return () => back.offClick(go);
  }, [pathname, navigate]);
  useEffect(() => () => webApp()?.BackButton?.hide(), []);
  return null;
}

const TABS: Array<{ to: string; label: string; icon: IconName }> = [
  { to: '/app', label: msg('Главная'), icon: 'home' },
  { to: '/app/plans', label: msg('Тарифы'), icon: 'tag' },
  { to: '/app/connect', label: msg('Подключить'), icon: 'zap' },
  { to: '/app/profile', label: msg('Профиль'), icon: 'user' },
];

function MiniNav() {
  const { pathname } = useLocation();
  return (
    <nav aria-label={t('Меню')} className="mn-nav">
      {TABS.map((tab) => {
        const current = tab.to === '/app' ? ['/app', '/app/', '/app/topup', '/app/payments', '/app/roulette', '/app/promo'].includes(pathname) : pathname.startsWith(tab.to);
        return (
          <Link key={tab.to} to={tab.to} className="mn-tab press" aria-current={current ? 'page' : undefined}>
            <Icon name={tab.icon} size={23} />
            <span>{t(tab.label)}</span>
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
  const label = theme === 'dark' ? t('Светлая тема') : t('Тёмная тема');
  return (
    <button type="button" className="mn-circ press" aria-label={label} onClick={flipTheme}>
      <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={19} />
    </button>
  );
}

export function BackHead({ title, to = '/app' }: { title: string; to?: string }) {
  return (
    <header className="mn-head">
      <Link to={to} className="mn-circ press back" aria-label={t('Назад')}>
        <Icon name="chevron-left" size={20} />
      </Link>
      <h1 className="mn-h1">{title}</h1>
      <ThemeCirc />
    </header>
  );
}

export function MiniSubSwitch() {
  const { me, subs, view, choose } = useMini();
  const box = useRef<HTMLDivElement>(null);
  const chosenId = view?.subscription.id ?? null;
  useShowPressed(box, `${chosenId}:${subs.length}`);
  if (subs.length < 2) {
    return null;
  }
  const blocked = me.client.status === 'blocked';
  return (
    <div ref={box} role="group" aria-label={t('Подписки')} className="mn-subs">
      {subs.map((v) => {
        const on = v.subscription.id === chosenId;
        const state = subState(v, blocked);
        return (
          <button key={v.subscription.id} type="button" className="mn-subs-item press" aria-pressed={on} onClick={() => !on && choose(v.subscription.id)}>
            <strong>{subName(v, subs)}</strong>
            <span className={`mn-tone ${state.tone}`}>{state.text}</span>
          </button>
        );
      })}
    </div>
  );
}
