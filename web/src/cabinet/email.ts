import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { t } from '../lib/i18n';

export type EmailProof = 'totp' | 'password';
export type EmailPending = { email: string; expires_at: string };
export type EmailState = {
  email: string | null;
  available: boolean;
  proof: EmailProof | null;
  confirm_current?: boolean;
  pending: EmailPending | null;
};
export type EmailDone = { email: string; previous: string | null };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function sixDigits(value: string): string {
  return value.replace(/\D/g, '').slice(0, 6);
}

export function doneText(done: EmailDone): string {
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
    try {
      const body = {
        email: address.trim(),
        password: info.proof === 'password' ? proof : undefined,
        code: info.proof === 'totp' || byCurrent ? proof : undefined,
      };
      setSent(await api<EmailPending>('/api/client/security/email/start', { method: 'POST', body }));
      setProof('');
      setCode('');
      setProofSent(null);
    } catch (err) {
      setError(failText(err, t('Код не отправился, попробуйте ещё раз')));
      if (info.proof === 'totp') {
        setProof('');
      }
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
      const previous = info?.email ?? null;
      const next = await api<EmailState>('/api/client/security/email/confirm', { method: 'POST', body: { email: sent.email, code } });
      setInfo(next);
      setSent(null);
      setCode('');
      setAddress('');
      setDone({ email: next.email ?? sent.email, previous });
      await onChanged();
    } catch (err) {
      setError(failText(err, t('Код не подошёл')));
    } finally {
      setBusy(false);
    }
  };

  const restart = () => {
    if (sent) {
      setAddress(sent.email);
    }
    setSent(null);
    setCode('');
    setError(null);
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
    done,
    ready,
    byCurrent,
    proofSent,
    sendProof,
    start,
    confirm,
    restart,
    reset,
  };
}
