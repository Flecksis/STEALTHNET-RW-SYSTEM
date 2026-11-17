import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { useSession } from '../session';
import { GROUPS, setupLink, setupText, type ModulesData, type ModuleView } from './model';
import '../form.css';
import './modules.css';

type Filter = 'all' | 'on' | 'off';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function Modules() {
  const { reload } = useSession();
  const [data, setData] = useState<ModulesData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>('all');
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api<ModulesData>('/api/admin/modules'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Модули не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const replace = (next: ModuleView) =>
    setData((current) => (current ? { ...current, modules: current.modules.map((m) => (m.info.id === next.info.id ? next : m)) } : current));

  const toggle = async (m: ModuleView) => {
    if (m.locked || busy) {
      return;
    }
    const enabled = !m.active;
    setBusy(m.info.id);
    setError(null);
    replace({ ...m, enabled, active: enabled, warning: enabled ? m.warning : null });
    try {
      replace(await api<ModuleView>(`/api/admin/modules/${m.info.id}`, { method: 'PATCH', body: { enabled } }));
      await reload();
    } catch (err) {
      replace(m);
      setError(failText(err, `Модуль «${m.info.title}» не переключился`));
    } finally {
      setBusy(null);
    }
  };

  const modules = data?.modules ?? [];
  const onCount = modules.filter((m) => m.active).length;
  const chips: Array<[Filter, string, number]> = [
    ['all', 'Все', modules.length],
    ['on', 'Включены', onCount],
    ['off', 'Выключены', modules.length - onCount],
  ];
  const fits = (m: ModuleView) => filter === 'all' || (filter === 'on') === m.active;

  return (
    <div className="md">
      <header className="md-head">
        <div className="md-title">
          <h1 className="page-title">Модули</h1>
          <p>Включайте только то, чем пользуетесь. Выключенный модуль пропадает из админки, бота, кабинета и мини-приложения, а его данные сохраняются.</p>
        </div>
        <div role="group" aria-label="Какие показать" className="md-chips">
          {chips.map(([key, label, n]) => (
            <button key={key} type="button" aria-pressed={filter === key} onClick={() => setFilter(key)}>
              <span>{label}</span>
              <span className="md-n">{n}</span>
            </button>
          ))}
        </div>
      </header>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {data === null && !error ? <p className="muted">Загружаем модули…</p> : null}
      {data ? (
        <section aria-labelledby="core-h" className="card md-core">
          <div className="md-core-head">
            <h2 id="core-h" className="section-title">
              Ядро
            </h2>
            <span className="muted">Работает всегда, выключить нельзя</span>
          </div>
          <ul>
            {data.core.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </section>
      ) : null}
      <div className="md-groups">
        {GROUPS.map(([group, name]) => {
          const all = modules.filter((m) => m.info.group === group);
          const shown = all.filter(fits);
          if (shown.length === 0) {
            return null;
          }
          const on = all.filter((m) => m.active).length;
          return (
            <section key={group} aria-labelledby={`grp-${group}`} className="card md-group">
              <div className="md-group-head">
                <h2 id={`grp-${group}`} className="section-title">
                  {name}
                </h2>
                <span className="muted">
                  {on} из {all.length} включено
                </span>
              </div>
              <ul>
                {shown.map((m) => (
                  <li key={m.info.id} className="md-row">
                    <div className="md-text">
                      <span className="md-name">{m.info.title}</span>
                      <span className="md-desc">{m.info.description}</span>
                      <span className="md-where">{m.info.where_used}</span>
                      {m.active && m.warning ? <span className="md-warn">{m.warning}</span> : null}
                      {m.locked ? <span className="md-note">{m.locked}</span> : null}
                    </div>
                    {m.active ? (
                      <Link to={setupLink(m)} className="md-link" aria-label={`${setupText(m)}: ${m.info.title}`}>
                        {setupText(m)}
                      </Link>
                    ) : null}
                    <button
                      type="button"
                      role="switch"
                      className="sw"
                      aria-checked={m.active}
                      aria-disabled={m.locked ? true : undefined}
                      aria-label={m.info.title}
                      onClick={() => void toggle(m)}
                      disabled={busy === m.info.id}
                    >
                      <span aria-hidden="true" />
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          );
        })}
      </div>
    </div>
  );
}
