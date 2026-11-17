import { useCallback, useEffect, useMemo, useState } from 'react';
import { Navigate, Route, Routes } from 'react-router';
import { api, ApiError } from '../lib/api';
import { usePhone } from '../lib/media';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { AdminTheme, Appearance, Me, Permission } from '../lib/types';
import { Clients } from './clients/Clients';
import { ClientPage } from './client/Client';
import { Tariffs } from './tariffs/Tariffs';
import { Home } from './home/Home';
import { Support } from './support/Support';
import { InfraNodes } from './infra/InfraNodes';
import { InfraSquads } from './infra/InfraSquads';
import { InfraBilling } from './infra/InfraBilling';
import { Modules } from './modules/Modules';
import { ModulePage } from './modules/ModulePage';
import { Login } from './Login';
import { Pending } from './pages/Pending';
import { SettingsLayout } from './pages/SettingsLayout';
import { SettingsLook } from './pages/SettingsLook';
import { SettingsRemna } from './pages/SettingsRemna';
import { SettingsPayments } from './pages/SettingsPayments';
import { SettingsGeneral } from './pages/SettingsGeneral';
import { SettingsBot } from './pages/SettingsBot';
import { SettingsBotImages } from './pages/SettingsBotImages';
import { SettingsBotTexts } from './pages/SettingsBotTexts';
import { SettingsDocuments } from './pages/SettingsDocuments';
import { SettingsUpdates } from './pages/SettingsUpdates';
import { SessionContext, type Session } from './session';
import { Shell } from './Shell';
import type { BuildSummary } from './version';

const THEME_KEY = 'sn.admin.theme';

type State = { kind: 'loading' } | { kind: 'guest' } | { kind: 'in'; me: Me } | { kind: 'error'; message: string };

function resolve(choice: AdminTheme, system: Theme): Theme {
  return choice === 'system' ? system : choice;
}

function storedTheme(): Theme | null {
  const value = readStored(THEME_KEY);
  return value === 'dark' || value === 'light' ? value : null;
}

export function AdminApp() {
  const site = useSite();
  const system = useSystemTheme();
  const [state, setState] = useState<State>({ kind: 'loading' });
  const [personal, setPersonal] = useState<Theme | null>(storedTheme);

  const load = useCallback(async () => {
    try {
      const me = await api<Me>('/api/admin/me');
      setState({ kind: 'in', me });
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        setState({ kind: 'guest' });
      } else {
        setState({ kind: 'error', message: err instanceof ApiError ? err.message : 'Сервер не ответил' });
      }
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const [supportBadge, setSupportBadge] = useState(0);
  const [build, setBuild] = useState<BuildSummary | null>(null);
  const signedIn = state.kind === 'in';
  useEffect(() => {
    if (!signedIn) {
      return;
    }
    let alive = true;
    const tick = () =>
      api<{ support: number }>('/api/admin/badges')
        .then((b) => alive && setSupportBadge(b.support))
        .catch(() => undefined);
    const onShow = () => {
      if (document.visibilityState === 'visible') {
        void tick();
      }
    };
    void tick();
    const timer = window.setInterval(onShow, 30_000);
    window.addEventListener('focus', onShow);
    document.addEventListener('visibilitychange', onShow);
    return () => {
      alive = false;
      window.clearInterval(timer);
      window.removeEventListener('focus', onShow);
      document.removeEventListener('visibilitychange', onShow);
    };
  }, [signedIn]);

  const canSupport = state.kind === 'in' && state.me.permissions.includes('support');
  const supportWaiting = useCallback(
    (count: number) => {
      if (canSupport) {
        setSupportBadge(count);
      }
    },
    [canSupport],
  );

  const adminId = state.kind === 'in' ? state.me.admin.admin_id : null;
  useEffect(() => {
    if (adminId === null) {
      setBuild(null);
      return;
    }
    let alive = true;
    const tick = () =>
      api<BuildSummary>('/api/admin/system/summary')
        .then((s) => alive && setBuild(s))
        .catch(() => undefined);
    void tick();
    const timer = window.setInterval(tick, 15 * 60_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [adminId]);

  const appearance: Appearance = state.kind === 'in' ? state.me.appearance : (site ?? DEFAULT_SITE).appearance;
  const canSettings = state.kind === 'in' && state.me.permissions.includes('settings');
  const theme: Theme = !canSettings && personal ? personal : resolve(appearance.admin_theme, system);
  const phone = usePhone();
  useSurface(phone ? 'admin-phone' : 'admin', theme, appearance.admin === 'top' ? 'b' : 'a');

  const setAppearance = useCallback((next: Appearance) => {
    setState((current) => (current.kind === 'in' ? { kind: 'in', me: { ...current.me, appearance: next } } : current));
  }, []);

  const flipTheme = useCallback(async () => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    if (state.kind === 'in' && canSettings) {
      const optimistic = { ...state.me.appearance, admin_theme: next };
      setAppearance(optimistic);
      try {
        const saved = await api<Appearance>('/api/admin/settings/appearance', { method: 'PUT', body: optimistic });
        setAppearance(saved);
      } catch {
        setAppearance(state.me.appearance);
      }
      return;
    }
    writeStored(THEME_KEY, next);
    setPersonal(next);
  }, [theme, state, canSettings, setAppearance]);

  const session = useMemo<Session | null>(() => {
    if (state.kind !== 'in') {
      return null;
    }
    const me = state.me;
    return {
      me,
      can: (permission: Permission) => me.permissions.includes(permission),
      reload: load,
      logout: async () => {
        try {
          await api('/api/admin/auth/logout', { method: 'POST' });
        } finally {
          setState({ kind: 'guest' });
        }
      },
      setAppearance,
      theme,
      flipTheme: () => void flipTheme(),
      supportWaiting,
    };
  }, [state, load, setAppearance, theme, flipTheme, supportWaiting]);

  if (state.kind === 'loading') {
    return <div style={{ minHeight: '100vh', background: 'var(--bg)' }} aria-busy="true" />;
  }
  if (state.kind === 'error') {
    return (
      <div className="login">
        <div className="login-box card login-card" role="alert">
          <h1 style={{ fontSize: 22, fontWeight: 800 }}>Админка не открылась</h1>
          <p className="muted">{state.message}</p>
          <button type="button" className="btn-main btn" onClick={() => void load()}>
            Повторить
          </button>
        </div>
      </div>
    );
  }
  if (state.kind === 'guest' || !session) {
    return <Login brand={(site ?? DEFAULT_SITE).brand_name} logo={site?.logo_url ?? null} onDone={() => void load()} />;
  }

  return (
    <SessionContext.Provider value={session}>
      <Shell theme={theme} onFlipTheme={() => void flipTheme()} supportBadge={supportBadge} build={build}>
        <Routes>
          <Route index element={<Home />} />
          <Route path="clients" element={<Clients />} />
          <Route path="clients/:id" element={<ClientPage />} />
          <Route path="support" element={<Support />} />
          <Route path="money/*" element={<Pending title="Деньги" />} />
          <Route path="tariffs" element={<Tariffs />} />
          <Route path="infra" element={<InfraNodes />} />
          <Route path="infra/squads" element={<InfraSquads />} />
          <Route path="infra/billing" element={<InfraBilling />} />
          <Route path="modules" element={<Modules />} />
          <Route path="modules/:id" element={<ModulePage />} />
          <Route path="m/:module/*" element={<Pending title="Модуль" />} />
          <Route path="settings/look" element={<SettingsLook />} />
          <Route path="settings/remnawave" element={<SettingsRemna />} />
          <Route path="settings/payments" element={<SettingsPayments />} />
          <Route path="settings/general" element={<SettingsGeneral />} />
          <Route path="settings/bot" element={<SettingsBot />} />
          <Route path="settings/updates" element={<SettingsUpdates />} />
          <Route path="settings/bot/texts" element={<SettingsBotTexts />} />
          <Route path="settings/bot/images" element={<SettingsBotImages />} />
          <Route path="settings/documents" element={<SettingsDocuments />} />
          <Route path="settings" element={<Navigate to="/admin/settings/payments" replace />} />
          <Route
            path="settings/*"
            element={
              <SettingsLayout>
                <Pending title="Раздел настроек" />
              </SettingsLayout>
            }
          />
          <Route path="*" element={<Pending title="Страница не найдена" text="Такого раздела нет. Выберите раздел в меню." />} />
        </Routes>
      </Shell>
    </SessionContext.Provider>
  );
}
