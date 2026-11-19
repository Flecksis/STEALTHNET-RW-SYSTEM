import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { ago, count, money, plural, type Money } from '../../lib/format';
import type { Person } from '../home/model';
import { useSession } from '../session';

export type WithdrawalStatus = 'holding' | 'pending' | 'paid' | 'rejected' | 'canceled';

export type PayoutOption = { id: string; title: string; hint: string; min: Money | null };

export type Withdrawal = {
  id: number;
  public_id: string;
  client_id: number;
  amount: Money;
  method: string;
  method_title?: string;
  details: string;
  status: WithdrawalStatus;
  comment: string | null;
  created_at: string;
  decided_at: string | null;
  decided_by: string | null;
  client: Person | null;
};

export type Summary = {
  levels: [number, number, number];
  withdraw_min: Money | null;
  withdrawals_enabled: boolean;
  payout_methods?: PayoutOption[];
  invited: number;
  referrers: number;
  earned: Money;
  paid_out: Money;
  pending_count: number;
  pending: Money;
  holders: number;
  balance: Money;
};

export type Referrer = {
  client_id: number;
  client: Person | null;
  code: string | null;
  personal_percent: number | null;
  invited: number;
  earned: Money;
  paid_out: Money;
  balance: Money;
};

type Page<T> = { items: T[]; next: number | null };

export type HistoryFilter = 'decided' | 'paid' | 'rejected' | 'canceled';

export const HISTORY: Array<[HistoryFilter, string]> = [
  ['decided', 'Все'],
  ['paid', 'Выплачено'],
  ['rejected', 'Отклонено'],
  ['canceled', 'Отменено'],
];

export type Verdict = 'paid' | 'rejected';

export function methodName(w: Withdrawal): string {
  return w.method_title?.trim() || w.method;
}

export function detailsText(w: Withdrawal): string {
  if (/^\d{13,19}$/.test(w.details)) {
    return w.details.replace(/(\d{4})(?=\d)/g, '$1 ');
  }
  return w.details;
}

export function contactLine(person: Person | null): string | null {
  if (person?.telegram_username) {
    return `@${person.telegram_username}`;
  }
  if (person?.email) {
    return person.email;
  }
  if (person?.telegram_id) {
    return `Telegram ${person.telegram_id}`;
  }
  return null;
}

export function statusText(w: Withdrawal): { text: string; tone: 'ink' | 'bad' | 'muted' | 'warn' } {
  switch (w.status) {
    case 'paid':
      return { text: 'Выплачено', tone: 'ink' };
    case 'rejected':
      return { text: 'Отклонено', tone: 'bad' };
    case 'canceled':
      return { text: 'Отменено', tone: 'muted' };
    default:
      return { text: 'Ждёт решения', tone: 'warn' };
  }
}

export function recentText(iso: string, now = Date.now()): string | null {
  const at = Date.parse(iso);
  return Number.isNaN(at) || now - at >= 86_400_000 ? null : ago(iso);
}

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function termsText(s: Summary): string {
  const [first, second, third] = s.levels;
  const deeper = [second ? `${second}% со второго уровня` : null, third ? `${third}% с третьего` : null].filter(Boolean).join(' и ');
  const levels = `Пригласившему ${first}% с оплат друзей${deeper ? `, ${deeper}` : ''}.`;
  return `${levels} ${s.withdrawals_enabled ? withdrawText(s) : 'Заявки на вывод сейчас не принимаются.'}`;
}

function withdrawText(s: Summary): string {
  const methods = s.payout_methods ?? [];
  const mins = new Set(methods.map((m) => m.min?.minor ?? 0));
  if (mins.size > 1) {
    return `Вывод: ${methods.map((m) => `${m.title} ${m.min ? `от ${money(m.min)}` : 'без минимума'}`).join(', ')}.`;
  }
  const min = methods.length ? (methods[0]?.min ?? null) : s.withdraw_min;
  return min ? `Вывод от ${money(min)}.` : 'Вывод без минимальной суммы.';
}

export function totalsText(s: Summary): string {
  if (!s.invited) {
    return 'По приглашениям пока никто не пришёл.';
  }
  const who = `${count(s.referrers, 'участник', 'участника', 'участников')} ${plural(s.referrers, 'пригласил', 'пригласили', 'пригласили')} ${count(s.invited, 'клиента', 'клиентов', 'клиентов')}.`;
  return `${who} Начислено ${money(s.earned)}, выплачено ${money(s.paid_out)}, на реферальных балансах ${money(s.balance)}.`;
}

export function queueText(s: Summary | null, queue: Withdrawal[] | null): string {
  const n = s?.pending_count ?? queue?.length ?? 0;
  if (!n) {
    return '';
  }
  const total = s ? s.pending : null;
  return `${count(n, 'заявка', 'заявки', 'заявок')} на вывод${total ? `, всего ${money(total)}` : ''}`;
}

export function percentText(r: Referrer, s: Summary | null): { text: string; own: boolean } {
  if (r.personal_percent !== null) {
    return { text: `${r.personal_percent}%`, own: true };
  }
  return { text: s ? `${s.levels[0]}%` : '—', own: false };
}

function disabledModule(err: unknown): boolean {
  return err instanceof ApiError && err.code === 'module.disabled';
}

export function useReferral() {
  const { can, me } = useSession();
  const allowed = can('money');
  const [summary, setSummary] = useState<Summary | null>(null);
  const [queue, setQueue] = useState<Withdrawal[] | null>(null);
  const [queueNext, setQueueNext] = useState<number | null>(null);
  const [filter, setFilter] = useState<HistoryFilter>('decided');
  const [history, setHistory] = useState<Withdrawal[] | null>(null);
  const [historyNext, setHistoryNext] = useState<number | null>(null);
  const [referrers, setReferrers] = useState<Referrer[] | null>(null);
  const [referrersNext, setReferrersNext] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [loading, setLoading] = useState<'queue' | 'history' | 'referrers' | null>(null);

  const fail = useCallback((err: unknown, fallback: string) => {
    setOff(disabledModule(err));
    setError(failText(err, fallback));
  }, []);

  const loadSummary = useCallback(async () => {
    try {
      setSummary(await api<Summary>('/api/admin/m/referral/summary'));
    } catch (err) {
      fail(err, 'Сводка не загрузилась');
    }
  }, [fail]);

  const loadQueue = useCallback(async () => {
    try {
      const page = await api<Page<Withdrawal>>('/api/admin/m/referral/withdrawals?status=pending&limit=100');
      setQueue(page.items);
      setQueueNext(page.next);
      setError(null);
      setOff(false);
    } catch (err) {
      setQueue([]);
      fail(err, 'Заявки не загрузились');
    }
  }, [fail]);

  const loadReferrers = useCallback(async () => {
    try {
      const page = await api<Page<Referrer>>('/api/admin/m/referral/referrers?limit=20');
      setReferrers(page.items);
      setReferrersNext(page.next);
    } catch (err) {
      setReferrers([]);
      fail(err, 'Список участников не загрузился');
    }
  }, [fail]);

  useEffect(() => {
    if (!allowed) {
      return;
    }
    void loadQueue();
    void loadSummary();
    void loadReferrers();
  }, [allowed, loadQueue, loadSummary, loadReferrers]);

  useEffect(() => {
    if (!allowed) {
      return;
    }
    let alive = true;
    setHistory(null);
    api<Page<Withdrawal>>(`/api/admin/m/referral/withdrawals?status=${filter}&limit=20`)
      .then((page) => {
        if (alive) {
          setHistory(page.items);
          setHistoryNext(page.next);
        }
      })
      .catch((err: unknown) => {
        if (alive) {
          setHistory([]);
          fail(err, 'История не загрузилась');
        }
      });
    return () => {
      alive = false;
    };
  }, [allowed, filter, fail]);

  const more = async (what: 'queue' | 'history' | 'referrers') => {
    setLoading(what);
    try {
      if (what === 'queue' && queueNext !== null) {
        const page = await api<Page<Withdrawal>>(`/api/admin/m/referral/withdrawals?status=pending&limit=100&after=${queueNext}`);
        setQueue((list) => [...(list ?? []), ...page.items]);
        setQueueNext(page.next);
      } else if (what === 'history' && historyNext !== null) {
        const page = await api<Page<Withdrawal>>(`/api/admin/m/referral/withdrawals?status=${filter}&limit=20&after=${historyNext}`);
        setHistory((list) => [...(list ?? []), ...page.items]);
        setHistoryNext(page.next);
      } else if (what === 'referrers' && referrersNext !== null) {
        const page = await api<Page<Referrer>>(`/api/admin/m/referral/referrers?limit=20&after=${referrersNext}`);
        setReferrers((list) => [...(list ?? []), ...page.items]);
        setReferrersNext(page.next);
      }
    } catch (err) {
      setError(failText(err, 'Список не загрузился'));
    } finally {
      setLoading(null);
    }
  };

  const decide = async (w: Withdrawal, verdict: Verdict, comment: string): Promise<Withdrawal> => {
    const text = comment.trim();
    const body = verdict === 'paid' ? { decision: 'paid', comment: text || null } : { decision: 'rejected', comment: text };
    const saved = await api<Withdrawal>(`/api/admin/m/referral/withdrawals/${w.id}/decision`, { method: 'POST', body });
    const decided: Withdrawal = { ...w, ...saved, client: w.client, decided_by: me.email };
    setQueue((list) => list?.filter((x) => x.id !== w.id) ?? list);
    if (filter === 'decided' || filter === decided.status) {
      setHistory((list) => (list ? [decided, ...list.filter((x) => x.id !== w.id)] : list));
    }
    void loadSummary();
    void loadReferrers();
    return decided;
  };

  const setPercent = async (r: Referrer, percent: number | null) => {
    await api(`/api/admin/m/referral/clients/${r.client_id}/percent`, { method: 'PUT', body: { percent } });
    setReferrers((list) => list?.map((x) => (x.client_id === r.client_id ? { ...x, personal_percent: percent } : x)) ?? list);
  };

  const reloadQueue = async () => {
    await Promise.all([loadQueue(), loadSummary()]);
  };

  return {
    allowed,
    summary,
    queue,
    queueNext,
    filter,
    setFilter,
    history,
    historyNext,
    referrers,
    referrersNext,
    error,
    off,
    loading,
    more,
    decide,
    setPercent,
    reloadQueue,
  };
}

export function useCopy(): [string | null, (key: string, text: string) => void] {
  const [copied, setCopied] = useState<string | null>(null);
  useEffect(() => {
    if (!copied) {
      return;
    }
    const timer = window.setTimeout(() => setCopied(null), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);
  const copy = (key: string, text: string) => {
    navigator.clipboard
      .writeText(text)
      .then(() => setCopied(key))
      .catch(() => setCopied(null));
  };
  return [copied, copy];
}

export function parsePercent(raw: string): number | null | 'bad' {
  const clean = raw.trim().replace('%', '').trim();
  if (!clean) {
    return null;
  }
  if (!/^\d{1,3}$/.test(clean)) {
    return 'bad';
  }
  const n = Number(clean);
  return n <= 100 ? n : 'bad';
}
