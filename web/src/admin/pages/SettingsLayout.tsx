import { useEffect, useState, type ReactNode } from 'react';
import { Link, useLocation } from 'react-router';
import { api } from '../../lib/api';
import '../settings.css';
import './updates.css';

const UPDATES = '/admin/settings/updates';

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
  { label: 'Обновления и резервные копии', to: UPDATES },
];

type Summary = { version: string; commit: string | null; update: { commit: string } | null };

export function SettingsLayout({ children }: { children: ReactNode }) {
  const { pathname } = useLocation();
  const [summary, setSummary] = useState<Summary | null>(null);
  useEffect(() => {
    let alive = true;
    api<Summary>('/api/admin/system/summary')
      .then((s) => alive && setSummary(s))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);
  const fresh = summary?.update?.commit.slice(0, 7) ?? null;
  return (
    <>
      <h1 className="page-title tight">Настройки</h1>
      <div className="setgrid">
        <nav aria-label="Разделы настроек" className="subnav">
          {SECTIONS.map((section) =>
            section.to ? (
              <Link
                key={section.label}
                to={section.to}
                className="hov"
                aria-current={pathname === section.to ? 'page' : undefined}
                aria-label={section.to === UPDATES && fresh ? `${section.label}, вышла новая сборка` : undefined}
              >
                <span className="subnav-label">{section.label}</span>
                {section.to === UPDATES && fresh ? <span className="subnav-new" aria-hidden="true" /> : null}
              </Link>
            ) : (
              <span key={section.label} title="Раздел в работе">
                {section.label}
              </span>
            ),
          )}
          {summary ? (
            <Link to={UPDATES} className="subnav-version">
              STEALTHNET {summary.version}
              {summary.commit ? ` · ${summary.commit.slice(0, 7)}` : ''}
              {fresh ? ` → ${fresh}` : ''}
            </Link>
          ) : null}
        </nav>
        <div className="setbody">{children}</div>
      </div>
    </>
  );
}
