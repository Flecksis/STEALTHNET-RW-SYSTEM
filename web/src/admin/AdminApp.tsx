import { useCallback, useEffect, useLayoutEffect, useMemo, useState } from 'react';
import { Navigate, Route, Routes } from 'react-router';
import { api, ApiError } from '../lib/api';
import { applyLang } from '../lib/i18n';
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
import { InfraHosts } from './infra/InfraHosts';
import { InfraBilling } from './infra/InfraBilling';
import { Modules } from './modules/Modules';
import { ModulePage } from './modules/ModulePage';
import { ModuleScreen } from './modules/ModuleScreen';
import { Login } from './Login';
import { Pending } from './pages/Pending';
import { Antibot } from './protect/Antibot';
import { Limits } from './protect/Limits';
import { Money } from './money/Money';
import { SettingsLayout } from './pages/SettingsLayout';
import { SettingsLook } from './pages/SettingsLook';
import { SettingsRemna } from './pages/SettingsRemna';
import { SettingsPayments } from './pages/SettingsPayments';
import { SettingsGeneral } from './pages/SettingsGeneral';
import { SettingsBot } from './pages/SettingsBot';
import { SettingsBotImages } from './pages/SettingsBotImages';
import { SettingsBotTexts } from './pages/SettingsBotTexts';
import { SettingsDocuments } from './pages/SettingsDocuments';
import { SettingsLanding } from './pages/SettingsLanding';
import { SettingsAdmins } from './pages/SettingsAdmins';
import { SettingsApiKeys } from './pages/SettingsApiKeys';
import { SettingsAlerts } from './pages/SettingsAlerts';
import { SettingsMail } from './pages/SettingsMail';
import { SettingsClientAccess } from './pages/SettingsClientAccess';
import { SettingsLanguages } from './pages/SettingsLanguages';
import { ForcedPassword, Security } from './pages/Security';
import { SettingsUpdates } from './pages/SettingsUpdates';
import { SettingsImport } from './pages/SettingsImport';
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
  useLayoutEffect(() => {
    void applyLang('ru');
  }, []);
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
  const [moneyBadge, setMoneyBadge] = useState(0);
  const [build, setBuild] = useState<BuildSummary | null>(null);
  const [badgeTick, setBadgeTick] = useState(0);
  const signedIn = state.kind === 'in';
  useEffect(() => {
    if (!signedIn) {
      return;
    }
    let alive = true;
    const tick = () =>
      api<{ support: number; money?: number }>('/api/admin/badges')
        .then((b) => {
          if (alive) {
            setSupportBadge(b.support);
            setMoneyBadge(b.money ?? 0);
          }
        })
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
  }, [signedIn, badgeTick]);

  const refreshBadges = useCallback(() => setBadgeTick((n) => n + 1), []);

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
      refreshBadges,
    };
  }, [state, load, setAppearance, theme, flipTheme, supportWaiting, refreshBadges]);

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
  if (state.me.must_change_password) {
    return (
      <ForcedPassword brand={state.me.brand_name} logo={state.me.logo_url ?? null} onDone={() => void load()} onLogout={() => void session.logout()} />
    );
  }

  return (
    <SessionContext.Provider value={session}>
      <Shell theme={theme} onFlipTheme={() => void flipTheme()} supportBadge={supportBadge} moneyBadge={moneyBadge} build={build}>
        <Routes>
          <Route index element={<Home />} />
          <Route path="clients" element={<Clients />} />
          <Route path="clients/antibot" element={<Antibot />} />
          <Route path="clients/limits" element={<Limits />} />
          <Route path="clients/:id" element={<ClientPage />} />
          <Route path="support" element={<Support />} />
          <Route path="money/*" element={<Money />} />
          <Route path="tariffs" element={<Tariffs />} />
          <Route path="infra" element={<InfraNodes />} />
          <Route path="infra/squads" element={<InfraSquads />} />
          <Route path="infra/hosts" element={<InfraHosts />} />
          <Route path="infra/billing" element={<InfraBilling />} />
          <Route path="modules" element={<Modules />} />
          <Route path="modules/:id" element={<ModulePage />} />
          <Route path="m/:module/*" element={<ModuleScreen />} />
          <Route path="settings/look" element={<SettingsLook />} />
          <Route path="settings/remnawave" element={<SettingsRemna />} />
          <Route path="settings/payments" element={<SettingsPayments />} />
          <Route path="settings/general" element={<SettingsGeneral />} />
          <Route path="settings/bot" element={<SettingsBot />} />
          <Route path="settings/updates" element={<SettingsUpdates />} />
          <Route path="settings/bot/texts" element={<SettingsBotTexts />} />
          <Route path="settings/bot/images" element={<SettingsBotImages />} />
          <Route path="settings/documents" element={<SettingsDocuments />} />
          <Route path="settings/landing" element={<SettingsLanding />} />
          <Route path="settings/admins" element={<SettingsAdmins />} />
          <Route path="settings/api-keys" element={<SettingsApiKeys />} />
          <Route path="settings/mail" element={<SettingsMail />} />
          <Route path="settings/alerts" element={<SettingsAlerts />} />
          <Route path="settings/client" element={<SettingsClientAccess />} />
          <Route path="settings/languages" element={<SettingsLanguages />} />
          <Route path="settings/import" element={<SettingsImport />} />
          <Route path="security" element={<Security />} />
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
