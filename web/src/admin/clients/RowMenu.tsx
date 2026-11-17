import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as KeyEvent } from 'react';
import { createPortal } from 'react-dom';
import { Link } from 'react-router';
import { Icon, type IconName } from '../../lib/icons';
import { usePhone } from '../../lib/media';

export type RowAction = 'extend' | 'grant' | 'change' | 'message' | 'balance' | 'block' | 'unblock';

export type RowItem = { key: RowAction; label: string; icon: IconName; bad?: boolean };

type Props = { name: string; href: string; items: RowItem[]; onPick: (key: RowAction) => void };

const GAP = 4;
const EDGE = 8;

function entries(menu: HTMLElement | null): HTMLElement[] {
  return menu ? [...menu.querySelectorAll<HTMLElement>('[role="menuitem"]')] : [];
}

export function RowMenu({ name, href, items, onPick }: Props) {
  const phone = usePhone();
  const [open, setOpen] = useState(false);
  const [place, setPlace] = useState<{ top: number; left: number } | null>(null);
  const button = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);

  const aimed = useRef(false);

  const close = (focus: boolean) => {
    setOpen(false);
    setPlace(null);
    aimed.current = false;
    if (focus) {
      button.current?.focus();
    }
  };

  const placeMenu = (): boolean => {
    if (!button.current || !menu.current) {
      return false;
    }
    const rect = button.current.getBoundingClientRect();
    if (rect.bottom <= 0 || rect.top >= window.innerHeight) {
      return false;
    }
    const box = menu.current.getBoundingClientRect();
    const below = rect.bottom + GAP + box.height <= window.innerHeight - EDGE;
    const top = below ? rect.bottom + GAP : Math.max(EDGE, rect.top - GAP - box.height);
    const left = Math.max(EDGE, Math.min(rect.right - box.width, window.innerWidth - box.width - EDGE));
    setPlace({ top, left });
    return true;
  };

  useLayoutEffect(() => {
    if (open && !phone) {
      placeMenu();
    }
  }, [open, phone]);

  useEffect(() => {
    if (place && !aimed.current) {
      aimed.current = true;
      entries(menu.current)[0]?.focus({ preventScroll: true });
    }
  }, [place]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const shut = () => {
      setOpen(false);
      setPlace(null);
      aimed.current = false;
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        shut();
        button.current?.focus();
      }
    };
    document.addEventListener('keydown', onKey);
    if (phone) {
      return () => document.removeEventListener('keydown', onKey);
    }
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!menu.current?.contains(target) && !button.current?.contains(target)) {
        shut();
      }
    };
    const follow = () => {
      if (!placeMenu()) {
        shut();
      }
    };
    document.addEventListener('mousedown', onDown);
    window.addEventListener('resize', follow);
    window.addEventListener('scroll', follow, true);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onDown);
      window.removeEventListener('resize', follow);
      window.removeEventListener('scroll', follow, true);
    };
  }, [open, phone]);

  const onMenuKey = (event: KeyEvent<HTMLDivElement>) => {
    const list = entries(menu.current);
    const at = list.indexOf(document.activeElement as HTMLElement);
    let next = -1;
    if (event.key === 'ArrowDown') {
      next = at < 0 ? 0 : (at + 1) % list.length;
    } else if (event.key === 'ArrowUp') {
      next = at <= 0 ? list.length - 1 : at - 1;
    } else if (event.key === 'Home') {
      next = 0;
    } else if (event.key === 'End') {
      next = list.length - 1;
    } else if (event.key === 'Tab') {
      close(false);
      return;
    }
    if (next >= 0) {
      event.preventDefault();
      list[next]?.focus();
    }
  };

  const pick = (key: RowAction) => {
    close(false);
    onPick(key);
  };

  return (
    <>
      <button
        ref={button}
        type="button"
        className="dots-btn hov"
        aria-label={`Действия: ${name}`}
        aria-haspopup={phone ? 'dialog' : 'menu'}
        aria-expanded={open}
        onClick={() => (open ? close(false) : setOpen(true))}
      >
        <Icon name="dots" size={18} stroke={2.6} />
      </button>
      {open && !phone
        ? createPortal(
            <div
              ref={menu}
              role="menu"
              aria-label={`Действия: ${name}`}
              className="row-menu"
              style={place ? { top: place.top, left: place.left } : { top: 0, left: 0, visibility: 'hidden' }}
              onKeyDown={onMenuKey}
            >
              <Link to={href} role="menuitem" className="hov">
                Открыть карточку
              </Link>
              {items.map((item, index) => (
                <span key={item.key} style={{ display: 'contents' }}>
                  {item.bad && !items[index - 1]?.bad ? <span className="row-menu-sep" role="separator" /> : null}
                  <button type="button" role="menuitem" className={item.bad ? 'hov bad' : 'hov'} onClick={() => pick(item.key)}>
                    {item.label}
                  </button>
                </span>
              ))}
            </div>,
            document.body,
          )
        : null}
      {open && phone
        ? createPortal(
            <div className="psheet-back" role="presentation" onClick={() => close(true)}>
              <div className="psheet" role="dialog" aria-modal="true" aria-label={`Действия: ${name}`} onClick={(event) => event.stopPropagation()}>
                <div className="row-sheet-name">{name}</div>
                <div className="psheet-list">
                  <Link to={href} className="psheet-row press">
                    <Icon name="user" size={21} />
                    <span className="psheet-name">Открыть карточку</span>
                    <Icon name="chevron-right" size={16} />
                  </Link>
                  {items.map((item) => (
                    <button key={item.key} type="button" className={item.bad ? 'psheet-row press bad' : 'psheet-row press'} onClick={() => pick(item.key)}>
                      <Icon name={item.icon} size={21} />
                      <span className="psheet-name">{item.label}</span>
                    </button>
                  ))}
                </div>
              </div>
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
