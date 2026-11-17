import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { changedConfig, initialValues, missingRequired, SchemaFields, type FieldValues, type SecretState } from '../SchemaFields';
import type { ModuleConfig, ModulesData, ModuleView } from '../modules/model';
import { useSession } from '../session';
import '../form.css';
import './admins.css';
import { SettingsLayout } from './SettingsLayout';

const NO_SECRETS: SecretState = { clear: [], replacing: [] };

type Transport = 'none' | 'smtp' | 'resend';

const TRANSPORTS: Array<[Transport, string]> = [
  ['none', 'Без подтверждения'],
  ['smtp', 'SMTP'],
  ['resend', 'Resend API'],
];

const FIELDS: Record<Transport, string[]> = {
  none: [],
  smtp: ['smtp_host', 'security', 'smtp_port', 'username', 'password', 'from_email', 'from_name'],
  resend: ['resend_api_key', 'from_email', 'from_name'],
};

const ABOUT: Record<Transport, string> = {
  none: 'Клиенты регистрируются на сайте по почте и паролю, письма не отправляются. Восстановить пароль сами они не смогут — только через поддержку.',
  smtp: 'Вход на сайте по коду из письма. Письма уходят через ваш почтовый сервер: Яндекс, Mail.ru, Gmail или свой.',
  resend: 'Вход на сайте по коду из письма. Письма уходят через сервис Resend по ключу API; домен адреса отправителя должен быть подтверждён в Resend.',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function asTransport(value: unknown): Transport {
  return value === 'smtp' || value === 'resend' ? value : 'none';
}

export function SettingsMail() {
  const { me } = useSession();
  const [view, setView] = useState<ModuleView | null>(null);
  const [cfg, setCfg] = useState<ModuleConfig | null>(null);
  const [values, setValues] = useState<FieldValues>({});
  const [secrets, setSecrets] = useState<SecretState>(NO_SECRETS);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [to, setTo] = useState(me.email ?? '');
  const [test, setTest] = useState<{ text: string; bad: boolean } | null>(null);
  const [testing, setTesting] = useState(false);

  const apply = (next: ModuleConfig) => {
    setCfg(next);
    setValues(initialValues(next.schema, next.config));
    setSecrets(NO_SECRETS);
  };

  const load = useCallback(async () => {
    try {
      const [list, config] = await Promise.all([api<ModulesData>('/api/admin/modules'), api<ModuleConfig>('/api/admin/modules/mail/config')]);
      setView(list.modules.find((m) => m.info.id === 'mail') ?? null);
      apply(config);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Настройки почты не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const enable = async () => {
    setBusy(true);
    setStatus(null);
    try {
      setView(await api<ModuleView>('/api/admin/modules/mail', { method: 'PATCH', body: { enabled: true } }));
    } catch (err) {
      setStatus({ text: failText(err, 'Почта не включилась'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  if (!cfg || !view) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем настройки почты…</p>
        )}
      </SettingsLayout>
    );
  }

  const transport = asTransport(values.transport);
  const saved = asTransport(cfg.config.transport);
  const visible = cfg.schema.filter((field) => FIELDS[transport].includes(field.key));
  const sends = view.active && saved !== 'none';

  const save = async () => {
    if (busy) {
      return;
    }
    const lacking = missingRequired(visible, cfg.config, values, secrets);
    if (lacking.length) {
      setStatus({ text: `Заполните: ${lacking.map((f) => f.label).join(', ')}`, bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      apply(
        await api<ModuleConfig>('/api/admin/modules/mail/config', {
          method: 'PUT',
          body: { config: changedConfig(cfg.schema, cfg.config, values), clear: secrets.clear },
        }),
      );
      setStatus({ text: 'Сохранено', bad: false });
      setTest(null);
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const sendTest = async () => {
    if (!to.trim() || testing) {
      return;
    }
    setTesting(true);
    setTest(null);
    try {
      await api('/api/admin/mail/test', { method: 'POST', body: { to: to.trim() } });
      setTest({ text: `Письмо отправлено на ${to.trim()}. Если его нет во «Входящих», проверьте «Спам».`, bad: false });
    } catch (err) {
      setTest({ text: failText(err, 'Письмо не отправилось'), bad: true });
    } finally {
      setTesting(false);
    }
  };

  return (
    <SettingsLayout>
      <div className="sec-stack">
        <section className="sec" aria-labelledby="mail-h">
          <div className="sec-head">
            <h2 id="mail-h" className="section-title">
              Регистрация по почте
            </h2>
            <span className="muted">Как клиенты входят в кабинет на сайте по почте. Уведомления клиентам в Telegram отправляет бот, отдельно их настраивать не нужно.</span>
          </div>
          {!view.active ? (
            <div className="row adm-row">
              <span className="adm-who">
                <span className="adm-email">Модуль «Почта» выключен</span>
                <span className="adm-status">Письма не отправляются, вход по почте работает только по паролю.</span>
              </span>
              <span className="adm-acts">
                <button type="button" className="b34 hov" onClick={() => void enable()} disabled={busy}>
                  Включить
                </button>
              </span>
            </div>
          ) : null}
          <div className="row">
            <span className="rc">
              <span role="group" aria-label="Подтверждение почты" className="segs">
                {TRANSPORTS.map(([key, label]) => (
                  <button
                    key={key}
                    type="button"
                    aria-pressed={transport === key}
                    onClick={() => {
                      setValues({ ...values, transport: key });
                      setStatus(null);
                    }}
                    disabled={busy}
                  >
                    {label}
                  </button>
                ))}
              </span>
              <span className="cap">{ABOUT[transport]}</span>
            </span>
          </div>
          {visible.length ? (
            <div className="row mail-fields">
              <SchemaFields schema={visible} config={cfg.config} values={values} onChange={setValues} secrets={secrets} onSecrets={setSecrets} busy={busy} idPrefix="mail" />
            </div>
          ) : null}
          <div className="row">
            <span className="rc">
              {status ? (
                <span className={status.bad ? 'err' : 'cap'} role={status.bad ? 'alert' : 'status'}>
                  {status.text}
                </span>
              ) : null}
              <span>
                <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
                  {busy ? 'Сохраняем…' : 'Сохранить'}
                </button>
              </span>
            </span>
          </div>
        </section>
        {sends ? (
          <section className="sec" aria-labelledby="mail-test-h">
            <div className="sec-head">
              <h2 id="mail-test-h" className="section-title">
                Проверка
              </h2>
              <span className="muted">Отправьте письмо себе, чтобы убедиться, что сервис принимает настройки и письма доходят.</span>
            </div>
            <form
              className="row"
              onSubmit={(e) => {
                e.preventDefault();
                void sendTest();
              }}
            >
              <span className="rc">
                <span className="adm-new-fields">
                  <input
                    className="field"
                    type="email"
                    aria-label="Куда отправить проверочное письмо"
                    placeholder="Куда отправить"
                    value={to}
                    onChange={(e) => setTo(e.target.value)}
                    disabled={testing}
                  />
                  <button type="submit" className="btn-main btn" disabled={testing || !to.trim()}>
                    {testing ? 'Отправляем…' : 'Отправить проверку'}
                  </button>
                </span>
                {test ? (
                  <span className={test.bad ? 'err' : 'cap'} role={test.bad ? 'alert' : 'status'}>
                    {test.text}
                  </span>
                ) : null}
              </span>
            </form>
          </section>
        ) : null}
      </div>
    </SettingsLayout>
  );
}
