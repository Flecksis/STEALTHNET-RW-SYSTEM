import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { NBSP, timeHM } from '../../lib/format';
import '../form.css';
import { Switch } from '../ui';
import { ConnectAppsEditor, appsProblem, fullApps, sameApps, type Apps, type AppsView } from './ConnectApps';
import { SettingsLayout } from './SettingsLayout';
import { SYNC_PATH } from './SettingsRemnaSync';
import { Link } from 'react-router';
import { useSession } from '../session';

type Guard = 'none' | 'cookie' | 'tinyauth' | 'portal';

type RemnaView = {
  url: string | null;
  url_from_env: boolean;
  token_set: boolean;
  token_from_env: boolean;
  cookie_set: boolean;
  api_key_set: boolean;
  guard: Guard;
  guard_from_env: boolean;
  webhook_secret_set: boolean;
  env: string | null;
};

type Check = {
  ok: boolean;
  version: string | null;
  supported: boolean;
  sub_public_domain: string | null;
  webhooks_enabled: boolean | null;
  usage_records: boolean | null;
  request_history: boolean | null;
  error: string | null;
  hint: string | null;
  checked_at: string;
};

type Remarks = {
  expiredUsers: string[];
  limitedUsers: string[];
  disabledUsers: string[];
  HWIDMaxDevicesExceeded: string[];
  HWIDNotSupported: string[];
  emptyHosts: string[];
};

type SubSettings = {
  isShowCustomRemarks: boolean;
  customRemarks: Remarks;
  hwidSettings: { enabled: boolean; fallbackDeviceLimit: number; maxDevicesAnnounce: string | null } | null;
};

type Connect = { mode: 'ours' | 'remnawave'; page_config: string | null };
type PageConfig = { uuid: string; name: string; position: number };

type RemKey = keyof Remarks;

const REMARKS: Array<[RemKey, string]> = [
  ['expiredUsers', 'Подписка истекла'],
  ['limitedUsers', 'Закончился трафик'],
  ['disabledUsers', 'Подписка заблокирована'],
  ['HWIDMaxDevicesExceeded', 'Слишком много устройств'],
  ['HWIDNotSupported', 'Приложение не поддерживается'],
  ['emptyHosts', 'Нет серверов'],
];

const GUARDS: Array<[Guard, string]> = [
  ['none', 'Нет'],
  ['cookie', 'Кука'],
  ['tinyauth', 'Логин и пароль TinyAuth'],
  ['portal', 'Ключ портала (Caddy)'],
];

const GUARD_NOTE: Record<Guard, string> = {
  none: 'Нужна, если панель ставили скриптом eGames, а STEALTHNET стоит на другом сервере. На том же сервере укажите адрес http://remnawave:3000: запросы пойдут к панели напрямую, без защиты',
  cookie: 'Вариант eGames по умолчанию: без куки панель закрывает соединение, ничего не ответив',
  tinyauth: 'Страница входа TinyAuth, вариант eGames для nginx',
  portal: 'Страница входа с MFA, вариант eGames для Caddy',
};

type Work = {
  addr: string;
  token: string | null;
  guard: Guard;
  cookie: string;
  login: string;
  password: string;
  key: string;
  devOn: boolean;
  devLimit: string;
  devMsg: string;
  remOn: boolean;
  rem: Record<RemKey, string>;
  mode: Connect['mode'];
  look: string;
  apps: Apps;
};

type Loaded = {
  remna: RemnaView;
  check: Check | null;
  sub: SubSettings | null;
  connect: Connect;
  configs: PageConfig[];
  apps: AppsView | null;
};

function workFrom(data: Loaded): Work {
  const hwid = data.sub?.hwidSettings;
  const rem = {} as Record<RemKey, string>;
  for (const [key] of REMARKS) {
    rem[key] = data.sub?.customRemarks[key]?.[0] ?? '';
  }
  return {
    addr: data.remna.url ?? '',
    token: null,
    guard: data.remna.guard,
    cookie: '',
    login: '',
    password: '',
    key: '',
    devOn: hwid?.enabled ?? false,
    devLimit: String(hwid?.fallbackDeviceLimit ?? 1),
    devMsg: hwid?.maxDevicesAnnounce ?? '',
    remOn: data.sub?.isShowCustomRemarks ?? false,
    rem,
    mode: data.connect.mode,
    look: data.connect.page_config ?? data.configs[0]?.uuid ?? '',
    apps: fullApps(data.apps?.apps),
  };
}

function limitBad(value: string): boolean {
  return !/^\d+$/.test(value) || +value < 1 || +value > 99;
}

function guardSet(view: RemnaView, guard: Guard): boolean {
  return guard === 'cookie' ? view.cookie_set : guard === 'none' ? false : view.api_key_set;
}

function guardFilled(work: Work): boolean {
  switch (work.guard) {
    case 'cookie':
      return !!work.cookie.trim();
    case 'tinyauth':
      return !!work.login.trim() && !!work.password.trim();
    case 'portal':
      return !!work.key.trim();
    default:
      return true;
  }
}

function guardTouched(work: Work): boolean {
  switch (work.guard) {
    case 'cookie':
      return !!work.cookie.trim();
    case 'tinyauth':
      return !!work.login.trim() || !!work.password.trim();
    case 'portal':
      return !!work.key.trim();
    default:
      return false;
  }
}

function guardBody(work: Work): Record<string, string> {
  switch (work.guard) {
    case 'none':
      return { mode: 'none' };
    case 'cookie':
      return { mode: 'cookie', cookie: work.cookie.trim() };
    case 'tinyauth':
      return { mode: 'tinyauth', login: work.login.trim(), password: work.password };
    default:
      return { mode: 'portal', key: work.key.trim() };
  }
}

function keysPage(addr: string): string | null {
  try {
    const url = new URL(addr.trim());
    return url.protocol === 'https:' ? `${url.origin}/r/settings/apikeys` : null;
  } catch {
    return null;
  }
}

function maskEnv(env: string): string {
  return env.replace(/(WEBHOOK_SECRET_HEADER=)(.+)$/m, (_, key: string, secret: string) =>
    secret.length > 8 && !secret.includes('…') ? `${key}${secret.slice(0, 4)}…${secret.slice(-4)}` : `${key}${secret}`,
  );
}

export function SettingsRemna() {
  const { can, reload } = useSession();
  const [data, setData] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [work, setWork] = useState<Work | null>(null);
  const [tokenEdit, setTokenEdit] = useState(false);
  const [guardEdit, setGuardEdit] = useState(false);
  const [tried, setTried] = useState(false);
  const [note, setNote] = useState('');
  const [saveError, setSaveError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [checking, setChecking] = useState(false);
  const [copied, setCopied] = useState(false);
  const [loads, setLoads] = useState(0);

  const load = useCallback(async () => {
    try {
      const [remna, connect] = await Promise.all([
        api<RemnaView>('/api/admin/settings/remnawave'),
        api<Connect>('/api/admin/settings/connect'),
      ]);
      const linked = !!remna.url && remna.token_set;
      const [check, sub, configs, apps] = await Promise.all([
        remna.url ? api<Check>('/api/admin/settings/remnawave/check', { method: 'POST' }).catch(() => null) : Promise.resolve(null),
        linked ? api<SubSettings>('/api/admin/settings/subscription').catch(() => null) : Promise.resolve(null),
        linked ? api<PageConfig[]>('/api/admin/settings/remnawave/page-configs').catch(() => [] as PageConfig[]) : Promise.resolve([] as PageConfig[]),
        api<AppsView>('/api/admin/settings/connect-apps').catch(() => null),
      ]);
      const loaded = { remna, check, sub, connect, configs, apps };
      setData(loaded);
      setWork(workFrom(loaded));
      setLoads((n) => n + 1);
      setTokenEdit(false);
      setGuardEdit(false);
      setTried(false);
      setLoadError(null);
    } catch (err) {
      setLoadError(err instanceof ApiError ? err.message : 'Не удалось загрузить настройки');
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const base = useMemo(() => (data ? workFrom(data) : null), [data]);

  if (loadError) {
    return (
      <SettingsLayout>
        <section className="sec card-pad" role="alert">
          <h2 className="section-title">Настройки не загрузились</h2>
          <p className="muted">{loadError}</p>
        </section>
      </SettingsLayout>
    );
  }
  if (!data || !work || !base) {
    return <SettingsLayout>{null}</SettingsLayout>;
  }

  const linkOk = !!data.check?.ok;
  const noAddr = !work.addr.trim();
  const subMissing = data.sub === null;
  const edit = (patch: Partial<Work>) => {
    setWork({ ...work, ...patch });
    setNote('');
    setSaveError(null);
  };
  const remChanged = work.remOn !== base.remOn || REMARKS.some(([key]) => work.rem[key] !== base.rem[key]);
  const devChanged = work.devOn !== base.devOn || work.devLimit !== base.devLimit || work.devMsg !== base.devMsg;
  const guardKeep = work.guard === data.remna.guard && guardSet(data.remna, work.guard) && !guardEdit;
  const guardChanged = work.guard !== data.remna.guard || (!guardKeep && guardTouched(work));
  const guardBad = guardChanged && !guardKeep && !guardFilled(work);
  const linkChanged = work.addr.trim() !== base.addr.trim() || (work.token !== null && work.token.trim() !== '') || guardChanged;
  const connectChanged = work.mode !== base.mode;
  const appsChanged = !!data.apps && !sameApps(work.apps, base.apps);
  const appsIssue = appsChanged ? appsProblem(work.apps) : null;
  const dirty = remChanged || devChanged || linkChanged || connectChanged || appsChanged;
  const addrBad = noAddr;
  const remBad = work.remOn && REMARKS.some(([key]) => !work.rem[key].trim());
  const limBad = work.devOn && limitBad(work.devLimit);
  const invalid = addrBad || remBad || limBad || guardBad || !!appsIssue;
  const blocked = (!linkOk || subMissing) && (remChanged || devChanged) && !linkChanged;

  let statusText: string;
  let statusColor = 'var(--ink-2)';
  if (noAddr) {
    statusText = 'Укажите адрес панели';
    statusColor = 'var(--bad)';
  } else if (linkOk && data.check) {
    statusText = `Подключено · Remnawave${NBSP}${data.check.version ?? ''} · проверено в${NBSP}${timeHM(data.check.checked_at)}`;
  } else if (data.check) {
    statusText = `${data.check.error ?? 'Токен не принят: проверьте, что он не удалён в панели'} · проверено в${NBSP}${timeHM(data.check.checked_at)}`;
    statusColor = 'var(--bad)';
  } else {
    statusText = 'Нет связи с панелью';
    statusColor = 'var(--bad)';
  }

  const check = async () => {
    if (noAddr) {
      return;
    }
    if (guardBad) {
      setTried(true);
      return;
    }
    setChecking(true);
    try {
      const body: Record<string, unknown> = { url: work.addr.trim() };
      if (work.token && work.token.trim()) {
        body.token = work.token.trim();
      }
      if (guardChanged) {
        body.guard = guardBody(work);
      }
      const result = await api<Check>('/api/admin/settings/remnawave/check', { method: 'POST', body });
      setData({ ...data, check: result });
    } catch (err) {
      setData({
        ...data,
        check: {
          ok: false,
          version: null,
          supported: false,
          sub_public_domain: null,
          webhooks_enabled: null,
          usage_records: null,
          request_history: null,
          error: err instanceof ApiError ? err.message : 'Проверка не прошла',
          hint: null,
          checked_at: new Date().toISOString(),
        },
      });
    } finally {
      setChecking(false);
    }
  };

  const save = async () => {
    if (guardBad) {
      setTried(true);
    }
    if (!dirty || invalid || blocked || busy) {
      return;
    }
    setBusy(true);
    setSaveError(null);
    try {
      if (linkChanged) {
        const body: Record<string, unknown> = { url: work.addr.trim() };
        if (work.token && work.token.trim()) {
          body.token = work.token.trim();
        }
        if (guardChanged) {
          body.guard = guardBody(work);
        }
        await api('/api/admin/settings/remnawave', { method: 'PUT', body });
        void reload();
      }
      if (devChanged || remChanged) {
        const remarks = {} as Record<RemKey, string[]>;
        for (const [key] of REMARKS) {
          remarks[key] = [work.rem[key].trim()];
        }
        const body: Record<string, unknown> = {};
        if (devChanged) {
          body.hwid = {
            enabled: work.devOn,
            fallbackDeviceLimit: work.devOn ? Number(work.devLimit) : Number(base.devLimit) || 1,
            maxDevicesAnnounce: work.devMsg.trim() || null,
          };
        }
        if (remChanged) {
          body.show_custom_remarks = work.remOn;
          if (work.remOn) {
            body.custom_remarks = remarks;
          }
        }
        await api('/api/admin/settings/subscription', { method: 'PUT', body });
      }
      if (appsChanged && data.apps) {
        await api('/api/admin/settings/connect-apps', {
          method: 'PUT',
          body: { apps: sameApps(work.apps, data.apps.defaults) ? null : work.apps },
        });
      }
      if (connectChanged) {
        await api('/api/admin/settings/connect', {
          method: 'PUT',
          body: { mode: work.mode, page_config: data.connect.page_config },
        });
      }
      await load();
      setNote('Сохранено');
    } catch (err) {
      setSaveError(err instanceof ApiError ? err.message : 'Не получилось сохранить');
    } finally {
      setBusy(false);
    }
  };

  const copyEnv = async () => {
    try {
      const full = await api<{ env: string | null }>('/api/admin/settings/remnawave/webhook-env');
      if (full.env && navigator.clipboard) {
        await navigator.clipboard.writeText(full.env);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 2000);
      }
    } catch {
      setCopied(false);
    }
  };

  const createSecret = async () => {
    try {
      await api('/api/admin/settings/remnawave/webhook-secret', { method: 'POST' });
      await load();
    } catch (err) {
      setSaveError(err instanceof ApiError ? err.message : 'Не получилось создать секрет');
    }
  };

  const hooksOn = data.check?.webhooks_enabled;
  const flagState = (value: boolean | null | undefined, on: string, off: string) =>
    value === true ? { text: on, color: 'var(--ink-2)' } : value === false ? { text: off, color: 'var(--warn)' } : { text: 'неизвестно', color: 'var(--muted)' };
  const flags = [
    { name: 'Вебхуки', ...flagState(hooksOn, 'включены', 'выключены'), use: 'События для ленты на главной, «Не подключились», уведомления о нодах и оплате серверов' },
    { name: 'Учёт трафика по дням', ...flagState(data.check?.usage_records, 'включён', 'выключен'), use: 'Трафик за 30 дней в карточке клиента' },
    { name: 'История запросов подписки', ...flagState(data.check?.request_history, 'включена', 'выключена'), use: 'Запросы подписки в карточке клиента' },
  ];
  const envBlock = (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: hooksOn === false ? '0 20px 18px' : '14px 20px 18px', borderTop: hooksOn === false ? 'none' : '1px solid var(--line)' }}>
      <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: '8px 12px', justifyContent: 'space-between' }}>
        <h3 style={{ fontSize: 15, fontWeight: 700 }}>Куда Remnawave отправляет события</h3>
        {data.remna.env ? (
          <button type="button" className="b2 hov" onClick={() => void copyEnv()}>
            {copied ? 'Скопировано' : 'Скопировать'}
          </button>
        ) : (
          <button type="button" className="b2 hov" onClick={() => void createSecret()}>
            Создать секрет
          </button>
        )}
      </div>
      {data.remna.env ? <pre className="code mono">{maskEnv(data.remna.env)}</pre> : <span className="cap">Секрет вебхуков ещё не создан</span>}
      <span className="cap">Вставьте в .env панели Remnawave и перезапустите её</span>
    </div>
  );

  const direct = /^http:\/\/remnawave(:\d+)?(\/|$)/i.test(work.addr.trim());
  const guardErr =
    tried && guardBad
      ? work.guard === 'cookie'
        ? 'Вставьте куку'
        : work.guard === 'portal'
          ? 'Вставьте ключ'
          : !work.login.trim() && !work.password.trim()
            ? 'Укажите логин и пароль'
            : !work.login.trim()
              ? 'Укажите логин'
              : 'Укажите пароль'
      : null;
  const keys = keysPage(work.addr);
  const secretNote =
    work.guard === 'cookie'
      ? 'Ссылка входа, которую установщик eGames показал в конце установки, или строка map $http_cookie из /opt/remnawave/nginx.conf. Подойдёт и пара ИМЯ=ЗНАЧЕНИЕ'
      : work.guard === 'tinyauth'
        ? 'Установщик eGames показывает их в конце установки, вместе с адресом страницы входа'
        : keys
          ? `Ключ создаётся на странице входа, в разделе API Keys: ${keys}`
          : 'Ключ создаётся на странице входа панели, в разделе API Keys';
  const secretDescribed = guardErr ? 'rw-sec-d rw-sec-e' : 'rw-sec-d';
  const dropSecrets = () => {
    setGuardEdit(false);
    setTried(false);
    edit({ cookie: '', login: '', password: '', key: '' });
  };

  const saveText = blocked
    ? 'Нет связи с панелью: замените токен, чтобы сохранить'
    : invalid
      ? addrBad || remBad || guardBad || (work.devOn && !work.devLimit)
        ? 'Заполните пустые поля'
        : appsIssue
          ? appsIssue
          : 'Исправьте лимит устройств'
      : saveError
        ? saveError
        : dirty
          ? 'Есть несохранённые изменения'
          : note;
  const saveBad = blocked || invalid || !!saveError;

  return (
    <SettingsLayout>
      <section aria-labelledby="cn-h" className="sec">
        <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: '12px 20px', padding: '14px 20px' }}>
          <div style={{ flex: '1 1 340px', minWidth: 0, display: 'flex', flexDirection: 'column', gap: 4 }}>
            <h2 id="cn-h" className="section-title">
              Подключение
            </h2>
            <span role="status" style={{ color: statusColor, fontWeight: 600 }}>
              {statusText}
            </span>
            {linkOk && data.check && !data.check.supported ? (
              <span className="warn-line">Система проверена с Remnawave 3.4.11, у этой панели другая версия</span>
            ) : null}
            {!linkOk && data.check?.hint ? (
              <span style={{ color: 'var(--ink-2)', textWrap: 'pretty' }}>{data.check.hint}</span>
            ) : null}
            {!linkOk && !noAddr ? <span className="muted">Нет связи с панелью, часть настроек недоступна</span> : null}
          </div>
          <button type="button" className="hov b2" onClick={() => void check()} disabled={checking}>
            {checking ? 'Проверяем…' : 'Проверить подключение'}
          </button>
        </div>
        <div className="row">
          <label htmlFor="rw-addr" className="rl">
            Адрес панели
          </label>
          <div className="rc">
            <input
              id="rw-addr"
              className="field mono"
              type="url"
              value={work.addr}
              maxLength={200}
              placeholder="https://"
              aria-invalid={addrBad}
              aria-describedby={addrBad ? 'rw-addr-e' : undefined}
              onChange={(e) => edit({ addr: e.target.value })}
            />
            {addrBad ? (
              <span id="rw-addr-e" className="err">
                Укажите адрес панели
              </span>
            ) : null}
            {data.remna.url_from_env && work.addr === base.addr ? <span className="cap">Адрес взят из переменной REMNAWAVE_URL</span> : null}
          </div>
        </div>
        <div className="row">
          <label htmlFor="rw-tok" className="rl">
            API-токен
          </label>
          <div className="rc">
            <div style={{ display: 'flex', gap: 8 }}>
              {tokenEdit ? (
                <input
                  id="rw-tok"
                  className="field mono"
                  type="text"
                  value={work.token ?? ''}
                  maxLength={400}
                  placeholder="Вставьте новый токен"
                  aria-describedby="rw-tok-d"
                  autoFocus
                  onChange={(e) => edit({ token: e.target.value })}
                />
              ) : (
                <input
                  id="rw-tok"
                  className="field mono"
                  type="text"
                  readOnly
                  value={data.remna.token_set ? '••••••••••••' : ''}
                  placeholder="Токен не задан"
                  aria-describedby="rw-tok-d"
                  style={{ color: 'var(--ink-2)' }}
                />
              )}
              <button
                type="button"
                className="hov b2"
                aria-label={tokenEdit ? 'Отменить замену токена' : 'Заменить API-токен'}
                onClick={() => {
                  if (tokenEdit) {
                    setTokenEdit(false);
                    edit({ token: null });
                  } else {
                    setTokenEdit(true);
                  }
                }}
              >
                {tokenEdit ? 'Отменить' : data.remna.token_set ? 'Заменить' : 'Указать'}
              </button>
            </div>
            <span id="rw-tok-d" className="cap">
              Токен создаётся в панели Remnawave, в разделе «API-токены»
            </span>
          </div>
        </div>
        <div className="row">
          <label htmlFor="rw-guard" className="rl">
            Защита панели
          </label>
          <div className="rc">
            <select
              id="rw-guard"
              className="field"
              value={work.guard}
              aria-describedby="rw-guard-d"
              style={{ maxWidth: 320 }}
              onChange={(e) => {
                setGuardEdit(false);
                setTried(false);
                edit({ guard: e.target.value as Guard });
              }}
            >
              {GUARDS.map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
            <span id="rw-guard-d" className="cap">
              {GUARD_NOTE[work.guard]}
            </span>
            {direct && work.guard !== 'none' ? (
              <span className="warn-line">Адрес http://remnawave:3000 ведёт к панели напрямую, защиту можно выключить</span>
            ) : null}
          </div>
        </div>
        {work.guard !== 'none' ? (
          <div className="row">
            {work.guard === 'tinyauth' ? (
              <span id="rw-sec-l" className="rl">
                Логин и пароль
              </span>
            ) : (
              <label htmlFor="rw-sec" className="rl">
                {work.guard === 'cookie' ? 'Кука' : 'Ключ портала'}
              </label>
            )}
            <div className="rc">
              {guardKeep ? (
                <div style={{ display: 'flex', gap: 8 }}>
                  <input
                    id="rw-sec"
                    className="field mono"
                    type="text"
                    readOnly
                    value="••••••••••••"
                    aria-labelledby={work.guard === 'tinyauth' ? 'rw-sec-l' : undefined}
                    aria-describedby="rw-sec-d"
                    style={{ color: 'var(--ink-2)' }}
                  />
                  <button
                    type="button"
                    className="hov b2"
                    aria-label={work.guard === 'cookie' ? 'Заменить куку' : work.guard === 'tinyauth' ? 'Заменить логин и пароль' : 'Заменить ключ портала'}
                    onClick={() => setGuardEdit(true)}
                  >
                    Заменить
                  </button>
                </div>
              ) : (
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
                  {work.guard === 'tinyauth' ? (
                    <>
                      <input
                        id="rw-sec"
                        className="field"
                        type="text"
                        value={work.login}
                        maxLength={200}
                        placeholder="Логин"
                        aria-label="Логин TinyAuth"
                        aria-invalid={!!guardErr && !work.login.trim()}
                        aria-describedby={secretDescribed}
                        autoComplete="off"
                        spellCheck={false}
                        autoFocus={guardEdit}
                        style={{ flex: '1 1 150px' }}
                        onChange={(e) => edit({ login: e.target.value })}
                      />
                      <input
                        id="rw-pass"
                        className="field"
                        type="password"
                        value={work.password}
                        maxLength={200}
                        placeholder="Пароль"
                        aria-label="Пароль TinyAuth"
                        aria-invalid={!!guardErr && !work.password.trim()}
                        aria-describedby={secretDescribed}
                        autoComplete="new-password"
                        style={{ flex: '1 1 150px' }}
                        onChange={(e) => edit({ password: e.target.value })}
                      />
                    </>
                  ) : (
                    <input
                      id="rw-sec"
                      className="field mono"
                      type="text"
                      value={work.guard === 'cookie' ? work.cookie : work.key}
                      maxLength={1000}
                      placeholder={work.guard === 'cookie' ? 'Ссылка входа или ИМЯ=ЗНАЧЕНИЕ' : 'Ключ со страницы входа'}
                      aria-invalid={!!guardErr}
                      aria-describedby={secretDescribed}
                      autoComplete="off"
                      spellCheck={false}
                      autoFocus={guardEdit}
                      style={{ flex: '1 1 240px' }}
                      onChange={(e) => edit(work.guard === 'cookie' ? { cookie: e.target.value } : { key: e.target.value })}
                    />
                  )}
                  {guardEdit ? (
                    <button type="button" className="hov b2" onClick={dropSecrets}>
                      Отменить
                    </button>
                  ) : null}
                </div>
              )}
              {guardErr ? (
                <span id="rw-sec-e" className="err">
                  {guardErr}
                </span>
              ) : null}
              <span id="rw-sec-d" className="cap">
                {secretNote}
              </span>
              {guardKeep && data.remna.guard_from_env ? (
                <span className="cap">{work.guard === 'cookie' ? 'Кука взята из переменной REMNAWAVE_COOKIE' : 'Ключ взят из переменной REMNAWAVE_X_API_KEY'}</span>
              ) : null}
            </div>
          </div>
        ) : null}
        <div className="row">
          <span className="rl" style={{ paddingTop: 0 }}>
            Домен подписки
          </span>
          <div className="rc" style={{ gap: 2 }}>
            <span className="mono" style={{ fontWeight: 600 }}>
              {data.check?.sub_public_domain ?? '—'}
            </span>
            <span className="cap">Задаётся в Remnawave. Его видят клиенты в ссылке подписки</span>
          </div>
        </div>
      </section>

      {can('remna_sync') ? (
        <section aria-labelledby="sync-h" className="sec">
          <div className="sec-head">
            <h2 id="sync-h" className="section-title">
              Синхронизация
            </h2>
            <span className="muted">
              Перенести клиентов и подписки панели в Remnawave с прежними ссылками или забрать в панель пользователей Remnawave, которых в ней
              нет. Пригодится после восстановления из копии или переезда на новую Remnawave
            </span>
          </div>
          <div className="row">
            <Link to={SYNC_PATH} className="btn-line hov">
              Открыть синхронизацию
            </Link>
          </div>
        </section>
      ) : null}

      <section id="devices" aria-labelledby="dv-h" className="sec">
        <div className="sec-head">
          <h2 id="dv-h" className="section-title">
            Устройства
          </h2>
          {subMissing ? <span className="muted">Эти настройки хранятся в Remnawave. Без связи с панелью их не прочитать</span> : null}
        </div>
        <div className={subMissing ? 'off' : undefined} aria-disabled={subMissing}>
          <div className="toggle-row">
            <div>
              <span id="dv-l" style={{ fontWeight: 600 }}>
                Проверять лимит устройств
              </span>
              <span id="dv-d" className="cap">
                Клиент подключит не больше устройств, чем в его тарифе. Если выключить, лимит из тарифов не работает
              </span>
              {!work.devOn && !subMissing ? <span className="warn-line">Лимит выключен: на одной подписке можно подключить сколько угодно устройств</span> : null}
            </div>
            <Switch
              on={work.devOn}
              label="Проверять лимит устройств"
              describedBy="dv-d"
              onToggle={() => (subMissing ? undefined : edit(work.devOn ? { devOn: false, devLimit: limitBad(work.devLimit) ? base.devLimit : work.devLimit } : { devOn: true }))}
            />
          </div>
          <div role="group" aria-label="Лимит и сообщение" aria-disabled={!work.devOn} className={work.devOn ? undefined : 'off'}>
            <div className="row">
              <label htmlFor="dv-lim" className="rl">
                Лимит по умолчанию
              </label>
              <div className="rc" style={{ flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: '6px 12px' }}>
                <input
                  id="dv-lim"
                  className="fld"
                  inputMode="numeric"
                  value={work.devLimit}
                  readOnly={!work.devOn || subMissing}
                  aria-invalid={limBad}
                  aria-describedby={limBad ? 'dv-lim-d dv-lim-e' : 'dv-lim-d'}
                  onChange={(e) => edit({ devLimit: e.target.value.replace(/\D/g, '').slice(0, 2) })}
                />
                <span id="dv-lim-d" className="cap">
                  Если у клиента лимит не задан
                </span>
                {limBad ? (
                  <span id="dv-lim-e" className="err" style={{ flex: '0 0 100%' }}>
                    Укажите число от 1 до 99
                  </span>
                ) : null}
              </div>
            </div>
            <div className="row">
              <label htmlFor="dv-msg" className="rl">
                Сообщение, когда устройств больше лимита
              </label>
              <div className="rc">
                <textarea
                  id="dv-msg"
                  className="field"
                  rows={2}
                  maxLength={200}
                  value={work.devMsg}
                  readOnly={!work.devOn || subMissing}
                  aria-describedby="dv-cnt"
                  onChange={(e) => edit({ devMsg: e.target.value })}
                />
                <span id="dv-cnt" className="cap" style={{ alignSelf: 'flex-end' }}>
                  {work.devMsg.length} из 200
                </span>
              </div>
            </div>
          </div>
        </div>
      </section>

      <section aria-labelledby="rm-h" className="sec">
        <div className="sec-head">
          <h2 id="rm-h" className="section-title">
            Что видит клиент в приложении
          </h2>
          <span className="muted" style={{ textWrap: 'pretty' }}>
            Когда подписка не работает, вместо списка серверов в приложении клиента будут эти строки
          </span>
        </div>
        <div className={subMissing ? 'off' : undefined} aria-disabled={subMissing}>
          <div className="toggle-row">
            <div>
              <span style={{ fontWeight: 600 }}>Показывать эти строки</span>
              {!work.remOn && !subMissing ? <span className="cap">Клиент увидит пустой список серверов</span> : null}
            </div>
            <Switch on={work.remOn} label="Показывать эти строки" onToggle={() => (subMissing ? undefined : edit({ remOn: !work.remOn, rem: work.remOn ? { ...work.rem, ...Object.fromEntries(REMARKS.filter(([k]) => !work.rem[k].trim()).map(([k]) => [k, base.rem[k]])) } : work.rem }))} />
          </div>
          <div role="group" aria-label="Строки для клиента" aria-disabled={!work.remOn} className={work.remOn ? undefined : 'off'}>
            {REMARKS.map(([key, label]) => {
              const bad = work.remOn && !work.rem[key].trim();
              return (
                <div className="row" key={key}>
                  <label htmlFor={`rm-${key}`} className="rl">
                    {label}
                  </label>
                  <div className="rc">
                    <input
                      id={`rm-${key}`}
                      className="field"
                      maxLength={120}
                      value={work.rem[key]}
                      readOnly={!work.remOn || subMissing}
                      aria-invalid={bad}
                      onChange={(e) => edit({ rem: { ...work.rem, [key]: e.target.value } })}
                    />
                    {bad ? <span className="err">Напишите хотя бы одну строку</span> : null}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      </section>

      <section aria-labelledby="cp-h" className="sec">
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12, padding: '14px 20px 16px' }}>
          <h2 id="cp-h" className="section-title">
            Страница подключения
          </h2>
          <div role="group" aria-labelledby="cp-h" className="segs" style={{ alignSelf: 'flex-start' }}>
            {(
              [
                ['ours', 'Через нас'],
                ['remnawave', 'Страница Remnawave'],
              ] as const
            ).map(([mode, label]) => (
              <button key={mode} type="button" aria-pressed={work.mode === mode} aria-describedby="cp-d" onClick={() => edit({ mode })}>
                {label}
              </button>
            ))}
          </div>
          <span id="cp-d" className="muted" style={{ textWrap: 'pretty' }}>
            {work.mode === 'ours'
              ? 'Кабинет и мини-приложение показывают пошаговую инструкцию: приложение для устройства клиента и кнопку добавления подписки'
              : 'Кабинет и мини-приложение показывают кнопку «Открыть страницу подключения», она ведёт на страницу подписки Remnawave. Её оформление выбирается в самой Remnawave'}
          </span>
        </div>
        {work.mode === 'ours' ? (
          data.apps ? (
            <ConnectAppsEditor apps={work.apps} defaults={fullApps(data.apps.defaults)} limits={data.apps.limits} configs={data.configs} canImport={linkOk} look={work.look} version={loads} onChange={(apps) => edit({ apps })} />
          ) : (
            <p className="cap" style={{ padding: '0 20px 16px' }}>
              Список приложений не загрузился. Обновите страницу
            </p>
          )
        ) : null}
      </section>

      <section aria-labelledby="wh-h" className="sec">
        <div className="sec-head">
          <h2 id="wh-h" className="section-title">
            Вебхуки и учёт
          </h2>
          <span className="muted" style={{ textWrap: 'pretty' }}>
            Задаются в файле .env панели Remnawave, STEALTHNET их только показывает
          </span>
        </div>
        {hooksOn === false ? (
          <>
            <div style={{ borderTop: '1px solid var(--line)' }}>
              <div className="flag">
                <span className="flag-name">{flags[0]!.name}</span>
                <span className="flag-state" style={{ color: flags[0]!.color }}>
                  {flags[0]!.text}
                </span>
                <span className="flag-use">{flags[0]!.use}</span>
              </div>
              <div className="warn-line" style={{ padding: '0 20px 14px', fontSize: 14 }}>
                Лента событий, «Не подключились» и уведомления о нодах не обновляются. Данные о клиентах STEALTHNET берёт опросом раз в минуту
              </div>
            </div>
            {envBlock}
            {flags.slice(1).map((flag) => (
              <div className="flag" key={flag.name} style={{ borderTop: '1px solid var(--line)' }}>
                <span className="flag-name">{flag.name}</span>
                <span className="flag-state" style={{ color: flag.color }}>
                  {flag.text}
                </span>
                <span className="flag-use">{flag.use}</span>
              </div>
            ))}
          </>
        ) : (
          <>
            {flags.map((flag) => (
              <div key={flag.name} style={{ borderTop: '1px solid var(--line)' }}>
                <div className="flag">
                  <span className="flag-name">{flag.name}</span>
                  <span className="flag-state" style={{ color: flag.color }}>
                    {flag.text}
                  </span>
                  <span className="flag-use">{flag.use}</span>
                </div>
              </div>
            ))}
            {envBlock}
          </>
        )}
      </section>

      <div className="save-row">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          Сохранить
        </button>
        <button
          type="button"
          className="btn-line hov"
          disabled={busy}
          onClick={() => {
            setWork(base);
            setTokenEdit(false);
            setGuardEdit(false);
            setTried(false);
            setNote('');
            setSaveError(null);
          }}
        >
          Отменить
        </button>
        <span role="status" style={{ color: saveBad ? 'var(--bad)' : 'var(--ink-2)' }}>
          {saveText}
        </span>
      </div>
    </SettingsLayout>
  );
}
