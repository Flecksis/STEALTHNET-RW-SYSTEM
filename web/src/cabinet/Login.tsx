import { useEffect, useRef, useState, type FormEvent } from 'react';
import { api, ApiError } from '../lib/api';
import { QrCode } from '../lib/qr';
import { useSurface, useSystemTheme } from '../lib/theme';
import type { Site } from '../lib/types';

type Tg = { poll_token: string; link: string; expires_at: string };
type Status = { status: 'pending' | 'confirmed' | 'rejected' | 'expired' };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function Login({ site, onDone }: { site: Site; onDone: () => void }) {
  const system = useSystemTheme();
  const choice = site.appearance.client_theme;
  const theme = choice === 'dark' || choice === 'light' ? choice : system;
  useSurface('cabinet', theme, site.appearance.client === 'mono' ? 'b' : 'a');
  const [tg, setTg] = useState<Tg | null>(null);
  const [tgState, setTgState] = useState<'idle' | 'waiting' | 'rejected' | 'expired'>('idle');
  const [mode, setMode] = useState<'telegram' | 'email'>('telegram');
  const [email, setEmail] = useState('');
  const [code, setCode] = useState('');
  const [sent, setSent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (timer.current) {
        window.clearTimeout(timer.current);
      }
    },
    [],
  );

  const poll = (token: string, expires: number) => {
    timer.current = window.setTimeout(async () => {
      if (Date.now() > expires + 5000) {
        setTgState('expired');
        return;
      }
      try {
        const res = await api<Status>('/api/client/auth/telegram/status', { method: 'POST', body: { poll_token: token } });
        if (res.status === 'confirmed') {
          onDone();
          return;
        }
        if (res.status === 'rejected' || res.status === 'expired') {
          setTgState(res.status);
          return;
        }
        poll(token, expires);
      } catch (err) {
        if (err instanceof ApiError && err.status === 429) {
          poll(token, expires);
          return;
        }
        setError(failText(err, 'Вход не подтвердился'));
        setTgState('idle');
      }
    }, 2500);
  };

  const startTelegram = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await api<Tg>('/api/client/auth/telegram', { method: 'POST', body: {} });
      setTg(res);
      setTgState('waiting');
      poll(res.poll_token, Date.parse(res.expires_at));
    } catch (err) {
      setError(err instanceof ApiError && err.code === 'bot.not_configured' ? 'Вход через Telegram пока не настроен. Попробуйте позже или войдите по почте.' : failText(err, 'Вход не начался'));
    } finally {
      setBusy(false);
    }
  };

  const sendCode = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api('/api/client/auth/email', { method: 'POST', body: { email: email.trim() } });
      setSent(true);
    } catch (err) {
      setError(err instanceof ApiError && err.status === 501 ? 'Вход по почте пока недоступен. Войдите через Telegram.' : failText(err, 'Код не отправился'));
    } finally {
      setBusy(false);
    }
  };

  const verify = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api('/api/client/auth/email/verify', { method: 'POST', body: { email: email.trim(), code: code.trim() } });
      onDone();
    } catch (err) {
      setError(failText(err, 'Код не подошёл'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="cb-solo">
      <div className="cb-solo-box cb-login">
        <div className="cb-brand" aria-hidden="true">
          <span className="cb-mark">S</span>
          <span className="cb-brand-name">{site.brand_name.toUpperCase()}</span>
        </div>
        <h1>Вход в кабинет</h1>
        <div role="group" aria-label="Способ входа" className="cb-seg">
          <button type="button" aria-pressed={mode === 'telegram'} onClick={() => setMode('telegram')}>
            Telegram
          </button>
          <button type="button" aria-pressed={mode === 'email'} onClick={() => setMode('email')}>
            Почта
          </button>
        </div>
        {mode === 'telegram' ? (
          tg && tgState === 'waiting' ? (
            <div className="cb-login-wait">
              <p>Откройте бота и подтвердите вход. Страница обновится сама.</p>
              <a className="cb-main" href={tg.link} target="_blank" rel="noreferrer">
                Открыть Telegram
              </a>
              <QrCode text={tg.link} size={180} label="QR-код для входа через Telegram" />
              <span className="cb-muted">На компьютере отсканируйте код телефоном</span>
            </div>
          ) : (
            <div className="cb-login-wait">
              {tgState === 'rejected' ? <p className="cb-warn">Вход отклонён в боте.</p> : null}
              {tgState === 'expired' ? <p className="cb-muted">Время на подтверждение вышло, начните заново.</p> : null}
              <p>Бот пришлёт запрос на вход — подтвердите его, и кабинет откроется.</p>
              <button type="button" className="cb-main" onClick={() => void startTelegram()} disabled={busy}>
                {busy ? 'Готовим вход…' : 'Войти через Telegram'}
              </button>
            </div>
          )
        ) : sent ? (
          <form className="cb-login-form" onSubmit={verify}>
            <label className="cb-field">
              <span>Код из письма</span>
              <input value={code} onChange={(e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))} inputMode="numeric" autoComplete="one-time-code" placeholder="6 цифр" />
            </label>
            <button type="submit" className="cb-main" disabled={busy || code.length !== 6}>
              Войти
            </button>
          </form>
        ) : (
          <form className="cb-login-form" onSubmit={sendCode}>
            <label className="cb-field">
              <span>Почта</span>
              <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} autoComplete="email" placeholder="you@example.com" />
            </label>
            <button type="submit" className="cb-main" disabled={busy || !email.includes('@')}>
              Прислать код
            </button>
          </form>
        )}
        {error ? (
          <p className="cb-error" role="alert">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  );
}
