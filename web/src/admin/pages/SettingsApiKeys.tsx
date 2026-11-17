import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { dateLong, dateTime } from '../../lib/format';
import type { Permission } from '../../lib/types';
import { useSession } from '../session';
import '../form.css';
import './admins.css';
import { SettingsLayout } from './SettingsLayout';

type ApiKey = {
  id: number;
  name: string;
  prefix: string;
  permissions: Permission[];
  created_by: string;
  created_at: string;
  expires_at: string | null;
  last_used_at: string | null;
  revoked: boolean;
  expired: boolean;
};

type Issued = { key: ApiKey; token: string };

const GRANTS: Array<[Permission, string]> = [
  ['clients_read', 'Клиенты: просмотр'],
  ['clients_write', 'Клиенты: изменения'],
  ['support', 'Поддержка'],
  ['money', 'Деньги'],
  ['refunds', 'Возвраты'],
  ['tariffs', 'Тарифы'],
  ['infrastructure', 'Инфраструктура'],
  ['marketing', 'Маркетинг'],
  ['modules', 'Модули'],
  ['settings', 'Настройки'],
];

const LABEL = Object.fromEntries(GRANTS) as Record<Permission, string>;

const TERMS: Array<[string, number | null]> = [
  ['Без срока', null],
  ['30 дней', 30],
  ['90 дней', 90],
  ['Год', 365],
];

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function stateLine(key: ApiKey): { text: string; tone: 'bad' | 'muted' } {
  if (key.revoked) {
    return { text: 'Отозван', tone: 'bad' };
  }
  if (key.expired) {
    return { text: `Срок истёк ${dateLong(key.expires_at)}`, tone: 'bad' };
  }
  const used = key.last_used_at ? `Использован: ${dateTime(key.last_used_at)}` : 'Ещё не использовался';
  const until = key.expires_at ? ` · действует до ${dateLong(key.expires_at)}` : '';
  return { text: used + until, tone: 'muted' };
}

function NewToken({ issued, onDone }: { issued: Issued; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(issued.token);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };
  return (
    <div className="row adm-issued" role="status">
      <span className="rc">
        <span>
          Ключ «<b>{issued.key.name}</b>»
        </span>
        <span className="adm-pass mono">{issued.token}</span>
        <span className="cap">
          Сохраните его сейчас: больше ключ не покажем. Передавайте в заголовке <span className="mono">Authorization: Bearer …</span>
        </span>
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

export function SettingsApiKeys() {
  const { can } = useSession();
  const [items, setItems] = useState<ApiKey[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [asking, setAsking] = useState<number | null>(null);
  const [busy, setBusy] = useState<number | 'new' | null>(null);
  const [rowError, setRowError] = useState<{ id: number | 'new'; text: string } | null>(null);
  const [issued, setIssued] = useState<Issued | null>(null);
  const [name, setName] = useState('');
  const [grants, setGrants] = useState<Permission[]>(['clients_read']);
  const [term, setTerm] = useState<number | null>(90);

  const load = useCallback(async () => {
    try {
      setItems(await api<ApiKey[]>('/api/admin/api-keys'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Список не загрузился'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const toggle = (permission: Permission) =>
    setGrants((list) => (list.includes(permission) ? list.filter((p) => p !== permission) : [...list, permission]));

  const create = async () => {
    setBusy('new');
    setRowError(null);
    try {
      const result = await api<Issued>('/api/admin/api-keys', {
        method: 'POST',
        body: { name: name.trim(), permissions: grants, expires_in_days: term },
      });
      setItems((list) => [result.key, ...(list ?? [])]);
      setIssued(result);
      setName('');
    } catch (err) {
      setRowError({ id: 'new', text: failText(err, 'Ключ не создался') });
    } finally {
      setBusy(null);
    }
  };

  const revoke = async (id: number) => {
    setBusy(id);
    setRowError(null);
    try {
      const next = await api<ApiKey>(`/api/admin/api-keys/${id}/revoke`, { method: 'POST', body: {} });
      setItems((list) => (list ? list.map((k) => (k.id === id ? next : k)) : list));
      setAsking(null);
      if (issued?.key.id === id) {
        setIssued(null);
      }
    } catch (err) {
      setRowError({ id, text: failText(err, 'Ключ не отозвался') });
    } finally {
      setBusy(null);
    }
  };

  const available = GRANTS.filter(([permission]) => can(permission));

  return (
    <SettingsLayout>
      <section className="sec" aria-labelledby="keys-h">
        <div className="sec-head">
          <h2 id="keys-h" className="section-title">
            API-ключи
          </h2>
          <span className="muted">
            Для своих скриптов и интеграций: ключ работает как вход в админку с выбранными правами. Права ключа не шире ваших, а управлять
            администраторами и ключами он не может.
          </span>
        </div>
        {error ? (
          <div className="row">
            <span className="err" role="alert">
              {error}
            </span>
          </div>
        ) : null}
        {issued && !(items ?? []).some((k) => k.id === issued.key.id) ? <NewToken issued={issued} onDone={() => setIssued(null)} /> : null}
        {(items ?? []).length === 0 && items !== null ? (
          <div className="row">
            <span className="cap">Ключей пока нет.</span>
          </div>
        ) : null}
        {(items ?? []).map((key) => {
          const line = stateLine(key);
          const dead = key.revoked || key.expired;
          return (
            <div key={key.id} className="adm-item">
              <div className={dead ? 'row adm-row off-row' : 'row adm-row'}>
                <span className="adm-who">
                  <span className="adm-email">
                    {key.name} <span className="mono muted">{key.prefix}…</span>
                  </span>
                  <span className="adm-status">{key.permissions.map((p) => LABEL[p] ?? p).join(', ')}</span>
                  <span className={`adm-status ${line.tone}`}>
                    {line.text} · автор {key.created_by}
                  </span>
                </span>
                {dead ? null : (
                  <span className="adm-acts">
                    <button
                      type="button"
                      className="b34 bad hov"
                      aria-expanded={asking === key.id}
                      disabled={busy !== null}
                      onClick={() => setAsking(asking === key.id ? null : key.id)}
                    >
                      Отозвать
                    </button>
                  </span>
                )}
              </div>
              {asking === key.id ? (
                <div className="row adm-ask">
                  <span className="cap">Запросы с этим ключом сразу перестанут проходить. Вернуть ключ нельзя, только выпустить новый.</span>
                  <span className="adm-acts">
                    <button type="button" className="btn-bad" disabled={busy !== null} onClick={() => void revoke(key.id)}>
                      {busy === key.id ? 'Отзываем…' : 'Отозвать ключ'}
                    </button>
                    <button type="button" className="btn-line hov" disabled={busy !== null} onClick={() => setAsking(null)}>
                      Отменить
                    </button>
                  </span>
                </div>
              ) : null}
              {rowError?.id === key.id ? (
                <div className="row">
                  <span className="err" role="alert">
                    {rowError.text}
                  </span>
                </div>
              ) : null}
              {issued?.key.id === key.id ? <NewToken issued={issued} onDone={() => setIssued(null)} /> : null}
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
                placeholder="Название, например «Выгрузка в CRM»"
                aria-label="Название ключа"
                value={name}
                maxLength={80}
                onChange={(e) => setName(e.target.value)}
                disabled={busy !== null}
              />
              <select
                className="field adm-new-role"
                aria-label="Срок действия"
                value={term === null ? '' : String(term)}
                onChange={(e) => setTerm(e.target.value ? Number(e.target.value) : null)}
                disabled={busy !== null}
              >
                {TERMS.map(([label, days]) => (
                  <option key={label} value={days === null ? '' : String(days)}>
                    {label}
                  </option>
                ))}
              </select>
              <button type="submit" className="btn-main btn" disabled={busy !== null || !name.trim() || grants.length === 0}>
                {busy === 'new' ? 'Создаём…' : 'Создать ключ'}
              </button>
            </span>
            <span className="key-grants" role="group" aria-label="Права ключа">
              {available.map(([permission, label]) => (
                <label key={permission} className="key-grant">
                  <input type="checkbox" checked={grants.includes(permission)} onChange={() => toggle(permission)} disabled={busy !== null} />
                  <span>{label}</span>
                </label>
              ))}
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
