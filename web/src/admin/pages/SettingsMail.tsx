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

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
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

  const toggle = async () => {
    if (!view || busy) {
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      setView(await api<ModuleView>('/api/admin/modules/mail', { method: 'PATCH', body: { enabled: !view.active } }));
    } catch (err) {
      setStatus({ text: failText(err, 'Почта не переключилась'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!cfg || busy) {
      return;
    }
    const lacking = missingRequired(cfg.schema, cfg.config, values, secrets);
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
      const list = await api<ModulesData>('/api/admin/modules');
      setView(list.modules.find((m) => m.info.id === 'mail') ?? view);
      setStatus({ text: 'Сохранено', bad: false });
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

  const filled = (key: string) => String(cfg.config[key] ?? '').trim() !== '';
  const configured = filled('smtp_host') && filled('from_email');
  const state = !view.active ? 'Письма не отправляются' : !configured ? 'Почта включена, но подключение не заполнено' : 'Письма отправляются';

  return (
    <SettingsLayout>
      <div className="sec-stack">
        <section className="sec" aria-labelledby="mail-h">
          <div className="sec-head">
            <h2 id="mail-h" className="section-title">
              Почта
            </h2>
            <span className="muted">
              Письма уходят через ваш почтовый сервер по SMTP. Сейчас письмом приходят коды входа в кабинет; без почты вход на сайте работает только через
              Telegram. Уведомления клиентам в Telegram отправляет бот, отдельно их настраивать не нужно.
            </span>
          </div>
          <div className="row adm-row">
            <span className="adm-who">
              <span className="adm-email">{state}</span>
              <span className="adm-status">Выключенная почта не отправляет ни одного письма, вход по почте в кабинете пропадает.</span>
            </span>
            <span className="adm-acts">
              <Switch on={view.active} onToggle={() => void toggle()} label="Отправлять письма" disabled={busy || view.locked !== null} />
            </span>
          </div>
          <div className="row mail-fields">
            <SchemaFields schema={cfg.schema} config={cfg.config} values={values} onChange={setValues} secrets={secrets} onSecrets={setSecrets} busy={busy} idPrefix="mail" />
          </div>
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
        <section className="sec" aria-labelledby="mail-test-h">
          <div className="sec-head">
            <h2 id="mail-test-h" className="section-title">
              Проверка
            </h2>
            <span className="muted">Отправьте письмо себе, чтобы убедиться, что сервер принимает логин и письма доходят.</span>
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
                <button type="submit" className="btn-main btn" disabled={testing || !to.trim() || !view.active}>
                  {testing ? 'Отправляем…' : 'Отправить проверку'}
                </button>
              </span>
              {!view.active ? <span className="cap">Сначала включите отправку писем и сохраните настройки.</span> : null}
              {test ? (
                <span className={test.bad ? 'err' : 'cap'} role={test.bad ? 'alert' : 'status'}>
                  {test.text}
                </span>
              ) : null}
            </span>
          </form>
        </section>
      </div>
    </SettingsLayout>
  );
}
