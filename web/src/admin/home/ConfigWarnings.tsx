import { useEffect, useState } from 'react';
import { api } from '../../lib/api';
import { useSession } from '../session';

type ConfigWarning = { kind: string; networks?: string[] };

function describe(warning: ConfigWarning): { title: string; text: string } | null {
  if (warning.kind === 'trusted_proxies_everyone') {
    const listed = (warning.networks ?? []).join(' и ') || '0.0.0.0/0';
    return {
      title: 'Любой посетитель может подменить свой адрес',
      text: `В файле .env в SN_TRUSTED_PROXIES стоит ${listed}: панель верит адресу из заголовка X-Forwarded-For от кого угодно. Так обходят ограничения попыток входа и промокодов по адресу, а в журнале остаются чужие адреса. Оставьте в SN_TRUSTED_PROXIES только адреса своих прокси и перезапустите систему.`,
    };
  }
  return null;
}

export function ConfigWarnings({ compact = false }: { compact?: boolean }) {
  const { can } = useSession();
  const allowed = can('settings');
  const [warnings, setWarnings] = useState<ConfigWarning[]>([]);
  useEffect(() => {
    if (!allowed) {
      return;
    }
    let alive = true;
    api<{ warnings: ConfigWarning[] }>('/api/admin/system/warnings')
      .then((view) => {
        if (alive) {
          setWarnings(view.warnings);
        }
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [allowed]);
  const shown = warnings.map((warning) => ({ kind: warning.kind, view: describe(warning) }));
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
