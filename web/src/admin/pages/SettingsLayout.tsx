import { useEffect, useState, type ReactNode } from 'react';
import { Link, useLocation } from 'react-router';
import { api } from '../../lib/api';
import { shortName, UPDATES_PATH, upgradeText, versionLine, type BuildSummary } from '../version';
import '../settings.css';
import './updates.css';

const SECTIONS: Array<{ label: string; to?: string }> = [
  { label: 'Общие', to: '/admin/settings/general' },
  { label: 'Remnawave', to: '/admin/settings/remnawave' },
  { label: 'Оформление', to: '/admin/settings/look' },
  { label: 'Оплата', to: '/admin/settings/payments' },
  { label: 'Бот', to: '/admin/settings/bot' },
  { label: 'Документы', to: '/admin/settings/documents' },
  { label: 'Кабинет и мини-приложение' },
  { label: 'Почта и уведомления' },
  { label: 'Администраторы' },
  { label: 'API-ключи' },
  { label: 'Обновления и резервные копии', to: UPDATES_PATH },
];

export function SettingsLayout({ children }: { children: ReactNode }) {
  const { pathname } = useLocation();
  const [summary, setSummary] = useState<BuildSummary | null>(null);
  useEffect(() => {
    let alive = true;
    api<BuildSummary>('/api/admin/system/summary')
      .then((s) => alive && setSummary(s))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);
  const fresh = summary?.update ? shortName(summary.update.label, summary.update.commit, summary.update.built_at) : null;
  const upgrade = summary ? upgradeText(summary) : null;
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
                className={section.to === UPDATES_PATH && fresh ? 'hov subnav-upd' : 'hov'}
                aria-current={pathname === section.to || pathname.startsWith(`${section.to}/`) ? 'page' : undefined}
                aria-label={section.to === UPDATES_PATH && fresh ? `${section.label}, вышла новая версия ${fresh}` : undefined}
              >
                <span className="subnav-label">{section.label}</span>
                {section.to === UPDATES_PATH && fresh ? <span className="subnav-new">новая версия {fresh}</span> : null}
              </Link>
            ) : (
              <span key={section.label} title="Раздел в работе">
                {section.label}
              </span>
            ),
          )}
          {summary ? (
            <Link to={UPDATES_PATH} className={upgrade ? 'subnav-version ver-new' : 'subnav-version'}>
              {upgrade ? `STEALTHNET ${upgrade}` : versionLine('STEALTHNET', summary)}
            </Link>
          ) : null}
        </nav>
        <div className="setbody">{children}</div>
      </div>
    </>
  );
}
