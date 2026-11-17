import { useState, type FormEvent } from 'react';
import { api, ApiError } from '../lib/api';
import './login.css';

type Props = {
  brand: string;
  onDone: () => void;
};

export function Login({ brand, onDone }: Props) {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [code, setCode] = useState('');
  const [needCode, setNeedCode] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api('/api/admin/auth/login', {
        method: 'POST',
        body: { email: email.trim(), password, totp: needCode ? code.trim() : undefined },
      });
      onDone();
    } catch (err) {
      if (err instanceof ApiError && err.code === 'auth.totp_required') {
        setNeedCode(true);
        setError(null);
      } else if (err instanceof ApiError && err.code === 'auth.required') {
        setError('Почта или пароль не подошли');
      } else if (err instanceof ApiError && err.code === 'auth.totp_invalid') {
        setError('Код не подошёл. Введите новый код из приложения');
        setCode('');
      } else if (err instanceof ApiError) {
        setError(err.message);
      } else {
        setError('Не получилось войти. Попробуйте ещё раз');
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="login">
      <div className="login-box">
        <div className="login-brand">
          <span className="mark" aria-hidden="true">
            S
          </span>
          <span>{brand.toUpperCase()}</span>
        </div>
        <form className="card login-card" onSubmit={submit} noValidate>
          <h1>Вход в админку</h1>
          <label className="login-field">
            <span>Почта</span>
            <input
              className="login-input"
              type="email"
              autoComplete="username"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              required
              autoFocus
            />
          </label>
          <label className="login-field">
            <span>Пароль</span>
            <input
              className="login-input"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
            />
          </label>
          {needCode ? (
            <label className="login-field">
              <span>Код из приложения</span>
              <input
                className="login-input"
                inputMode="numeric"
                autoComplete="one-time-code"
                value={code}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
                autoFocus
              />
            </label>
          ) : null}
          {error ? (
            <p className="login-error" role="alert">
              {error}
            </p>
          ) : null}
          <button className="btn-main btn" type="submit" disabled={busy || !email || !password || (needCode && code.length < 6)}>
            {busy ? 'Входим…' : 'Войти'}
          </button>
        </form>
      </div>
    </div>
  );
}
