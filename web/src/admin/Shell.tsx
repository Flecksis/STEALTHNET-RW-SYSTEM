import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Link, useLocation, useNavigate } from 'react-router';
import { Icon } from '../lib/icons';
import { useMedia } from '../lib/media';
import { readStored, writeStored, type Theme } from '../lib/theme';
import { isCurrent, navItems, type NavItem } from './nav';
import { ROLE_LABEL, useSession } from './session';
import './shell.css';

const PIN_KEY = 'sn.admin.pinned';

type Props = {
  theme: Theme;
  onFlipTheme: () => void;
  supportBadge?: number;
  children: ReactNode;
};

function initial(text: string | null | undefined): string {
  const first = (text ?? '').trim().charAt(0);
  return first ? first.toUpperCase() : 'А';
}

function useOutside(open: boolean, close: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) {
        close();
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        close();
      }
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, close]);
  return ref;
}

function ThemeLabel(theme: Theme): string {
  return theme === 'dark' ? 'Светлая тема' : 'Тёмная тема';
}

function Rail({ items, path, theme, onFlipTheme }: { items: NavItem[]; path: string; theme: Theme; onFlipTheme: () => void }) {
  const { me, logout } = useSession();
  const [pinned, setPinned] = useState(() => readStored(PIN_KEY) === '1');
  const [whoOpen, setWhoOpen] = useState(false);
  const whoRef = useOutside(whoOpen, () => setWhoOpen(false));
  const togglePin = () => {
    setPinned((value) => {
      writeStored(PIN_KEY, value ? null : '1');
      return !value;
    });
  };
  let lastGroup: NavItem['group'] | null = null;
  return (
    <div className={pinned ? 'rail-hold pinned' : 'rail-hold'}>
      <nav aria-label="Меню" className={pinned ? 'rail pinned' : 'rail'}>
        <div className="rail-logo">
          <span className="mark" aria-hidden="true">
            S
          </span>
          <span className="lbl">{me.brand_name.toUpperCase()}</span>
        </div>
        {items.map((item) => {
          const sep = lastGroup !== null && item.group !== lastGroup && item.group !== 'core';
          lastGroup = item.group;
          const current = isCurrent(item, path);
          return (
            <span key={item.key} style={{ display: 'contents' }}>
              {sep ? <span className="rail-sep" aria-hidden="true" /> : null}
              <Link to={item.to} className="rail-item hov" aria-current={current ? 'page' : undefined}>
                <span className="rail-icon">
                  <Icon name={item.icon} />
                  {item.badge ? <span className="rail-dot" aria-hidden="true" /> : null}
                </span>
                <span className="lbl grow">{item.label}</span>
                {item.badge ? <span className="lbl rail-count">{item.badge}</span> : null}
              </Link>
            </span>
          );
        })}
        <span className="rail-fill" />
        <button type="button" className="rail-item quiet hov" onClick={onFlipTheme}>
          <span className="rail-icon">
            <Icon name={theme === 'dark' ? 'sun' : 'moon'} />
          </span>
          <span className="lbl">{ThemeLabel(theme)}</span>
        </button>
        <button type="button" className="rail-item quiet hov" aria-pressed={pinned} onClick={togglePin}>
          <span className="rail-icon">
            <Icon name="pin" />
          </span>
          <span className="lbl">Закрепить меню</span>
        </button>
        <button
          type="button"
          className="rail-who"
          aria-expanded={whoOpen}
          aria-label="Профиль администратора"
          onClick={() => setWhoOpen((v) => !v)}
        >
          <span className="avatar" aria-hidden="true">
            {initial(me.email)}
          </span>
          <span className="lbl">
            <span className="who-name" title={me.email ?? undefined}>
              {me.email ?? 'Администратор'}
            </span>
            <span className="who-role">{ROLE_LABEL[me.admin.role]}</span>
          </span>
        </button>
      </nav>
      {whoOpen ? (
        <div className="profile-menu rail-pop" ref={whoRef}>
          <div className="profile-head">
            <b title={me.email ?? undefined}>{me.email ?? 'Администратор'}</b>
            <span className="muted" style={{ fontSize: 12 }}>
              {ROLE_LABEL[me.admin.role]}
            </span>
          </div>
          <button type="button" className="hov" onClick={() => void logout()}>
            <Icon name="logout" size={18} />
            Выйти
          </button>
        </div>
      ) : null}
    </div>
  );
}

function TopBar({ items, path, theme, onFlipTheme }: { items: NavItem[]; path: string; theme: Theme; onFlipTheme: () => void }) {
  const { me, logout } = useSession();
  const [moreOpen, setMoreOpen] = useState(false);
  const [profileOpen, setProfileOpen] = useState(false);
  const moreRef = useOutside(moreOpen, () => setMoreOpen(false));
  const profileRef = useOutside(profileOpen, () => setProfileOpen(false));
  const location = useLocation();
  useEffect(() => {
    setMoreOpen(false);
    setProfileOpen(false);
  }, [location.pathname]);
  const main = items.filter((item) => item.group === 'core');
  const more = items.filter((item) => item.group !== 'core');
  const moreCurrent = more.some((item) => isCurrent(item, path));
  let lastGroup: NavItem['group'] | null = null;
  return (
    <header className="topbar">
      <div className="topbar-in">
        <Link to="/admin" className="topbar-home">
          <span className="mark" aria-hidden="true">
            S
          </span>
          <span>{me.brand_name.toUpperCase()}</span>
        </Link>
        <nav aria-label="Разделы" className="tabs">
          {main.map((item) => (
            <Link key={item.key} to={item.to} className="tab hov" aria-current={isCurrent(item, path) ? 'page' : undefined}>
              <span>{item.label}</span>
              {item.badge ? <span className="tab-count">{item.badge}</span> : null}
            </Link>
          ))}
          {more.length > 0 ? (
            <div className="more-wrap" ref={moreRef}>
              <button
                type="button"
                className={moreCurrent ? 'tab more current hov' : 'tab more hov'}
                aria-expanded={moreOpen}
                onClick={() => setMoreOpen((v) => !v)}
              >
                <span>Ещё</span>
                <Icon name="chevron-down" size={15} stroke={2} />
              </button>
              {moreOpen ? (
                <div className="more-menu">
                  {more.map((item) => {
                    const sep = lastGroup !== null && item.group !== lastGroup;
                    lastGroup = item.group;
                    return (
                      <span key={item.key} style={{ display: 'contents' }}>
                        {sep ? <span className="more-sep" aria-hidden="true" /> : null}
                        <Link to={item.to} className="hov" aria-current={isCurrent(item, path) ? 'page' : undefined}>
                          {item.label}
                        </Link>
                      </span>
                    );
                  })}
                </div>
              ) : null}
            </div>
          ) : null}
        </nav>
        <div className="topbar-end">
          <button type="button" className="icon-btn hov" aria-label={ThemeLabel(theme)} title={ThemeLabel(theme)} onClick={onFlipTheme}>
            <Icon name={theme === 'dark' ? 'sun' : 'moon'} />
          </button>
          <div className="profile-wrap" ref={profileRef}>
            <button
              type="button"
              className="avatar"
              aria-label="Профиль администратора"
              aria-expanded={profileOpen}
              onClick={() => setProfileOpen((v) => !v)}
              style={{ cursor: 'pointer', fontWeight: 700 }}
            >
              {initial(me.email)}
            </button>
            {profileOpen ? (
              <div className="profile-menu">
                <div className="profile-head">
                  <b title={me.email ?? undefined}>{me.email ?? 'Администратор'}</b>
                  <span className="muted" style={{ fontSize: 12 }}>
                    {ROLE_LABEL[me.admin.role]}
                  </span>
                </div>
                <button type="button" className="hov" onClick={() => void logout()}>
                  <Icon name="logout" size={18} />
                  Выйти
                </button>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </header>
  );
}

export function Shell({ theme, onFlipTheme, supportBadge, children }: Props) {
  const { me, can } = useSession();
  const location = useLocation();
  const narrow = useMedia('(max-width: 760px)');
  const layout = narrow ? 'top' : me.appearance.admin;
  const items = navItems(me, can, supportBadge);
  const navigate = useNavigate();
  useEffect(() => {
    if (location.pathname === '/admin/settings' || location.pathname === '/admin/settings/') {
      navigate('/admin/settings/look', { replace: true });
    }
  }, [location.pathname, navigate]);
  return (
    <div className="adm" data-layout={layout}>
      {layout === 'side' ? (
        <Rail items={items} path={location.pathname} theme={theme} onFlipTheme={onFlipTheme} />
      ) : (
        <TopBar items={items} path={location.pathname} theme={theme} onFlipTheme={onFlipTheme} />
      )}
      <main className="page">{children}</main>
    </div>
  );
}
