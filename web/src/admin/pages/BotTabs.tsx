import { Link, useLocation } from 'react-router';
import './bot.css';

const TABS = [
  { to: '/admin/settings/bot', label: 'Подключение' },
  { to: '/admin/settings/bot/texts', label: 'Тексты и кнопки' },
  { to: '/admin/settings/bot/layout', label: 'Порядок кнопок' },
  { to: '/admin/settings/bot/images', label: 'Картинки' },
];

export function BotTabs() {
  const { pathname } = useLocation();
  return (
    <nav aria-label="Настройки бота" className="bt-tabs">
      {TABS.map((tab) => (
        <Link key={tab.to} to={tab.to} className="hov" aria-current={pathname === tab.to ? 'page' : undefined}>
          {tab.label}
        </Link>
      ))}
    </nav>
  );
}
