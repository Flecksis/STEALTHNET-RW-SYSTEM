import { useEffect, useState } from 'react';
import { Link, useLocation } from 'react-router';
import { api } from '../../lib/api';
import { ANTIBOT, LIMITS } from './model';
import '../pages/bot.css';
import './protect.css';

type Available = { antibot: boolean; limits: boolean };

let known: Available | null = null;

async function reachable(path: string): Promise<boolean> {
  try {
    await api(path);
    return true;
  } catch {
    return false;
  }
}

export function useProtectSections(): Available | null {
  const [available, setAvailable] = useState<Available | null>(known);
  useEffect(() => {
    let alive = true;
    void Promise.all([reachable(`${ANTIBOT}/summary`), reachable(`${LIMITS}/summary`)]).then(([antibot, limits]) => {
      known = { antibot, limits };
      if (alive) {
        setAvailable(known);
      }
    });
    return () => {
      alive = false;
    };
  }, []);
  return available;
}

export function ClientsTabs() {
  const available = useProtectSections();
  const { pathname } = useLocation();
  if (!available || (!available.antibot && !available.limits)) {
    return null;
  }
  const here = (path: string) => (pathname === path ? 'page' : undefined);
  return (
    <nav aria-label="Разделы клиентов" className="bt-tabs pt-tabs">
      <Link to="/admin/clients" className="hov" aria-current={here('/admin/clients')}>
        Все клиенты
      </Link>
      {available.antibot ? (
        <Link to="/admin/clients/antibot" className="hov" aria-current={here('/admin/clients/antibot')}>
          Антибот
        </Link>
      ) : null}
      {available.limits ? (
        <Link to="/admin/clients/limits" className="hov" aria-current={here('/admin/clients/limits')}>
          Лимиты трафика
        </Link>
      ) : null}
    </nav>
  );
}
