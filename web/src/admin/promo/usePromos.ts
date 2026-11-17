import { useCallback, useEffect, useState } from 'react';
import { api } from '../../lib/api';
import { useSession } from '../session';
import { disabledModule, failText, type Filter, type PromoCode, type PromoPage, type TariffBrief } from './model';

export function usePromos() {
  const { can } = useSession();
  const allowed = can('marketing');
  const [filter, setFilter] = useState<Filter>('all');
  const [items, setItems] = useState<PromoCode[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [more, setMore] = useState(false);
  const [tariffs, setTariffs] = useState<TariffBrief[] | null>(null);
  const [busy, setBusy] = useState<number | null>(null);

  const query = useCallback(
    (after: number | null) => {
      const params = new URLSearchParams({ limit: '50', status: filter });
      if (after !== null) {
        params.set('after', String(after));
      }
      return api<PromoPage>(`/api/admin/m/promo/codes?${params.toString()}`);
    },
    [filter],
  );

  const load = useCallback(async () => {
    if (!allowed) {
      return;
    }
    try {
      const page = await query(null);
      setItems(page.items);
      setNext(page.next);
      setError(null);
      setOff(false);
    } catch (err) {
      setItems([]);
      setNext(null);
      setOff(disabledModule(err));
      setError(failText(err, 'Промокоды не загрузились'));
    }
  }, [allowed, query]);

  useEffect(() => {
    setItems(null);
    void load();
  }, [load]);

  useEffect(() => {
    if (!can('tariffs')) {
      return;
    }
    let alive = true;
    api<{ tariffs: TariffBrief[] }>('/api/admin/tariffs')
      .then((view) => alive && setTariffs(view.tariffs.map((t) => ({ id: t.id, name: t.name }))))
      .catch(() => alive && setTariffs(null));
    return () => {
      alive = false;
    };
  }, [can]);

  const loadMore = async () => {
    if (next === null) {
      return;
    }
    setMore(true);
    try {
      const page = await query(next);
      setItems((list) => [...(list ?? []), ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'Промокоды не загрузились'));
    } finally {
      setMore(false);
    }
  };

  const switchCode = async (code: PromoCode, active: boolean): Promise<string> => {
    setBusy(code.id);
    try {
      const saved = await api<PromoCode>(`/api/admin/m/promo/codes/${code.id}/${active ? 'enable' : 'disable'}`, { method: 'POST' });
      setItems((list) => list?.map((p) => (p.id === saved.id ? { ...p, ...saved } : p)) ?? list);
      return active ? `Промокод ${saved.code} включён` : `Промокод ${saved.code} выключен`;
    } finally {
      setBusy(null);
    }
  };

  const added = (code: PromoCode) => {
    if (filter !== 'disabled') {
      setItems((list) => [code, ...(list ?? []).filter((p) => p.id !== code.id)]);
    }
  };

  return { allowed, filter, setFilter, items, next, error, off, more, tariffs, busy, load, loadMore, switchCode, added };
}
