import { Link, useLocation } from 'react-router';
import './bot.css';

const TABS = [
  { to: '/admin/settings/mail', label: 'Отправка' },
  { to: '/admin/settings/mail/letters', label: 'Шаблоны писем' },
];

export function MailTabs() {
  const { pathname } = useLocation();
  return (
    <nav aria-label="Почта" className="bt-tabs">
      {TABS.map((tab) => (
        <Link key={tab.to} to={tab.to} className="hov" aria-current={pathname === tab.to ? 'page' : undefined}>
          {tab.label}
        </Link>
      ))}
    </nav>
  );
}
