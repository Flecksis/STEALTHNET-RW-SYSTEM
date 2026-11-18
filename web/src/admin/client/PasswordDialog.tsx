import { useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { Dialog } from './dialogs';
import './password.css';

export function PasswordDialog({ clientId, email, onClose, onIssued }: { clientId: number; email: string; onClose: () => void; onIssued: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [password, setPassword] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const issue = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await api<{ password: string }>(`/api/admin/clients/${clientId}/password`, { method: 'POST', body: {} });
      setPassword(res.password);
      onIssued();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Пароль не выдан');
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    if (!password) {
      return;
    }
    try {
      await navigator.clipboard.writeText(password);
      setCopied(true);
    } catch {
      setError('Браузер не дал скопировать: выделите пароль вручную');
    }
  };

  if (password) {
    return (
      <Dialog title="Новый пароль выдан" onClose={onClose}>
        <p className="muted">
          Передайте клиенту пароль для входа на сайт с почтой {email}. Пароль показан один раз и работает только для первого входа: сразу после него клиент задаст свой пароль.
        </p>
        <div className="pw-issued">
          <span className="mono">{password}</span>
          <button type="button" className="btn-line hov" onClick={() => void copy()}>
            {copied ? 'Скопировано' : 'Скопировать'}
          </button>
        </div>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <div className="dialog-actions">
          <button type="button" className="btn-main btn" onClick={onClose}>
            Готово
          </button>
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog title="Новый пароль для сайта" busy={busy} onClose={onClose}>
      <p className="muted">Клиент получит одноразовый пароль для входа на сайт с почтой {email}. Старый пароль перестанет работать, все входы в кабинет на сайте закроются. Мини-приложение в Telegram продолжит работать. Клиенту придёт сообщение в бот и на почту, что пароль выдан.</p>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-bad" onClick={() => void issue()} disabled={busy}>
          {busy ? 'Выдаём…' : 'Выдать новый пароль'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}
