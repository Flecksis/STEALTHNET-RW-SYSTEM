import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { AUDIT, PAGE, filtersOf, paramsOf, queryOf, type AuditItem, type AuditMeta, type AuditPage, type Filters, type Target } from './model';

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function useMeta(enabled: boolean): AuditMeta | null {
  const [meta, setMeta] = useState<AuditMeta | null>(null);
  useEffect(() => {
    if (!enabled) {
      return;
    }
    let alive = true;
    api<AuditMeta>(`${AUDIT}/meta`)
      .then((value) => {
        if (alive) {
          setMeta(value);
        }
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [enabled]);
  return meta;
}

export function useJournal(enabled = true) {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const key = paramsOf(filtersOf(params)).toString();
  const filters = useMemo(() => filtersOf(new URLSearchParams(key)), [key]);
  const meta = useMeta(enabled);
  const [items, setItems] = useState<AuditItem[] | null>(null);
  const [next, setNext] = useState<string | null>(null);
  const [client, setClient] = useState<Target | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ticket = useRef(0);

  const load = useCallback(async (current: Filters) => {
    const mine = ++ticket.current;
    setError(null);
    try {
      const page = await api<AuditPage>(queryOf(current, null));
      if (mine !== ticket.current) {
        return;
      }
      setItems(page.items);
      setNext(page.next);
      setClient(page.client);
    } catch (err) {
      if (mine !== ticket.current) {
        return;
      }
      setItems([]);
      setNext(null);
      setError(failText(err, 'Журнал не загрузился'));
    }
  }, []);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    setItems(null);
    void load(filters);
  }, [enabled, filters, load]);

  const setFilters = useCallback(
    (patch: Partial<Filters>) => {
      const query = paramsOf({ ...filters, ...patch }).toString();
      navigate(`${PAGE}${query ? `?${query}` : ''}`, { replace: true });
    },
    [filters, navigate],
  );

  const more = useCallback(async () => {
    if (next === null || busy) {
      return;
    }
    const mine = ticket.current;
    setBusy(true);
    try {
      const page = await api<AuditPage>(queryOf(filters, next));
      if (mine !== ticket.current) {
        return;
      }
      setItems((list) => [...(list ?? []), ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'Журнал не загрузился'));
    } finally {
      setBusy(false);
    }
  }, [busy, filters, next]);

  const reset = useCallback(() => navigate(PAGE, { replace: true }), [navigate]);
  const filtered = paramsOf(filters).toString() !== '';
  return { filters, setFilters, reset, filtered, meta, items, next, client, error, busy, more };
}

export type Journal = ReturnType<typeof useJournal>;
