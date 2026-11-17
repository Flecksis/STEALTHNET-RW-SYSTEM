import type { ReactNode } from 'react';
import { Link, useLocation } from 'react-router';
import '../settings.css';

const SECTIONS: Array<{ label: string; to?: string }> = [
  { label: 'Общие', to: '/admin/settings/general' },
  { label: 'Remnawave', to: '/admin/settings/remnawave' },
  { label: 'Оформление', to: '/admin/settings/look' },
  { label: 'Оплата', to: '/admin/settings/payments' },
  { label: 'Бот', to: '/admin/settings/bot' },
  { label: 'Кабинет и мини-приложение' },
  { label: 'Почта и уведомления' },
  { label: 'Администраторы' },
  { label: 'API-ключи' },
  { label: 'Обновления и резервные копии' },
];

export function SettingsLayout({ children }: { children: ReactNode }) {
  const { pathname } = useLocation();
  return (
    <>
      <h1 className="page-title tight">Настройки</h1>
      <div className="setgrid">
        <nav aria-label="Разделы настроек" className="subnav">
          {SECTIONS.map((section) =>
            section.to ? (
              <Link key={section.label} to={section.to} className="hov" aria-current={pathname === section.to ? 'page' : undefined}>
                {section.label}
              </Link>
            ) : (
              <span key={section.label} title="Раздел в работе">
                {section.label}
              </span>
            ),
          )}
        </nav>
        <div className="setbody">{children}</div>
      </div>
    </>
  );
}
