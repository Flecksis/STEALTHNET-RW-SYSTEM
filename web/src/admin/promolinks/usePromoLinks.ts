import { useCallback, useEffect, useState } from 'react';
import { api } from '../../lib/api';
import { useSession } from '../session';
import { disabledModule, failText, ROOT, type Filter, type LinkPage, type PromoLink, type SquadBrief } from './model';

export function usePromoLinks() {
  const { can } = useSession();
  const allowed = can('marketing');
  const [filter, setFilter] = useState<Filter>('all');
  const [items, setItems] = useState<PromoLink[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [more, setMore] = useState(false);
  const [squads, setSquads] = useState<SquadBrief[] | null>(null);
  const [busy, setBusy] = useState<number | null>(null);

  const query = useCallback(
    (after: number | null) => {
      const params = new URLSearchParams({ limit: '50', status: filter });
      if (after !== null) {
        params.set('after', String(after));
      }
      return api<LinkPage>(`${ROOT}/links?${params.toString()}`);
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
      setError(failText(err, 'Промо-ссылки не загрузились'));
    }
  }, [allowed, query]);

  useEffect(() => {
    setItems(null);
    void load();
  }, [load]);

  useEffect(() => {
    if (!allowed) {
      return;
    }
    let alive = true;
    api<SquadBrief[]>('/api/admin/tariffs/squads')
      .then((list) => alive && setSquads(list))
      .catch(() => alive && setSquads([]));
    return () => {
      alive = false;
    };
  }, [allowed]);

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
      setError(failText(err, 'Промо-ссылки не загрузились'));
    } finally {
      setMore(false);
    }
  };

  const switchLink = async (link: PromoLink, enabled: boolean): Promise<string> => {
    setBusy(link.id);
    try {
      const saved = await api<PromoLink>(`${ROOT}/links/${link.id}/switch`, { method: 'POST', body: { enabled } });
      setItems((list) => list?.map((l) => (l.id === saved.id ? saved : l)) ?? list);
      return enabled ? `Ссылка «${saved.name}» снова работает` : `Ссылка «${saved.name}» выключена`;
    } finally {
      setBusy(null);
    }
  };

  const added = (link: PromoLink) => {
    if (filter !== 'off') {
      setItems((list) => [link, ...(list ?? []).filter((l) => l.id !== link.id)]);
    }
  };

  return { allowed, filter, setFilter, items, next, error, off, more, squads, busy, loadMore, switchLink, added };
}
