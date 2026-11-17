import type { ReactNode } from 'react';
import { Link } from 'react-router';
import { Icon } from '../lib/icons';
import { useSession } from './session';

type HeaderProps = {
  title: string;
  back?: { to: string; label: string };
  children?: ReactNode;
};

export function PhoneHeader({ title, back, children }: HeaderProps) {
  const { theme, flipTheme } = useSession();
  const label = theme === 'dark' ? 'Светлая тема' : 'Тёмная тема';
  return (
    <header className="ph-head">
      {back ? (
        <Link to={back.to} className="ph-circ press" aria-label={back.label}>
          <span className="ph-dot">
            <Icon name="chevron-left" size={20} />
          </span>
        </Link>
      ) : (
        <span className="ph-logo" aria-hidden="true">
          S
        </span>
      )}
      <h1 className="ph-title">{title}</h1>
      <span className="ph-tools">
        {children}
        <button type="button" className="ph-circ press" aria-label={label} onClick={flipTheme}>
          <span className="ph-dot">
            <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={19} />
          </span>
        </button>
      </span>
    </header>
  );
}
