import { Link, useLocation } from 'react-router';
import { AUTO_ROOT, ROOT } from './model';
import '../pages/bot.css';

export function BroadcastTabs() {
  const { pathname } = useLocation();
  const auto = pathname.startsWith(AUTO_ROOT);
  return (
    <nav aria-label="Виды рассылок" className="bt-tabs bc-tabs">
      <Link to={ROOT} className="hov" aria-current={auto ? undefined : 'page'}>
        Разовые
      </Link>
      <Link to={AUTO_ROOT} className="hov" aria-current={auto ? 'page' : undefined}>
        Автоматические
      </Link>
    </nav>
  );
}
