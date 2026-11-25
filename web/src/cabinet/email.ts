import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { t } from '../lib/i18n';
import type { Site } from '../lib/types';

export type EmailProof = 'totp' | 'password';
export type EmailPending = { email: string; expires_at: string };
export type EmailState = {
  email: string | null;
  available: boolean;
  without_code?: boolean;
  unconfirmed?: boolean;
  proof: EmailProof | null;
  confirm_current?: boolean;
  pending: EmailPending | null;
};
export type EmailDone = { email: string; previous: string | null; how: 'code' | 'no_code' | 'confirmed' };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function sixDigits(value: string): string {
  return value.replace(/\D/g, '').slice(0, 6);
}

export function unconfirmedText(site: Site): string {
  return site.email_auth === 'code'
    ? t('Подтвердите её кодом из письма, чтобы входить по ней на сайт.')
    : t('Подтвердите её кодом из письма, чтобы восстанавливать по ней пароль.');
}

export function aboutEmail(info: EmailState, site: Site): string | null {
  const siteLogin = site.client_access?.site_login !== false;
  if (info.available && !info.unconfirmed) {
    return siteLogin ? t('На эту почту приходят письма, по ней же можно входить на сайт.') : t('На эту почту приходят письма.');
  }
  return info.email && siteLogin && site.email_auth === 'password' ? t('По этой почте и паролю можно входить на сайт.') : null;
}

export function closedText(info: EmailState): string {
  return info.email ? t('Письма сейчас не отправляются, поэтому сменить почту нельзя.') : t('Письма сейчас не отправляются, поэтому добавить почту нельзя.');
}

export function withoutCodeText(): string {
  return t('Письма сейчас не отправляются, поэтому адрес сохранится без проверки. Проверьте его: по этой почте и паролю вы будете входить на сайт.');
}

export function doneText(done: EmailDone): string {
  if (done.how === 'confirmed') {
    return t('Почта подтверждена: {email}.', { email: done.email });
  }
  if (done.how === 'no_code') {
    return t('Почта сохранена: {email}.', { email: done.email });
  }
  return done.previous
    ? t('Почта изменена: {email}. На прежний адрес отправлено уведомление.', { email: done.email })
    : t('Почта добавлена: {email}.', { email: done.email });
}

export function useEmailChange(onChanged: () => Promise<void> | void) {
  const [info, setInfo] = useState<EmailState | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [address, setAddress] = useState('');
  const [proof, setProof] = useState('');
  const [code, setCode] = useState('');
  const [sent, setSent] = useState<EmailPending | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmError, setConfirmError] = useState<string | null>(null);
  const [done, setDone] = useState<EmailDone | null>(null);
  const [proofSent, setProofSent] = useState<EmailPending | null>(null);

  const load = useCallback(async () => {
    try {
      const next = await api<EmailState>('/api/client/security/email');
      setInfo(next);
      setSent(next.pending);
      setFailed(null);
    } catch (err) {
      setFailed(failText(err, t('Почта не загрузилась')));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const byCurrent = !!info?.confirm_current && info.proof === null;
  const ready =
    address.trim().length > 3 &&
    (info?.proof === 'totp' || byCurrent ? proof.length === 6 : info?.proof === 'password' ? proof.length > 0 : true);

  const sendProof = async () => {
    if (!info || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setProofSent(await api<EmailPending>('/api/client/security/email/proof', { method: 'POST' }));
      setProof('');
    } catch (err) {
      setError(failText(err, t('Код не отправился, попробуйте ещё раз')));
    } finally {
      setBusy(false);
    }
  };

  const start = async () => {
    if (!info || busy || !ready) {
      return;
    }
    setBusy(true);
    setError(null);
    setDone(null);
    const body = {
      email: address.trim(),
      password: info.proof === 'password' ? proof : undefined,
      code: info.proof === 'totp' || byCurrent ? proof : undefined,
    };
    try {
      if (info.without_code) {
        const next = await api<EmailState>('/api/client/security/email/link', { method: 'POST', body });
        setInfo(next);
        setAddress('');
        setDone({ email: next.email ?? body.email, previous: null, how: 'no_code' });
        await onChanged();
      } else {
        setSent(await api<EmailPending>('/api/client/security/email/start', { method: 'POST', body }));
        setCode('');
        setProofSent(null);
      }
      setProof('');
    } catch (err) {
      setError(failText(err, info.without_code ? t('Почта не сохранилась') : t('Код не отправился, попробуйте ещё раз')));
      if (info.proof === 'totp') {
        setProof('');
      }
    } finally {
      setBusy(false);
    }
  };

  const confirmCurrent = async () => {
    if (!info?.email || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    setConfirmError(null);
    setDone(null);
    try {
      setSent(await api<EmailPending>('/api/client/security/email/start', { method: 'POST', body: { email: info.email } }));
      setCode('');
    } catch (err) {
      setConfirmError(failText(err, t('Код не отправился, попробуйте ещё раз')));
    } finally {
      setBusy(false);
    }
  };

  const confirm = async () => {
    if (!sent || busy || code.length !== 6) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const before = info;
      const next = await api<EmailState>('/api/client/security/email/confirm', { method: 'POST', body: { email: sent.email, code } });
      setInfo(next);
      setSent(null);
      setCode('');
      setAddress('');
      const email = next.email ?? sent.email;
      const previous = before?.unconfirmed ? null : (before?.email ?? null);
      setDone({ email, previous, how: before?.unconfirmed && before.email === email ? 'confirmed' : 'code' });
      await onChanged();
    } catch (err) {
      setError(failText(err, t('Код не подошёл')));
    } finally {
      setBusy(false);
    }
  };

  const restart = () => {
    if (sent && sent.email !== info?.email) {
      setAddress(sent.email);
    }
    setSent(null);
    setCode('');
    setError(null);
    setConfirmError(null);
  };

  const reset = () => {
    setAddress('');
    setProof('');
    setCode('');
    setError(null);
    setDone(null);
    setProofSent(null);
  };

  return {
    info,
    failed,
    address,
    setAddress,
    proof,
    setProof,
    code,
    setCode,
    sent,
    busy,
    error,
    confirmError,
    done,
    ready,
    byCurrent,
    proofSent,
    sendProof,
    start,
    confirmCurrent,
    confirm,
    restart,
    reset,
  };
}
