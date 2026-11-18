import { Link, useLocation } from 'react-router';
import { useSession } from '../session';
import '../pages/bot.css';
import './links.css';

export function PromoTabs({ phone = false }: { phone?: boolean }) {
  const { me } = useSession();
  const { pathname } = useLocation();
  const enabled = me.modules ?? [];
  if (!enabled.includes('promo') || !enabled.includes('promolinks')) {
    return null;
  }
  const links = pathname.startsWith('/admin/m/promolinks');
  return (
    <nav aria-label="Промокоды и промо-ссылки" className={phone ? 'bt-tabs pl-tabs pl-ptabs' : 'bt-tabs pl-tabs'}>
      <Link to="/admin/m/promo" className="hov" aria-current={links ? undefined : 'page'}>
        Промокоды
      </Link>
      <Link to="/admin/m/promolinks" className="hov" aria-current={links ? 'page' : undefined}>
        Промо-ссылки
      </Link>
    </nav>
  );
}
