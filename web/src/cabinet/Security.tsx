import { Fragment, useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong } from '../lib/format';
import { Icon } from '../lib/icons';
import { QrCode } from '../lib/qr';
import { supportTickets } from '../lib/support';
import { Card, useCabinet } from './CabinetApp';
import type { TwoFactor } from './model';
import { finishOauth, leaveFor, PROVIDER_TITLE, ProviderMark, returnedProvider, type OauthProvider } from './oauth';

type Setup = { secret: string; uri: string };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function grouped(secret: string): string {
  return secret.replace(/(.{4})/g, '$1 ').trim();
}

function digits(value: string): string {
  return value.replace(/\D/g, '').slice(0, 6);
}

export function SecurityPage() {
  const { me, site, reload } = useCabinet();
  const [state, setState] = useState<TwoFactor>(me.two_factor);
  const [setup, setSetup] = useState<Setup | null>(null);
  const [turningOff, setTurningOff] = useState(false);
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let alive = true;
    api<TwoFactor>('/api/client/security').then(
      (next) => {
        if (alive) {
          setState(next);
        }
      },
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, []);

  const begin = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setSetup(await api<Setup>('/api/client/security/totp/setup', { method: 'POST', body: {} }));
      setCode('');
    } catch (err) {
      setError(failText(err, 'Настройка не началась, попробуйте ещё раз'));
    } finally {
      setBusy(false);
    }
  };

  const enable = async (event: FormEvent) => {
    event.preventDefault();
    if (!setup) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await api<TwoFactor>('/api/client/security/totp/enable', { method: 'POST', body: { secret: setup.secret, code } });
      setState(next);
      setSetup(null);
      setCode('');
      setNotice('Вход по коду включён. При следующем входе на сайт понадобится код из приложения.');
      void reload();
    } catch (err) {
      setError(failText(err, 'Код не подошёл'));
    } finally {
      setBusy(false);
    }
  };

  const disable = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const next = await api<TwoFactor>('/api/client/security/totp/disable', { method: 'POST', body: { code } });
      setState(next);
      setTurningOff(false);
      setCode('');
      setNotice('Вход по коду выключен.');
      void reload();
    } catch (err) {
      setError(failText(err, 'Код не подошёл'));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    if (!setup) {
      return;
    }
    try {
      await navigator.clipboard.writeText(setup.secret);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setError('Браузер не дал скопировать: выделите ключ вручную');
    }
  };

  const stop = () => {
    setSetup(null);
    setTurningOff(false);
    setCode('');
    setError(null);
  };

  const codeField = (
    <label className="cb-field">
      <span>Код из приложения</span>
      <input value={code} onChange={(e) => setCode(digits(e.target.value))} inputMode="numeric" autoComplete="one-time-code" placeholder="6 цифр" />
    </label>
  );

  return (
    <div className="cb-secure">
      <h1 className="cb-title">Безопасность</h1>
      <LoginsCard />
      <PasswordCard />
      <Card label="Вход по коду из приложения">
        <div className="cb-row wrap">
          <h2 className="cb-h2">Вход по коду из приложения</h2>
          <span className={state.enabled ? 'cb-status ink2' : 'cb-status muted'}>{state.enabled ? 'Включён' : 'Выключен'}</span>
        </div>
        <p className="cb-note">
          После подтверждения в Telegram или кода из письма сайт спросит ещё шестизначный код из приложения-аутентификатора, например Google Authenticator или Яндекс Ключ. В мини-приложении внутри Telegram код не нужен.
        </p>
        {state.enabled && state.enabled_at ? <span className="cb-small cb-muted">Включён {dateLong(state.enabled_at)}</span> : null}
        {notice ? (
          <p className="cb-result" role="status">
            {notice}
          </p>
        ) : null}
        {!state.enabled && !setup ? (
          <span className="cb-btns">
            <button type="button" className="cb-main" onClick={() => void begin()} disabled={busy}>
              Включить
            </button>
          </span>
        ) : null}
        {setup ? (
          <form className="cb-secure-form" onSubmit={enable}>
            <ol className="cb-steps">
              <li>
                <span className="cb-num">1</span>
                <span className="cb-step">
                  <span className="cb-step-text">
                    <strong>Добавьте {site.brand_name} в приложение</strong>
                    <span className="cb-small cb-muted">Отсканируйте QR-код камерой приложения или введите ключ вручную</span>
                  </span>
                  <QrCode text={setup.uri} size={176} label="QR-код для приложения-аутентификатора" />
                  <span className="cb-linkrow">
                    <span className="cb-link mono">{grouped(setup.secret)}</span>
                    <button type="button" className="cb-second" onClick={() => void copy()}>
                      {copied ? 'Скопировано' : 'Скопировать ключ'}
                    </button>
                  </span>
                </span>
              </li>
              <li>
                <span className="cb-num">2</span>
                <span className="cb-step">
                  <span className="cb-step-text">
                    <strong>Введите код, который покажет приложение</strong>
                    <span className="cb-small cb-muted">Код меняется каждые 30 секунд</span>
                  </span>
                  {codeField}
                </span>
              </li>
            </ol>
            <span className="cb-btns">
              <button type="submit" className="cb-main" disabled={busy || code.length !== 6}>
                Включить вход по коду
              </button>
              <button type="button" className="cb-cancel" onClick={stop} disabled={busy}>
                Отменить
              </button>
            </span>
          </form>
        ) : null}
        {state.enabled && !turningOff ? (
          <span className="cb-btns">
            <button
              type="button"
              className="cb-second"
              onClick={() => {
                setTurningOff(true);
                setCode('');
                setNotice(null);
              }}
            >
              Выключить
            </button>
          </span>
        ) : null}
        {state.enabled && turningOff ? (
          <form className="cb-secure-form" onSubmit={disable}>
            <p className="cb-ink2">Чтобы выключить вход по коду, введите код из приложения.</p>
            {codeField}
            <span className="cb-btns">
              <button type="submit" className="cb-main" disabled={busy || code.length !== 6}>
                Выключить вход по коду
              </button>
              <button type="button" className="cb-cancel" onClick={stop} disabled={busy}>
                Отменить
              </button>
            </span>
          </form>
        ) : null}
        {error ? (
          <p className="cb-error" role="alert">
            {error}
          </p>
        ) : null}
        <p className="cb-small cb-muted">
          Если телефон с приложением потерян,{' '}
          {supportTickets(site) ? (
            <Link className="cb-accent-link" to="/cabinet/help">
              напишите в поддержку
            </Link>
          ) : site.support_url ? (
            <a className="cb-accent-link" href={site.support_url} target="_blank" rel="noreferrer">
              напишите в поддержку
            </a>
          ) : (
            'напишите в поддержку'
          )}
          : вход по коду сбросят, и его можно будет включить заново.
        </p>
      </Card>
    </div>
  );
}

function PasswordCard() {
  const { me, site, reload } = useCabinet();
  const [current, setCurrent] = useState('');
  const [fresh, setFresh] = useState('');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; bad: boolean } | null>(null);
  if (!me.email || site.email_auth !== 'password') {
    return null;
  }
  const has = me.has_password === true;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setResult(null);
    try {
      await api('/api/client/security/password', { method: 'POST', body: { current: has ? current : undefined, password: fresh } });
      setCurrent('');
      setFresh('');
      setResult({ text: has ? 'Пароль изменён. На других устройствах кабинет попросит войти заново.' : 'Пароль задан. Теперь можно входить по почте и паролю.', bad: false });
      await reload();
    } catch (err) {
      setResult({ text: err instanceof ApiError ? err.message : 'Пароль не сохранился', bad: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card label="Пароль">
      <div className="cb-row wrap">
        <h2 className="cb-h2">Пароль</h2>
        <span className="cb-status muted">{me.email}</span>
      </div>
      <p className="cb-note">{has ? 'Пароль для входа на сайт по почте.' : 'Пароля пока нет: вы входили через Telegram или по коду из письма. Задайте его, чтобы входить по почте и паролю.'}</p>
      <form className="cb-secure-form" onSubmit={submit}>
        {has ? (
          <label className="cb-field">
            <span>Текущий пароль</span>
            <input type="password" value={current} onChange={(e) => setCurrent(e.target.value)} autoComplete="current-password" />
          </label>
        ) : null}
        <label className="cb-field">
          <span>Новый пароль</span>
          <input type="password" value={fresh} onChange={(e) => setFresh(e.target.value)} autoComplete="new-password" placeholder="Не короче 10 символов" />
        </label>
        {result ? (
          <p className={result.bad ? 'cb-error' : 'cb-result'} role={result.bad ? 'alert' : 'status'}>
            {result.text}
          </p>
        ) : null}
        <span className="cb-btns">
          <button type="submit" className="cb-main" disabled={busy || fresh.length < 10 || (has && !current)}>
            {busy ? 'Сохраняем…' : has ? 'Сменить пароль' : 'Задать пароль'}
          </button>
        </span>
      </form>
    </Card>
  );
}

type LoginKind = 'telegram' | 'email' | 'google' | 'apple';
type LoginMethod = { kind: LoginKind; account: string; linked_at: string; removable: boolean };
type Logins = { methods: LoginMethod[]; can_link: OauthProvider[] };

const KIND_TITLE: Record<LoginKind, string> = { telegram: 'Telegram', email: 'Почта', google: 'Google', apple: 'Apple' };

function isProvider(kind: LoginKind): kind is OauthProvider {
  return kind === 'google' || kind === 'apple';
}

function KindMark({ kind }: { kind: LoginKind }) {
  if (isProvider(kind)) {
    return <ProviderMark provider={kind} />;
  }
  return <Icon name={kind === 'telegram' ? 'send' : 'mail'} size={20} />;
}

function LoginsCard() {
  const { phone } = useCabinet();
  const navigate = useNavigate();
  const [data, setData] = useState<Logins | null>(null);
  const [ask, setAsk] = useState<OauthProvider | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const finished = useRef(false);

  const load = useCallback(async () => {
    try {
      setData(await api<Logins>('/api/client/security/logins'));
    } catch (err) {
      setError(failText(err, 'Способы входа не загрузились'));
    }
  }, []);

  useEffect(() => {
    const back = returnedProvider();
    if (!back || finished.current) {
      void load();
      return;
    }
    finished.current = true;
    finishOauth()
      .then(
        (res) => {
          if (res.status === 'linked') {
            setNotice(`Вход через ${PROVIDER_TITLE[res.provider]} привязан. Теперь им можно входить в кабинет.`);
          }
        },
        (err) => setError(failText(err, 'Привязка не завершилась, попробуйте ещё раз')),
      )
      .finally(() => {
        navigate('/cabinet/security', { replace: true });
        void load();
      });
  }, [load, navigate]);

  const link = async (provider: OauthProvider) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await leaveFor(`/api/client/security/logins/${provider}`);
    } catch (err) {
      setError(failText(err, 'Привязка не началась'));
      setBusy(false);
    }
  };

  const unlink = async (provider: OauthProvider) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setData(await api<Logins>(`/api/client/security/logins/${provider}`, { method: 'DELETE' }));
      setAsk(null);
      setNotice(`Вход через ${PROVIDER_TITLE[provider]} отвязан.`);
    } catch (err) {
      setError(failText(err, 'Не получилось отвязать'));
    } finally {
      setBusy(false);
    }
  };

  const shown = data !== null && (data.can_link.length > 0 || data.methods.some((m) => isProvider(m.kind)));
  if (!shown && !error) {
    return null;
  }

  return (
    <Card label="Способы входа" className="cb-logins">
      <h2 className="cb-h2">Способы входа</h2>
      <p className="cb-note">Войти в кабинет можно любым из них. Последний способ отвязать нельзя.</p>
      {data?.methods.map((m) => (
        <Fragment key={m.kind}>
          <div className="cb-device">
            <KindMark kind={m.kind} />
            <span className="cb-device-text">
              <strong>{KIND_TITLE[m.kind]}</strong>
              <span className="cb-small cb-muted">{m.account}</span>
            </span>
            {isProvider(m.kind) && m.removable ? (
              <button
                type="button"
                className={phone ? 'cb-text' : 'cb-second sm'}
                aria-label={`Отвязать ${KIND_TITLE[m.kind]}`}
                aria-expanded={ask === m.kind}
                onClick={() => setAsk(ask === m.kind ? null : (m.kind as OauthProvider))}
                disabled={busy}
              >
                Отвязать
              </button>
            ) : null}
          </div>
          {ask === m.kind ? (
            <div className="cb-ask">
              <p>
                <strong>Отвязать вход через {PROVIDER_TITLE[ask]}?</strong> Входить через этот аккаунт {PROVIDER_TITLE[ask]} больше не получится. Привязать его можно снова.
              </p>
              <span className="cb-ask-btns">
                <button type="button" className="cb-main sm" onClick={() => void unlink(ask)} disabled={busy}>
                  Отвязать
                </button>
                <button type="button" className="cb-cancel" onClick={() => setAsk(null)} disabled={busy}>
                  Отменить
                </button>
              </span>
            </div>
          ) : null}
        </Fragment>
      ))}
      {data && data.can_link.length > 0 ? (
        <span className="cb-btns">
          {data.can_link.map((p) => (
            <button key={p} type="button" className="cb-second cb-oauth-btn" onClick={() => void link(p)} disabled={busy}>
              <ProviderMark provider={p} />
              Привязать {PROVIDER_TITLE[p]}
            </button>
          ))}
        </span>
      ) : null}
      {notice ? (
        <p className="cb-result" role="status">
          {notice}
        </p>
      ) : null}
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
    </Card>
  );
}
