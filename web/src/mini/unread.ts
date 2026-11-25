import { useEffect } from 'react';
import { api } from '../lib/api';

const EVERY = 60_000;

export function useSupportUnread(on: boolean, set: (tickets: number) => void): void {
  useEffect(() => {
    if (!on) {
      return;
    }
    let alive = true;
    const check = () => {
      if (document.visibilityState !== 'visible') {
        return;
      }
      api<{ tickets: number }>('/api/client/support/unread')
        .then((res) => {
          if (alive) {
            set(res.tickets);
          }
        })
        .catch(() => undefined);
    };
    const timer = window.setInterval(check, EVERY);
    document.addEventListener('visibilitychange', check);
    return () => {
      alive = false;
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', check);
    };
  }, [on, set]);
}
