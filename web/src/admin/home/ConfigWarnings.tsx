import { useEffect, useState } from 'react';
import { api } from '../../lib/api';
import { useSession } from '../session';
import { warningText, type ConfigWarning } from './warnings';

const REFRESH_MS = 60_000;

export function ConfigWarnings({ compact = false }: { compact?: boolean }) {
  const { can } = useSession();
  const allowed = can('settings');
  const [warnings, setWarnings] = useState<ConfigWarning[]>([]);
  useEffect(() => {
    if (!allowed) {
      return;
    }
    let alive = true;
    const load = () => {
      api<{ warnings: ConfigWarning[] }>('/api/admin/system/warnings')
        .then((view) => {
          if (alive) {
            setWarnings(view.warnings);
          }
        })
        .catch(() => undefined);
    };
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [allowed]);
  const shown = warnings.map((warning) => ({ kind: warning.kind, view: warningText(warning) }));
  return (
    <>
      {shown.map(({ kind, view }) =>
        view ? (
          <section key={kind} className={compact ? 'hm-sec hm-config is-compact' : 'hm-sec hm-config'} aria-labelledby={`hm-config-${kind}`}>
            <h2 id={`hm-config-${kind}`} className="section-title">
              {view.title}
            </h2>
            <p className="muted">{view.text}</p>
          </section>
        ) : null,
      )}
    </>
  );
}
