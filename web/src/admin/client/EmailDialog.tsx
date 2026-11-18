import { useId, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { Dialog } from './dialogs';
import type { Identity } from './types';
import './email.css';

type Saved = { email: string; previous: string | null };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function EmailRow({ email, editable, onEdit }: { email: Identity | undefined; editable: boolean; onEdit: () => void }) {
  return (
    <div className="cc-prof">
      <span className="muted">Email</span>
      <span className="em-row">
        <span>{email ? email.value : 'нет'}</span>
        {email && !email.verified ? <span className="em-note">не подтверждена</span> : null}
        {editable ? (
          <button type="button" className="em-edit" onClick={onEdit}>
            {email ? 'Изменить' : 'Указать'}
          </button>
        ) : null}
      </span>
    </div>
  );
}

type Props = { clientId: number; current: Identity | undefined; onClose: () => void; onDone: (text: string) => void };

export function EmailDialog({ clientId, current, onClose, onDone }: Props) {
  const email = current?.value ?? null;
  const unconfirmed = !!current && !current.verified;
  const [removing, setRemoving] = useState(false);
  const [value, setValue] = useState(email ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [taken, setTaken] = useState<number | null>(null);
  const fieldId = useId();
  const address = value.trim();
  const ready = address.includes('@') && (unconfirmed || address.toLowerCase() !== (email ?? '').toLowerCase());

  const save = async () => {
    if (!ready || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    setTaken(null);
    try {
      const saved = await api<Saved>(`/api/admin/clients/${clientId}/email`, { method: 'PUT', body: { email: address } });
      onDone(saved.previous ? `Почта изменена на ${saved.email}` : `Почта ${saved.email} указана и подтверждена`);
    } catch (err) {
      const owner = err instanceof ApiError && err.code === 'client.email_taken' ? err.details.client_id : null;
      if (typeof owner === 'number') {
        setTaken(owner);
      } else {
        setError(failText(err, 'Почта не сохранилась'));
      }
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await api(`/api/admin/clients/${clientId}/email`, { method: 'DELETE' });
      onDone('Почта отвязана');
    } catch (err) {
      setError(failText(err, 'Почта не отвязалась'));
      setBusy(false);
    }
  };

  const errorLine = error ? (
    <p className="err" role="alert">
      {error}
    </p>
  ) : null;

  if (removing && email) {
    return (
      <Dialog title="Отвязать почту" busy={busy} onClose={onClose}>
        <p className="muted">
          Почта {email} и пароль для сайта отвяжутся от клиента, войти по почте будет нельзя. Входы в кабинет на сайте закроются, мини-приложение в Telegram продолжит работать. Клиенту придёт сообщение в бот.
        </p>
        {errorLine}
        <div className="dialog-actions">
          <button type="button" className="btn-bad" onClick={() => void remove()} disabled={busy}>
            {busy ? 'Отвязываем…' : 'Отвязать почту'}
          </button>
          <button
            type="button"
            className="btn-line hov"
            onClick={() => {
              setRemoving(false);
              setError(null);
            }}
            disabled={busy}
          >
            Назад
          </button>
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog title={email ? 'Почта клиента' : 'Указать почту'} busy={busy} onClose={onClose}>
      <p className="muted">
        {email
          ? `Новая почта заменит ${email}. Кода подтверждения не будет: панель считает почту подтверждённой, поэтому проверьте адрес. Пароль для сайта останется прежним, входы в кабинет на сайте закроются. Клиенту придёт сообщение в бот, на прежний адрес — письмо.`
          : 'Клиент сможет входить по этой почте на сайт и восстанавливать пароль кодом из письма. Кода подтверждения не будет: панель считает почту подтверждённой, поэтому проверьте адрес. Входы в кабинет на сайте закроются, клиенту придёт сообщение в бот.'}
        {unconfirmed ? ' Эта почта не подтверждена кодом: сохраните тот же адрес, чтобы панель считала её подтверждённой.' : ''}
      </p>
      <form
        className="field-row"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <label htmlFor={fieldId}>Почта</label>
        <input
          id={fieldId}
          className="field"
          type="email"
          inputMode="email"
          autoComplete="off"
          maxLength={254}
          value={value}
          onChange={(event) => {
            setValue(event.target.value);
            setTaken(null);
          }}
          aria-invalid={taken !== null}
          disabled={busy}
          autoFocus
        />
      </form>
      {taken !== null ? (
        <p className="err" role="alert">
          Эта почта уже указана у другого клиента.{' '}
          <Link to={`/admin/clients/${taken}`} className="em-link" onClick={onClose}>
            Открыть карточку
          </Link>
        </p>
      ) : null}
      {errorLine}
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || !ready}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
      {email ? (
        <button
          type="button"
          className="em-remove"
          onClick={() => {
            setRemoving(true);
            setError(null);
          }}
          disabled={busy}
        >
          Отвязать почту
        </button>
      ) : null}
    </Dialog>
  );
}
