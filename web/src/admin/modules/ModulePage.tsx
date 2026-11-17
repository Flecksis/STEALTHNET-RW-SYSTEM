import { useCallback, useEffect, useState } from 'react';
import { Link, useParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { useSession } from '../session';
import { Switch } from '../ui';
import type { ModuleConfig, ModulesData, ModuleView, SettingField } from './model';
import '../form.css';
import './modules.css';

type Values = Record<string, string | boolean>;

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function initial(field: SettingField, config: Record<string, unknown>): string | boolean {
  const stored = config[field.key];
  if (field.kind.type === 'secret') {
    return '';
  }
  if (field.kind.type === 'flag') {
    return typeof stored === 'boolean' ? stored : field.default === 'true';
  }
  if (stored === null || stored === undefined) {
    return field.default ?? '';
  }
  return String(stored);
}

function secretFilled(config: Record<string, unknown>, key: string): boolean {
  const value = config[key];
  return typeof value === 'object' && value !== null && (value as { filled?: boolean }).filled === true;
}

export function ModulePage() {
  const { id = '' } = useParams();
  const { reload } = useSession();
  const [view, setView] = useState<ModuleView | null>(null);
  const [cfg, setCfg] = useState<ModuleConfig | null>(null);
  const [values, setValues] = useState<Values>({});
  const [clear, setClear] = useState<string[]>([]);
  const [replacing, setReplacing] = useState<string[]>([]);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const apply = (next: ModuleConfig) => {
    setCfg(next);
    setValues(Object.fromEntries(next.schema.map((f) => [f.key, initial(f, next.config)])));
    setClear([]);
    setReplacing([]);
  };

  const load = useCallback(async () => {
    try {
      const [list, config] = await Promise.all([api<ModulesData>('/api/admin/modules'), api<ModuleConfig>(`/api/admin/modules/${id}/config`)]);
      const found = list.modules.find((m) => m.info.id === id) ?? null;
      setMissing(found === null);
      setView(found);
      apply(config);
      setError(null);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        setMissing(true);
        return;
      }
      setError(failText(err, 'Модуль не загрузился'));
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  const toggle = async () => {
    if (!view || view.locked || busy) {
      return;
    }
    setBusy(true);
    setNote(null);
    try {
      setView(await api<ModuleView>(`/api/admin/modules/${id}`, { method: 'PATCH', body: { enabled: !view.active } }));
      await reload();
    } catch (err) {
      setError(failText(err, 'Модуль не переключился'));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!cfg) {
      return;
    }
    const config: Record<string, unknown> = {};
    for (const field of cfg.schema) {
      const value = values[field.key];
      const before = initial(field, cfg.config);
      if (field.kind.type === 'secret') {
        if (typeof value === 'string' && value.trim()) {
          config[field.key] = value.trim();
        }
        continue;
      }
      if (value === before) {
        continue;
      }
      config[field.key] = field.kind.type === 'number' && value === '' ? null : value;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      apply(await api<ModuleConfig>(`/api/admin/modules/${id}/config`, { method: 'PUT', body: { config, clear } }));
      const list = await api<ModulesData>('/api/admin/modules');
      setView(list.modules.find((m) => m.info.id === id) ?? view);
      setNote('Настройки сохранены');
    } catch (err) {
      setError(failText(err, 'Настройки не сохранились'));
    } finally {
      setBusy(false);
    }
  };

  if (missing) {
    return (
      <div className="md-page">
        <Link to="/admin/modules" className="md-back">
          Все модули
        </Link>
        <h1 className="page-title tight">Модуль не найден</h1>
        <p className="muted">Такого модуля нет в этой установке.</p>
      </div>
    );
  }

  if (!view || !cfg) {
    return (
      <div className="md-page">
        <Link to="/admin/modules" className="md-back">
          Все модули
        </Link>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем модуль…</p>
        )}
      </div>
    );
  }

  const set = (key: string, value: string | boolean) => setValues({ ...values, [key]: value });

  return (
    <div className="md-page">
      <Link to="/admin/modules" className="md-back">
        Все модули
      </Link>
      <div className="md-title">
        <h1 className="page-title tight">{view.info.title}</h1>
        <p>{view.info.description}</p>
      </div>
      <section className="card md-state" aria-label="Состояние модуля">
        <div className="md-text">
          <span className="md-name">{view.active ? 'Модуль включён' : 'Модуль выключен'}</span>
          <span className="md-where">Где работает: {view.info.where_used}</span>
          {view.active && view.warning ? <span className="md-warn">{view.warning}</span> : null}
          {view.locked ? <span className="md-note">{view.locked}</span> : null}
        </div>
        <Switch on={view.active} onToggle={() => void toggle()} label={view.info.title} disabled={busy || view.locked !== null} />
      </section>
      {view.info.menu && view.active ? (
        <Link to={`/admin/m/${view.info.id}`} className="md-back">
          Открыть раздел «{view.info.menu.label}»
        </Link>
      ) : null}
      {cfg.schema.length === 0 ? (
        <section className="card card-pad">
          <p className="muted">Своих настроек у модуля нет: он работает сам там, где указано выше.</p>
        </section>
      ) : (
        <section className="card md-form" aria-labelledby="md-form-h">
          <h2 id="md-form-h" className="section-title">
            Настройки
          </h2>
          {cfg.schema.map((field) => {
            const value = values[field.key];
            const hintId = field.hint ? `hint-${field.key}` : undefined;
            if (field.kind.type === 'flag') {
              return (
                <div key={field.key} className="md-flag">
                  <span className="md-text">
                    <span className="md-name">{field.label}</span>
                    {field.hint ? (
                      <span id={hintId} className="cap">
                        {field.hint}
                      </span>
                    ) : null}
                  </span>
                  <Switch on={value === true} onToggle={() => set(field.key, value !== true)} label={field.label} describedBy={hintId} disabled={busy} />
                </div>
              );
            }
            if (field.kind.type === 'secret') {
              const filled = secretFilled(cfg.config, field.key) && !clear.includes(field.key);
              const editing = !filled || replacing.includes(field.key);
              return (
                <div key={field.key} className="md-field">
                  <span>{field.label}</span>
                  {editing ? (
                    <input
                      className="field mono"
                      type="password"
                      autoComplete="off"
                      value={typeof value === 'string' ? value : ''}
                      onChange={(e) => set(field.key, e.target.value)}
                      aria-label={field.label}
                      aria-describedby={hintId}
                      placeholder={filled ? 'Новое значение' : 'Вставьте значение'}
                      disabled={busy}
                    />
                  ) : (
                    <span className="md-secret">
                      <span className="muted">Сохранено</span>
                      <button type="button" className="b34 hov" onClick={() => setReplacing([...replacing, field.key])} disabled={busy}>
                        Заменить
                      </button>
                      {!field.required ? (
                        <button type="button" className="b34 hov" onClick={() => setClear([...clear, field.key])} disabled={busy}>
                          Удалить
                        </button>
                      ) : null}
                    </span>
                  )}
                  {field.hint ? (
                    <span id={hintId} className="cap">
                      {field.hint}
                    </span>
                  ) : null}
                </div>
              );
            }
            return (
              <label key={field.key} className="md-field">
                <span>{field.label}</span>
                {field.kind.type === 'select' ? (
                  <select className="field" value={String(value ?? '')} onChange={(e) => set(field.key, e.target.value)} aria-describedby={hintId} disabled={busy}>
                    {field.kind.options.map((option) => (
                      <option key={option} value={option}>
                        {option}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    className="field"
                    type={field.kind.type === 'url' ? 'url' : 'text'}
                    inputMode={field.kind.type === 'number' ? 'decimal' : undefined}
                    value={String(value ?? '')}
                    onChange={(e) => set(field.key, e.target.value)}
                    aria-describedby={hintId}
                    disabled={busy}
                  />
                )}
                {field.hint ? (
                  <span id={hintId} className="cap">
                    {field.hint}
                  </span>
                ) : null}
              </label>
            );
          })}
          {error ? (
            <p className="err" role="alert">
              {error}
            </p>
          ) : null}
          <div className="md-actions">
            <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
              {busy ? 'Сохраняем…' : 'Сохранить'}
            </button>
            {note ? (
              <span role="status" className="muted">
                {note}
              </span>
            ) : null}
          </div>
        </section>
      )}
      {cfg.schema.length === 0 && error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
