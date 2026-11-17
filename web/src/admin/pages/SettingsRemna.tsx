import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { NBSP, timeHM } from '../../lib/format';
import '../form.css';
import { Switch } from '../ui';
import { SettingsLayout } from './SettingsLayout';

type RemnaView = {
  url: string | null;
  url_from_env: boolean;
  token_set: boolean;
  token_from_env: boolean;
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

type Work = {
  addr: string;
  token: string | null;
  devOn: boolean;
  devLimit: string;
  devMsg: string;
  remOn: boolean;
  rem: Record<RemKey, string>;
  mode: Connect['mode'];
  look: string;
};

type Loaded = {
  remna: RemnaView;
  check: Check | null;
  sub: SubSettings | null;
  connect: Connect;
  configs: PageConfig[];
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
    devOn: hwid?.enabled ?? false,
    devLimit: String(hwid?.fallbackDeviceLimit ?? 1),
    devMsg: hwid?.maxDevicesAnnounce ?? '',
    remOn: data.sub?.isShowCustomRemarks ?? false,
    rem,
    mode: data.connect.mode,
    look: data.connect.page_config ?? data.configs[0]?.uuid ?? '',
  };
}

function limitBad(value: string): boolean {
  return !/^\d+$/.test(value) || +value < 1 || +value > 99;
}

function maskEnv(env: string): string {
  return env.replace(/(WEBHOOK_SECRET_HEADER=)(.+)$/m, (_, key: string, secret: string) =>
    secret.length > 8 && !secret.includes('…') ? `${key}${secret.slice(0, 4)}…${secret.slice(-4)}` : `${key}${secret}`,
  );
}

export function SettingsRemna() {
  const [data, setData] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [work, setWork] = useState<Work | null>(null);
  const [tokenEdit, setTokenEdit] = useState(false);
  const [note, setNote] = useState('');
  const [saveError, setSaveError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [checking, setChecking] = useState(false);
  const [copied, setCopied] = useState(false);

  const load = useCallback(async () => {
    try {
      const [remna, connect] = await Promise.all([
        api<RemnaView>('/api/admin/settings/remnawave'),
        api<Connect>('/api/admin/settings/connect'),
      ]);
      const [check, sub, configs] = await Promise.all([
        remna.url ? api<Check>('/api/admin/settings/remnawave/check', { method: 'POST' }).catch(() => null) : Promise.resolve(null),
        api<SubSettings>('/api/admin/settings/subscription').catch(() => null),
        api<PageConfig[]>('/api/admin/settings/remnawave/page-configs').catch(() => [] as PageConfig[]),
      ]);
      const loaded = { remna, check, sub, connect, configs };
      setData(loaded);
      setWork(workFrom(loaded));
      setTokenEdit(false);
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
  const linkChanged = work.addr.trim() !== base.addr.trim() || (work.token !== null && work.token.trim() !== '');
  const connectChanged = work.mode !== base.mode || (work.mode === 'remnawave' && work.look !== base.look);
  const dirty = remChanged || devChanged || linkChanged || connectChanged;
  const addrBad = noAddr;
  const remBad = work.remOn && REMARKS.some(([key]) => !work.rem[key].trim());
  const limBad = work.devOn && limitBad(work.devLimit);
  const invalid = addrBad || remBad || limBad;
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
    setChecking(true);
    try {
      const body: Record<string, string> = { url: work.addr.trim() };
      if (work.token && work.token.trim()) {
        body.token = work.token.trim();
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
          checked_at: new Date().toISOString(),
        },
      });
    } finally {
      setChecking(false);
    }
  };

  const save = async () => {
    if (!dirty || invalid || blocked || busy) {
      return;
    }
    setBusy(true);
    setSaveError(null);
    try {
      if (linkChanged) {
        const body: Record<string, string> = { url: work.addr.trim() };
        if (work.token && work.token.trim()) {
          body.token = work.token.trim();
        }
        await api('/api/admin/settings/remnawave', { method: 'PUT', body });
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
      if (connectChanged) {
        await api('/api/admin/settings/connect', {
          method: 'PUT',
          body: { mode: work.mode, page_config: work.mode === 'remnawave' ? work.look || null : data.connect.page_config },
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

  const saveText = blocked
    ? 'Нет связи с панелью: замените токен, чтобы сохранить'
    : invalid
      ? addrBad || remBad || (work.devOn && !work.devLimit)
        ? 'Заполните пустые поля'
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
              : 'Кабинет и мини-приложение показывают кнопку «Открыть страницу подключения», она ведёт на страницу подписки Remnawave'}
          </span>
        </div>
        {work.mode === 'remnawave' ? (
          <div className="row">
            <label htmlFor="cp-look" className="rl">
              Оформление страницы
            </label>
            <div className="rc">
              {data.configs.length > 0 ? (
                <select id="cp-look" className="field" value={work.look} onChange={(e) => edit({ look: e.target.value })} style={{ maxWidth: 280 }}>
                  {data.configs.map((config) => (
                    <option key={config.uuid} value={config.uuid}>
                      {config.name}
                    </option>
                  ))}
                </select>
              ) : (
                <span className="cap" style={{ paddingTop: 10 }}>
                  {linkOk ? 'В Remnawave нет оформлений страницы подписки' : 'Список оформлений появится, когда будет связь с панелью'}
                </span>
              )}
            </div>
          </div>
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
