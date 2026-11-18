import { Link, useLocation } from 'react-router';
import { JOURNAL } from './journal';
import '../pages/bot.css';
import '../protect/protect.css';

export function MoneyTabs() {
  const { pathname } = useLocation();
  const journal = pathname.startsWith(JOURNAL);
  return (
    <nav aria-label="Разделы денег" className="bt-tabs pt-tabs">
      <Link to="/admin/money" className="hov" aria-current={journal ? undefined : 'page'}>
        Платежи
      </Link>
      <Link to={JOURNAL} className="hov" aria-current={journal ? 'page' : undefined}>
        Уведомления платёжек
      </Link>
    </nav>
  );
}
