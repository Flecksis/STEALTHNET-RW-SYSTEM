import { useCallback, useEffect, useState } from 'react';
import { api } from '../../lib/api';
import { useSession } from '../session';
import { disabledModule, failText, ROOT, type AdminGift, type Counts, type Filter, type GiftPage, type GiftSettings } from './model';

export function useGifts() {
  const { can } = useSession();
  const allowed = can('marketing');
  const [filter, setFilter] = useState<Filter>('all');
  const [search, setSearch] = useState('');
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<AdminGift[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [counts, setCounts] = useState<Counts | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [more, setMore] = useState(false);

  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(search.trim()), 300);
    return () => window.clearTimeout(timer);
  }, [search]);

  const fetchPage = useCallback(
    (after: number | null) => {
      const params = new URLSearchParams({ limit: '50', status: filter });
      if (query) {
        params.set('q', query);
      }
      if (after !== null) {
        params.set('after', String(after));
      }
      return api<GiftPage>(`${ROOT}/gifts?${params.toString()}`);
    },
    [filter, query],
  );

  const load = useCallback(async () => {
    if (!allowed) {
      return;
    }
    try {
      const [page, sum] = await Promise.all([fetchPage(null), api<Counts>(`${ROOT}/summary`)]);
      setItems(page.items);
      setNext(page.next);
      setCounts(sum);
      setError(null);
      setOff(false);
    } catch (err) {
      setItems([]);
      setNext(null);
      setOff(disabledModule(err));
      setError(failText(err, 'Подарки не загрузились'));
    }
  }, [allowed, fetchPage]);

  useEffect(() => {
    setItems(null);
    void load();
  }, [load]);

  const loadMore = async () => {
    if (next === null) {
      return;
    }
    setMore(true);
    try {
      const page = await fetchPage(next);
      setItems((list) => [...(list ?? []), ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'Подарки не загрузились'));
    } finally {
      setMore(false);
    }
  };

  return { allowed, filter, setFilter, search, setSearch, items, next, counts, error, off, more, loadMore };
}

export function useGiftSettings(open: boolean) {
  const [view, setView] = useState<GiftSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open || view) {
      return;
    }
    api<GiftSettings>(`${ROOT}/settings`)
      .then(setView)
      .catch((err) => setError(failText(err, 'Настройки не загрузились')));
  }, [open, view]);

  const save = async (tariffs: number[], codeDays: number): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      setView(await api<GiftSettings>(`${ROOT}/settings`, { method: 'PUT', body: { tariffs, code_days: codeDays } }));
      return true;
    } catch (err) {
      setError(failText(err, 'Настройки не сохранились'));
      return false;
    } finally {
      setBusy(false);
    }
  };

  return { view, error, busy, save };
}
