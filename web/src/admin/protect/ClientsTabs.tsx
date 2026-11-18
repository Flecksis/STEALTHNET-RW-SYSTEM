import { Link, useLocation } from 'react-router';
import { useSession } from '../session';
import { protectSections, type Sections } from './sections';
import '../pages/bot.css';
import './protect.css';

export function useProtectSections(): Sections {
  const { me, can } = useSession();
  return protectSections(me.modules, can('clients_read'));
}

export function ClientsTabs() {
  const available = useProtectSections();
  const { pathname } = useLocation();
  if (!available.antibot && !available.limits) {
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
