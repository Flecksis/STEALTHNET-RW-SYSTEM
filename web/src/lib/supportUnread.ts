import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from './api';
import { unreadTurns } from './unreadTurns';

const EVERY = 60_000;

export function useSupportUnread(on: boolean, page = ''): [number, (tickets: number) => void] {
  const [unread, show] = useState(0);
  const [turns] = useState(() => unreadTurns(show));
  const shown = useRef(page);

  const check = useCallback(() => {
    if (document.visibilityState === 'visible') {
      void turns.ask(async () => (await api<{ tickets: number }>('/api/client/support/unread')).tickets);
    }
  }, [turns]);

  useEffect(() => {
    if (!on) {
      return;
    }
    const timer = window.setInterval(check, EVERY);
    document.addEventListener('visibilitychange', check);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', check);
      turns.drop();
    };
  }, [on, check, turns]);

  useEffect(() => {
    if (shown.current === page) {
      return;
    }
    shown.current = page;
    if (on) {
      check();
    }
  }, [on, page, check]);

  return [unread, turns.put];
}
