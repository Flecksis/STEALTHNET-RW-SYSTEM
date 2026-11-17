import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { changedConfig, initialValues, missingRequired, SchemaFields, type FieldValues, type SecretState } from '../SchemaFields';
import type { ModuleConfig, ModulesData, ModuleView } from '../modules/model';
import { useSession } from '../session';
import { Switch } from '../ui';
import '../form.css';
import './admins.css';
import { SettingsLayout } from './SettingsLayout';

const NO_SECRETS: SecretState = { clear: [], replacing: [] };

type Transport = 'none' | 'smtp' | 'resend';
type Login = 'password' | 'code';

const TRANSPORTS: Array<[Transport, string]> = [
  ['none', 'Не отправлять'],
  ['smtp', 'SMTP'],
  ['resend', 'Resend API'],
];

const FIELDS: Record<Transport, string[]> = {
  none: [],
  smtp: ['smtp_host', 'security', 'smtp_port', 'username', 'password', 'from_email', 'from_name'],
  resend: ['resend_api_key', 'from_email', 'from_name'],
};

const ABOUT: Record<Transport, string> = {
  none: 'Письма не уходят: ни коды, ни напоминания.',
  smtp: 'Письма уходят через ваш почтовый сервер: Яндекс, Mail.ru, Gmail или свой.',
  resend: 'Письма уходят через сервис Resend по ключу API. Домен адреса отправителя должен быть подтверждён в Resend.',
};

const LOGINS: Array<[Login, string]> = [
  ['password', 'По паролю'],
  ['code', 'По коду из письма'],
];

const LETTERS: Array<[string, string, string]> = [
  ['letter_expiring', 'Окончание подписки', 'Напоминание перед концом срока и письма об автопродлении'],
  ['letter_payments', 'Оплата', 'Оплата получена, подписка выдана или продлена, баланс пополнен'],
  ['letter_support', 'Ответ поддержки', 'Текст ответа и ссылка на переписку в кабинете'],
];

const CUSTOM = new Set(['transport', 'login', ...LETTERS.map(([key]) => key)]);

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function asTransport(value: unknown): Transport {
  return value === 'smtp' || value === 'resend' ? value : 'none';
}

function loginOf(value: unknown, transport: Transport): Login {
  if (transport === 'none') {
    return 'password';
  }
  return value === 'password' || value === 'code' ? value : 'code';
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
    const initial = initialValues(next.schema, next.config);
    const saved = asTransport(next.config.transport);
    setValues({ ...initial, login: loginOf(next.config.login, saved) });
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
  const login = loginOf(values.login, transport);
  const visible = cfg.schema.filter((field) => FIELDS[transport].includes(field.key) && !CUSTOM.has(field.key));
  const sends = view.active && saved !== 'none';
  const edit = (patch: FieldValues) => {
    setValues({ ...values, ...patch });
    setStatus(null);
  };

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
      const config = changedConfig(cfg.schema, cfg.config, { ...values, login });
      if (cfg.config.login !== login) {
        config.login = login;
      }
      apply(
        await api<ModuleConfig>('/api/admin/modules/mail/config', {
          method: 'PUT',
          body: { config, clear: secrets.clear },
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

  const loginText =
    login === 'code'
      ? 'Пароль не нужен: при каждом входе на почту приходит код.'
      : transport === 'none'
        ? 'Клиент регистрируется с паролем, почту не подтверждает. Забытый пароль сам восстановить не сможет — его выдаёт поддержка: «Новый пароль» в карточке клиента.'
        : 'Клиент регистрируется с паролем. Забытый пароль меняет сам по коду из письма.';

  return (
    <SettingsLayout>
      <div className="sec-stack">
        <section className="sec" aria-labelledby="mail-h">
          <div className="sec-head">
            <h2 id="mail-h" className="section-title">
              Отправка писем
            </h2>
            <span className="muted">Через что сервис отправляет письма клиентам: коды, напоминания, ответы поддержки и рассылки</span>
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
              <span role="group" aria-label="Отправка писем" className="segs">
                {TRANSPORTS.map(([key, label]) => (
                  <button key={key} type="button" aria-pressed={transport === key} onClick={() => edit({ transport: key })} disabled={busy}>
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
        </section>

        <section className="sec" aria-labelledby="mail-login-h">
          <div className="sec-head">
            <h2 id="mail-login-h" className="section-title">
              Вход на сайт по почте
            </h2>
            <span className="muted">Как клиент входит в кабинет, если выбрал почту, а не Telegram</span>
          </div>
          <div className="row">
            <span className="rc">
              <span role="group" aria-label="Вход на сайт по почте" className="segs">
                {LOGINS.map(([key, label]) => (
                  <button key={key} type="button" aria-pressed={login === key} onClick={() => edit({ login: key })} disabled={busy || (key === 'code' && transport === 'none')}>
                    {label}
                  </button>
                ))}
              </span>
              <span className="cap">{loginText}</span>
            </span>
          </div>
        </section>

        {transport !== 'none' ? (
          <section className="sec" aria-labelledby="mail-letters-h">
            <div className="sec-head">
              <h2 id="mail-letters-h" className="section-title">
                Письма клиентам
              </h2>
              <span className="muted">Приходят тем, кому нельзя написать в Telegram: Telegram не привязан или клиент остановил бота</span>
            </div>
            {LETTERS.map(([key, title, text]) => (
              <div key={key} className="row adm-row">
                <span className="adm-who">
                  <span className="adm-email">{title}</span>
                  <span className="adm-status">{text}</span>
                </span>
                <span className="adm-acts">
                  <Switch on={values[key] === true} onToggle={() => edit({ [key]: values[key] !== true })} label={title} disabled={busy} />
                </span>
              </div>
            ))}
          </section>
        ) : null}

        <div className="save-row">
          <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
          <span role="status" className={status?.bad ? 'bad' : undefined}>
            {status?.text ?? ''}
          </span>
        </div>

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
