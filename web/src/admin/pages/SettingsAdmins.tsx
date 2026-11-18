import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { dateTime } from '../../lib/format';
import type { AdminRole } from '../../lib/types';
import { ROLE_LABEL, useSession } from '../session';
import { Switch } from '../ui';
import '../form.css';
import './admins.css';
import { SettingsLayout } from './SettingsLayout';

type AdminItem = {
  id: number;
  email: string;
  role: AdminRole;
  two_factor: boolean;
  disabled: boolean;
  must_change_password: boolean;
  created_at: string;
  last_login_at: string | null;
  remna_sync: boolean;
};

type Issued = { admin: AdminItem; password: string };

type Ask = { id: number; kind: 'disable' | 'password' | 'totp' };

const ROLES: AdminRole[] = ['owner', 'admin', 'support', 'read_only'];

const ROLE_NOTE: Record<AdminRole, string> = {
  owner: 'всё, включая администраторов',
  admin: 'всё, кроме администраторов; синхронизацию с Remnawave владелец включает отдельно',
  support: 'клиенты и обращения, без выдачи пароля клиенту',
  read_only: 'только просмотр клиентов',
};

const ASK_TEXT: Record<Ask['kind'], { text: string; confirm: string }> = {
  disable: { text: 'Доступ закроется, все сессии этого администратора завершатся.', confirm: 'Отключить' },
  password: { text: 'Старый пароль перестанет работать, сессии завершатся. Новый временный пароль покажем один раз.', confirm: 'Выдать новый пароль' },
  totp: { text: 'Вход по коду выключится, сессии завершатся. Включить его снова можно в «Безопасности».', confirm: 'Сбросить 2FA' },
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function statusLine(item: AdminItem): { text: string; tone: 'bad' | 'warn' | 'muted' } {
  if (item.disabled) {
    return { text: 'Доступ отключён', tone: 'bad' };
  }
  if (item.must_change_password) {
    return { text: item.last_login_at ? 'Ждём смены временного пароля' : 'Входа ещё не было, пароль временный', tone: 'warn' };
  }
  return { text: item.last_login_at ? `Последний вход: ${dateTime(item.last_login_at)}` : 'Входа ещё не было', tone: 'muted' };
}

function CopyPassword({ issued, onDone }: { issued: Issued; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(issued.password);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };
  return (
    <div className="row adm-issued" role="status">
      <span className="rc">
        <span>
          Временный пароль для <b>{issued.admin.email}</b>
        </span>
        <span className="adm-pass mono">{issued.password}</span>
        <span className="cap">Передайте его лично. При первом входе админка попросит заменить его своим. Пароль показан один раз.</span>
      </span>
      <span className="adm-acts">
        <button type="button" className="b34 hov" onClick={() => void copy()}>
          {copied ? 'Скопировано' : 'Скопировать'}
        </button>
        <button type="button" className="b34 hov" onClick={onDone}>
          Готово
        </button>
      </span>
    </div>
  );
}

export function SettingsAdmins() {
  const { me } = useSession();
  const [items, setItems] = useState<AdminItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [busy, setBusy] = useState<number | 'new' | null>(null);
  const [rowError, setRowError] = useState<{ id: number | 'new'; text: string } | null>(null);
  const [issued, setIssued] = useState<Issued | null>(null);
  const [email, setEmail] = useState('');
  const [role, setRole] = useState<AdminRole>('support');

  const load = useCallback(async () => {
    try {
      setItems(await api<AdminItem[]>('/api/admin/admins'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Список не загрузился'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const put = (next: AdminItem) => setItems((list) => (list ? list.map((item) => (item.id === next.id ? next : item)) : list));

  const act = async (id: number, path: string, body?: unknown) => {
    setBusy(id);
    setRowError(null);
    try {
      const result = await api<AdminItem | Issued>(`/api/admin/admins/${id}/${path}`, { method: 'POST', body: body ?? {} });
      if ('password' in result) {
        put(result.admin);
        setIssued(result);
      } else {
        put(result);
      }
      setAsk(null);
    } catch (err) {
      setRowError({ id, text: failText(err, 'Не получилось') });
    } finally {
      setBusy(null);
    }
  };

  const create = async () => {
    if (!email.trim()) {
      return;
    }
    setBusy('new');
    setRowError(null);
    try {
      const result = await api<Issued>('/api/admin/admins', { method: 'POST', body: { email: email.trim(), role } });
      setItems((list) => (list ? [...list, result.admin] : [result.admin]));
      setIssued(result);
      setEmail('');
    } catch (err) {
      setRowError({ id: 'new', text: failText(err, 'Администратор не добавился') });
    } finally {
      setBusy(null);
    }
  };

  return (
    <SettingsLayout>
      <section className="sec" aria-labelledby="adm-h">
        <div className="sec-head">
          <h2 id="adm-h" className="section-title">
            Администраторы
          </h2>
          <span className="muted">Кто входит в админку и что может делать. Свой пароль и вход по коду меняются в «Безопасности».</span>
        </div>
        {error ? (
          <div className="row">
            <span className="err" role="alert">
              {error}
            </span>
          </div>
        ) : null}
        {items === null && !error ? (
          <div className="row">
            <span className="cap">Загружаем…</span>
          </div>
        ) : null}
        {(items ?? []).map((item) => {
          const self = item.id === me.admin.admin_id;
          const status = statusLine(item);
          const open = ask?.id === item.id ? ask : null;
          return (
            <div key={item.id} className="adm-item">
              <div className="row adm-row">
                <span className="adm-who">
                  <span className="adm-email" title={item.email}>
                    {item.email}
                    {self ? <span className="muted"> · вы</span> : null}
                  </span>
                  <span className={`adm-status ${status.tone}`}>
                    {status.text}
                    {item.two_factor ? <span className="muted"> · вход по коду</span> : null}
                  </span>
                </span>
                <label className="adm-role">
                  <span className="sr-only">Роль {item.email}</span>
                  <select
                    className="field"
                    value={item.role}
                    disabled={self || busy !== null}
                    onChange={(e) => void act(item.id, 'role', { role: e.target.value })}
                  >
                    {ROLES.map((r) => (
                      <option key={r} value={r}>
                        {ROLE_LABEL[r]}
                      </option>
                    ))}
                  </select>
                </label>
                <span className="adm-acts">
                  {self ? (
                    <Link to="/admin/security" className="b34 hov">
                      Безопасность
                    </Link>
                  ) : item.disabled ? (
                    <button type="button" className="b34 hov" disabled={busy !== null} onClick={() => void act(item.id, 'enable')}>
                      Включить доступ
                    </button>
                  ) : (
                    <>
                      <button
                        type="button"
                        className="b34 hov"
                        aria-expanded={open?.kind === 'password'}
                        disabled={busy !== null}
                        onClick={() => setAsk(open?.kind === 'password' ? null : { id: item.id, kind: 'password' })}
                      >
                        Новый пароль
                      </button>
                      {item.two_factor ? (
                        <button
                          type="button"
                          className="b34 hov"
                          aria-expanded={open?.kind === 'totp'}
                          disabled={busy !== null}
                          onClick={() => setAsk(open?.kind === 'totp' ? null : { id: item.id, kind: 'totp' })}
                        >
                          Сбросить 2FA
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="b34 bad hov"
                        aria-expanded={open?.kind === 'disable'}
                        disabled={busy !== null}
                        onClick={() => setAsk(open?.kind === 'disable' ? null : { id: item.id, kind: 'disable' })}
                      >
                        Отключить
                      </button>
                    </>
                  )}
                </span>
              </div>
              {item.role === 'admin' && !self && !item.disabled ? (
                <div className="row adm-grant">
                  <span className="cap">Синхронизация с Remnawave: перенос клиентов и подписок в Remnawave и обратно</span>
                  <Switch
                    on={item.remna_sync}
                    onToggle={() => void act(item.id, 'remna-sync', { on: !item.remna_sync })}
                    label={`Синхронизация с Remnawave для ${item.email}`}
                    disabled={busy !== null}
                  />
                </div>
              ) : null}
              {open ? (
                <div className="row adm-ask">
                  <span className="cap">{ASK_TEXT[open.kind].text}</span>
                  <span className="adm-acts">
                    <button
                      type="button"
                      className={open.kind === 'disable' ? 'btn-bad' : 'btn-main btn'}
                      disabled={busy !== null}
                      onClick={() => void act(item.id, open.kind === 'disable' ? 'disable' : open.kind === 'password' ? 'password' : '2fa/reset')}
                    >
                      {busy === item.id ? 'Выполняем…' : ASK_TEXT[open.kind].confirm}
                    </button>
                    <button type="button" className="btn-line hov" disabled={busy !== null} onClick={() => setAsk(null)}>
                      Отменить
                    </button>
                  </span>
                </div>
              ) : null}
              {rowError?.id === item.id ? (
                <div className="row">
                  <span className="err" role="alert">
                    {rowError.text}
                  </span>
                </div>
              ) : null}
              {issued?.admin.id === item.id ? <CopyPassword issued={issued} onDone={() => setIssued(null)} /> : null}
            </div>
          );
        })}
        <form
          className="row adm-new"
          onSubmit={(e) => {
            e.preventDefault();
            void create();
          }}
        >
          <span className="rc">
            <span className="adm-new-fields">
              <input
                className="field"
                type="email"
                placeholder="Почта нового администратора"
                aria-label="Почта нового администратора"
                value={email}
                maxLength={254}
                onChange={(e) => setEmail(e.target.value)}
                disabled={busy !== null}
              />
              <select className="field adm-new-role" aria-label="Роль нового администратора" value={role} onChange={(e) => setRole(e.target.value as AdminRole)} disabled={busy !== null}>
                {ROLES.map((r) => (
                  <option key={r} value={r}>
                    {ROLE_LABEL[r]}
                  </option>
                ))}
              </select>
              <button type="submit" className="btn-main btn" disabled={busy !== null || !email.trim()}>
                {busy === 'new' ? 'Добавляем…' : 'Добавить'}
              </button>
            </span>
            <span className="cap">
              {ROLE_LABEL[role]}: {ROLE_NOTE[role]}. Пароль придумает система и покажет один раз.
            </span>
            {rowError?.id === 'new' ? (
              <span className="err" role="alert">
                {rowError.text}
              </span>
            ) : null}
          </span>
        </form>
      </section>
    </SettingsLayout>
  );
}
