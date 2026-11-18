import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import type { ImportView } from '../pages/importModel';
import { useSession } from '../session';

export function ImportHint({ compact = false }: { compact?: boolean }) {
  const { can } = useSession();
  const allowed = can('settings') && can('money');
  const [show, setShow] = useState(false);
  useEffect(() => {
    if (!allowed) {
      return;
    }
    let alive = true;
    api<ImportView>('/api/admin/import')
      .then((view) => {
        if (alive) {
          setShow(view.clients === 0 && view.history.length === 0);
        }
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [allowed]);
  if (!show) {
    return null;
  }
  return (
    <section className={compact ? 'hm-sec hm-import is-compact' : 'hm-sec hm-import'} aria-labelledby="hm-import-h">
      <div className="hm-import-text">
        <h2 id="hm-import-h" className="section-title">
          Переезжаете с другого бота?
        </h2>
        <p className="muted">
          Перенесите клиентов, подписки, балансы и историю оплат из STEALTHNET, бота Бедолаги, Remnashop, Minishop, Remnawave Telegram Shop или Chill Remna bot. Старый бот при этом не меняется, клиентам ничего
          не приходит.
        </p>
      </div>
      <Link to="/admin/settings/import" className="btn-main btn hm-import-go">
        Перенести клиентов
      </Link>
    </section>
  );
}
