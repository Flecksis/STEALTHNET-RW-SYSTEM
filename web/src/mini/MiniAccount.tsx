import { useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, timeHM } from '../lib/format';
import { NATIVE, siteLangs, t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { QrCode } from '../lib/qr';
import { doneText, sixDigits, useEmailChange } from '../cabinet/email';
import type { TwoFactor } from '../cabinet/model';
import { BackHead, useMini } from './MiniApp';

type Setup = { secret: string; uri: string };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function grouped(secret: string): string {
  return secret.replace(/(.{4})/g, '$1 ').trim();
}

export function AccountCard() {
  const { me, site, lang } = useMini();
  const siteLogin = site.client_access?.site_login !== false;
  const langs = siteLangs(site);
  return (
    <section aria-label={t('Аккаунт')} className="mn-devsec">
      <div className="mn-devsec-head">
        <h2>{t('Аккаунт')}</h2>
      </div>
      <div className="mn-list">
        {me.email_change ? (
          <Link to="/app/profile/email" className="mn-row press">
            <Icon name="mail" size={21} />
            <span className="mn-row-name">{t('Почта')}</span>
            <span className="mn-row-val cut">{me.email ?? t('не указана')}</span>
            <Icon name="chevron-right" size={16} />
          </Link>
        ) : (
          <div className="mn-row mn-ticket still">
            <Icon name="mail" size={21} />
            <span className="mn-ticket-text">
              <span className="mn-row-name">{t('Почта')}</span>
              <span className="mn-tone muted">{me.email ?? t('не указана')}</span>
              <span className="mn-tone muted">{t('Сменить нельзя: письма сейчас не отправляются')}</span>
            </span>
          </div>
        )}
        {siteLogin && site.email_auth === 'password' ? (
          <Link to="/app/profile/password" className="mn-row press">
            <Icon name="lock" size={21} />
            <span className="mn-row-name">{t('Пароль для сайта')}</span>
            <span className="mn-row-val">{me.has_password ? t('задан') : t('не задан')}</span>
            <Icon name="chevron-right" size={16} />
          </Link>
        ) : null}
        {siteLogin ? (
          <Link to="/app/profile/totp" className="mn-row press">
            <Icon name="phone" size={21} />
            <span className="mn-row-name">{t('Вход по коду')}</span>
            <span className="mn-row-val">{me.two_factor.enabled ? t('включён') : t('выключен')}</span>
            <Icon name="chevron-right" size={16} />
          </Link>
        ) : null}
        {langs.length > 1 ? (
          <Link to="/app/profile/language" className="mn-row press">
            <Icon name="globe" size={21} />
            <span className="mn-row-name">{t('Язык')}</span>
            <span className="mn-row-val">{NATIVE[lang]}</span>
            <Icon name="chevron-right" size={16} />
          </Link>
        ) : null}
      </div>
    </section>
  );
}

export function MiniEmail() {
  const { me, site, reload } = useMini();
  const flow = useEmailChange(reload);
  const { info, sent } = flow;
  const siteLogin = site.client_access?.site_login !== false;
  const passwordNext = !!flow.done && siteLogin && site.email_auth === 'password' && me.has_password !== true;
  const codeNext = !!flow.done && siteLogin && site.email_auth === 'code';

  if (!info) {
    return (
      <main className="mn-page">
        <BackHead title={t('Почта')} to="/app/profile" />
        <p className={flow.failed ? 'mn-error' : 'mn-muted'}>{flow.failed ?? t('Загружаем…')}</p>
      </main>
    );
  }

  const note =
    info.proof === 'totp'
      ? t('Для смены почты нужен код из приложения-аутентификатора, затем код из письма на новый адрес.')
      : info.proof === 'password'
        ? t('Для смены почты нужен пароль для сайта, затем код из письма на новый адрес.')
        : t('На новый адрес придёт код. Почта сменится, когда вы его введёте.');

  return (
    <main className="mn-page">
      <BackHead title={t('Почта')} to="/app/profile" />
      <div className="mn-card mn-pad">
        <span className="mn-muted">{t('Текущая почта')}</span>
        <strong className="mn-big mn-wrap">{info.email ?? t('Почта не указана')}</strong>
        <span className="mn-muted small">{siteLogin ? t('На эту почту приходят письма, по ней же можно входить на сайт.') : t('На эту почту приходят письма.')}</span>
      </div>
      {flow.done ? (
        <p className="mn-note" role="status">
          {doneText(flow.done)}
        </p>
      ) : null}
      {passwordNext ? (
        <div className="mn-card mn-pad">
          <strong>{t('Теперь задайте пароль для сайта')}</strong>
          <span className="mn-muted small">{t('С почтой и паролем можно войти в кабинет на сайте, если Telegram не работает.')}</span>
          <Link to="/app/profile/password" className="mn-main sm press mn-btn-link">
            {t('Задать пароль')}
          </Link>
        </div>
      ) : null}
      {codeNext ? <p className="mn-notes">{t('Теперь на сайт можно входить по этой почте: код для входа придёт в письме.')}</p> : null}
      {!info.available ? (
        <p className="mn-note">{t('Письма сейчас не отправляются, поэтому сменить почту нельзя.')}</p>
      ) : sent ? (
        <>
          <p className="mn-notes">{t('Код отправлен на {email}. Он действует до {time}.', { email: sent.email, time: timeHM(sent.expires_at) })}</p>
          <label className="mn-field">
            <span className="sr-only">{t('Код из письма')}</span>
            <input
              value={flow.code}
              onChange={(e) => flow.setCode(sixDigits(e.target.value))}
              inputMode="numeric"
              autoComplete="one-time-code"
              placeholder={t('Код из письма')}
              disabled={flow.busy}
            />
          </label>
          {flow.error ? (
            <p className="mn-error" role="alert">
              {flow.error}
            </p>
          ) : null}
          <button type="button" className="mn-main sm press" onClick={() => void flow.confirm()} disabled={flow.busy || flow.code.length !== 6}>
            {flow.busy ? t('Проверяем…') : t('Подтвердить')}
          </button>
          <button type="button" className="mn-text press" onClick={flow.restart} disabled={flow.busy}>
            {t('Указать другой адрес')}
          </button>
        </>
      ) : (
        <>
          <label className="mn-field">
            <span className="sr-only">{t('Новая почта')}</span>
            <input
              type="email"
              value={flow.address}
              onChange={(e) => flow.setAddress(e.target.value)}
              inputMode="email"
              autoComplete="email"
              placeholder={t('Новая почта')}
              maxLength={254}
              disabled={flow.busy}
            />
          </label>
          {info.proof === 'password' ? (
            <label className="mn-field">
              <span className="sr-only">{t('Пароль для сайта')}</span>
              <input
                type="password"
                value={flow.proof}
                onChange={(e) => flow.setProof(e.target.value)}
                autoComplete="current-password"
                placeholder={t('Пароль для сайта')}
                disabled={flow.busy}
              />
            </label>
          ) : null}
          {info.proof === 'totp' ? (
            <label className="mn-field">
              <span className="sr-only">{t('Код из приложения')}</span>
              <input
                value={flow.proof}
                onChange={(e) => flow.setProof(sixDigits(e.target.value))}
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder={t('Код из приложения')}
                disabled={flow.busy}
              />
            </label>
          ) : null}
          <p className="mn-notes">{note}</p>
          {flow.error ? (
            <p className="mn-error" role="alert">
              {flow.error}
            </p>
          ) : null}
          <button type="button" className="mn-main sm press" onClick={() => void flow.start()} disabled={flow.busy || !flow.ready}>
            {flow.busy ? t('Отправляем…') : t('Получить код')}
          </button>
        </>
      )}
    </main>
  );
}

export function MiniPassword() {
  const { me, reload } = useMini();
  const [current, setCurrent] = useState('');
  const [fresh, setFresh] = useState('');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; bad: boolean } | null>(null);
  const has = me.has_password === true;

  const submit = async () => {
    if (busy) {
      return;
    }
    setBusy(true);
    setResult(null);
    try {
      await api('/api/client/security/password', { method: 'POST', body: { current: has ? current : undefined, password: fresh } });
      setCurrent('');
      setFresh('');
      setResult({ text: has ? t('Пароль изменён. На сайте нужно будет войти заново.') : t('Пароль задан. Теперь на сайт можно входить по почте и паролю.'), bad: false });
      await reload();
    } catch (err) {
      setResult({ text: failText(err, t('Пароль не сохранился')), bad: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="mn-page">
      <BackHead title={t('Пароль для сайта')} to="/app/profile" />
      {!me.email ? (
        <div className="mn-card mn-pad">
          <p>{t('Сначала добавьте почту: на сайт входят по почте и паролю.')}</p>
          {me.email_change ? (
            <Link to="/app/profile/email" className="mn-btn2 press mn-btn-link">
              {t('Добавить почту')}
            </Link>
          ) : (
            <span className="mn-muted small">{t('Письма сейчас не отправляются, поэтому добавить почту нельзя.')}</span>
          )}
        </div>
      ) : (
        <>
          <div className="mn-card mn-pad">
            <span className="mn-muted">{t('Вход на сайт по почте')}</span>
            <strong className="mn-big mn-wrap">{me.email}</strong>
            <span className="mn-muted small">
              {has ? t('Пароль задан. В мини-приложении он не нужен, его спрашивает только сайт.') : t('Пароля пока нет. Задайте его, чтобы входить на сайт по почте и паролю.')}
            </span>
          </div>
          {has ? (
            <label className="mn-field">
              <span className="sr-only">{t('Текущий пароль')}</span>
              <input type="password" value={current} onChange={(e) => setCurrent(e.target.value)} autoComplete="current-password" placeholder={t('Текущий пароль')} disabled={busy} />
            </label>
          ) : null}
          <label className="mn-field">
            <span className="sr-only">{t('Новый пароль')}</span>
            <input type="password" value={fresh} onChange={(e) => setFresh(e.target.value)} autoComplete="new-password" placeholder={t('Новый пароль, не короче 10 символов')} disabled={busy} />
          </label>
          {result ? (
            <p className={result.bad ? 'mn-error' : 'mn-note'} role={result.bad ? 'alert' : 'status'}>
              {result.text}
            </p>
          ) : null}
          <button type="button" className="mn-main sm press" onClick={() => void submit()} disabled={busy || fresh.length < 10 || (has && !current)}>
            {busy ? t('Сохраняем…') : has ? t('Сменить пароль') : t('Задать пароль')}
          </button>
        </>
      )}
    </main>
  );
}

export function MiniTotp() {
  const { me, site, reload } = useMini();
  const [state, setState] = useState<TwoFactor>(me.two_factor);
  const [setup, setSetup] = useState<Setup | null>(null);
  const [turningOff, setTurningOff] = useState(false);
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const begin = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setSetup(await api<Setup>('/api/client/security/totp/setup', { method: 'POST', body: {} }));
      setCode('');
    } catch (err) {
      setError(failText(err, t('Настройка не началась, попробуйте ещё раз')));
    } finally {
      setBusy(false);
    }
  };

  const enable = async () => {
    if (!setup || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setState(await api<TwoFactor>('/api/client/security/totp/enable', { method: 'POST', body: { secret: setup.secret, code } }));
      setSetup(null);
      setCode('');
      setNotice(t('Вход по коду включён. При следующем входе на сайт понадобится код из приложения.'));
      await reload();
    } catch (err) {
      setError(failText(err, t('Код не подошёл')));
    } finally {
      setBusy(false);
    }
  };

  const disable = async () => {
    if (busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setState(await api<TwoFactor>('/api/client/security/totp/disable', { method: 'POST', body: { code } }));
      setTurningOff(false);
      setCode('');
      setNotice(t('Вход по коду выключен.'));
      await reload();
    } catch (err) {
      setError(failText(err, t('Код не подошёл')));
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
      setError(t('Не получилось скопировать: выделите ключ вручную'));
    }
  };

  const stop = () => {
    setSetup(null);
    setTurningOff(false);
    setCode('');
    setError(null);
  };

  const codeField = (
    <label className="mn-field">
      <span className="sr-only">{t('Код из приложения')}</span>
      <input value={code} onChange={(e) => setCode(sixDigits(e.target.value))} inputMode="numeric" autoComplete="one-time-code" placeholder={t('Код из приложения, 6 цифр')} disabled={busy} />
    </label>
  );
  const errorLine = error ? (
    <p className="mn-error" role="alert">
      {error}
    </p>
  ) : null;
  const [lostBefore, lostAfter] = t('Если телефон с приложением потерян, {link}: вход по коду сбросят, и его можно будет включить заново.').split('{link}');

  return (
    <main className="mn-page">
      <BackHead title={t('Вход по коду')} to="/app/profile" />
      <div className="mn-card mn-pad">
        <span className="mn-muted">{t('Вход на сайт по коду из приложения')}</span>
        <strong className="mn-big">{state.enabled ? t('Включён') : t('Выключен')}</strong>
        {state.enabled && state.enabled_at ? <span className="mn-muted small">{t('Включён {date}', { date: dateLong(state.enabled_at) })}</span> : null}
        <span className="mn-muted small">{t('Сайт спрашивает этот код при входе. В мини-приложении код не нужен: вход сюда защищает сам Telegram.')}</span>
      </div>
      {notice ? (
        <p className="mn-note" role="status">
          {notice}
        </p>
      ) : null}
      {!state.enabled && !setup ? (
        <>
          {errorLine}
          <button type="button" className="mn-main sm press" onClick={() => void begin()} disabled={busy}>
            {t('Включить')}
          </button>
        </>
      ) : null}
      {setup ? (
        <>
          <div className="mn-steps">
            <div className="mn-card mn-step col">
              <span className="mn-step-row">
                <span className="mn-num">1</span>
                <span className="mn-step-text">
                  <strong>{t('Добавьте {brand} в приложение', { brand: site.brand_name })}</strong>
                  <span>{t('Google Authenticator, Яндекс Ключ или другое приложение для кодов')}</span>
                </span>
              </span>
              <a href={setup.uri} className="mn-main sm press">
                {t('Открыть в приложении')}
              </a>
              <span className="mn-muted small">{t('Если кнопка не открыла приложение, добавьте ключ вручную:')}</span>
              <p className="mn-note mono">{grouped(setup.secret)}</p>
              <button type="button" className="mn-btn2 press" onClick={() => void copy()}>
                {copied ? t('Скопировано') : t('Скопировать ключ')}
              </button>
              <QrCode text={setup.uri} size={132} label={t('QR-код для приложения-аутентификатора')} />
            </div>
            <div className="mn-card mn-step">
              <span className="mn-num">2</span>
              <span className="mn-step-text">
                <strong>{t('Введите код, который покажет приложение')}</strong>
                <span>{t('Код меняется каждые 30 секунд')}</span>
              </span>
            </div>
          </div>
          {codeField}
          {errorLine}
          <button type="button" className="mn-main sm press" onClick={() => void enable()} disabled={busy || code.length !== 6}>
            {t('Включить вход по коду')}
          </button>
          <button type="button" className="mn-text press" onClick={stop} disabled={busy}>
            {t('Отменить')}
          </button>
        </>
      ) : null}
      {state.enabled && !turningOff ? (
        <button
          type="button"
          className="mn-btn2 press"
          onClick={() => {
            setTurningOff(true);
            setCode('');
            setNotice(null);
          }}
        >
          {t('Выключить')}
        </button>
      ) : null}
      {state.enabled && turningOff ? (
        <>
          <p className="mn-notes">{t('Чтобы выключить вход по коду, введите код из приложения.')}</p>
          {codeField}
          {errorLine}
          <button type="button" className="mn-main sm press" onClick={() => void disable()} disabled={busy || code.length !== 6}>
            {t('Выключить вход по коду')}
          </button>
          <button type="button" className="mn-text press" onClick={stop} disabled={busy}>
            {t('Отменить')}
          </button>
        </>
      ) : null}
      {state.enabled ? (
        <p className="mn-notes">
          {lostBefore}
          {t('напишите в поддержку')}
          {lostAfter}
        </p>
      ) : null}
    </main>
  );
}

export function MiniLanguage() {
  const { site, lang, switchLang } = useMini();
  return (
    <main className="mn-page">
      <BackHead title={t('Язык')} to="/app/profile" />
      <div role="radiogroup" aria-label={t('Язык')} className="mn-terms">
        {siteLangs(site).map((item) => (
          <button key={item} type="button" role="radio" aria-checked={lang === item} className="mn-term press" lang={item} onClick={() => item !== lang && switchLang(item)}>
            <span className="mn-dot" aria-hidden="true" />
            <span className="mn-term-text">
              <strong>{NATIVE[item]}</strong>
            </span>
          </button>
        ))}
      </div>
    </main>
  );
}
