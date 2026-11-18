import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useSearchParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { t } from '../lib/i18n';
import { useCabinet } from './CabinetApp';
import type { Device, SubscriptionView } from './model';
import { liveSubs, pickSub, readRemembered, remember, subName, subState, useShowPressed, type Remembered } from './subs';

export const CABINET_SUB_KEY = 'sn.cabinet.sub';

export type Subs = {
  list: SubscriptionView[] | undefined;
  chosen: SubscriptionView | null;
  error: string | null;
  choose: (id: number) => void;
  reload: () => Promise<SubscriptionView[]>;
  link: (path: string) => string;
};

export function useSubs(): Subs {
  const [params, setParams] = useSearchParams();
  const [list, setList] = useState<SubscriptionView[] | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [kept, setKept] = useState<Remembered | null>(() => readRemembered(CABINET_SUB_KEY));

  const reload = useCallback(async () => {
    try {
      const next = liveSubs(await api<SubscriptionView[]>('/api/client/subscriptions'));
      setList(next);
      setError(null);
      return next;
    } catch (err) {
      setError(err instanceof ApiError ? err.message : t('Подписка не загрузилась'));
      setList((prev) => prev ?? []);
      return [];
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const asked = Number(params.get('sub'));
  const fromUrl = list?.find((v) => v.subscription.id === asked) ?? null;
  const chosen = useMemo(() => (list ? (fromUrl ?? pickSub(list, kept)) : null), [list, fromUrl, kept]);

  useEffect(() => {
    if (fromUrl && kept !== fromUrl.subscription.id) {
      remember(CABINET_SUB_KEY, fromUrl.subscription.id);
      setKept(fromUrl.subscription.id);
    }
  }, [fromUrl, kept]);

  const choose = useCallback(
    (id: number) => {
      remember(CABINET_SUB_KEY, id);
      setKept(id);
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set('sub', String(id));
          return next;
        },
        { replace: true },
      );
    },
    [setParams],
  );

  const many = (list?.length ?? 0) > 1;
  const chosenId = chosen?.subscription.id ?? null;
  const link = useCallback(
    (path: string) => {
      if (!many || chosenId === null) {
        return path;
      }
      const at = path.indexOf('#');
      const base = at < 0 ? path : path.slice(0, at);
      const hash = at < 0 ? '' : path.slice(at);
      return `${base}${base.includes('?') ? '&' : '?'}sub=${chosenId}${hash}`;
    },
    [many, chosenId],
  );

  return { list, chosen, error, choose, reload, link };
}

type Detail = { view: SubscriptionView; devices: Device[] | null };

export type SubDetail = {
  view: SubscriptionView | null;
  devices: Device[] | null;
  ready: boolean;
  setView: (view: SubscriptionView) => void;
  setDevices: (devices: Device[]) => void;
};

export function useSubDetail(chosen: SubscriptionView | null): SubDetail {
  const cache = useRef(new Map<number, Detail>());
  const [, setTick] = useState(0);
  const id = chosen?.subscription.id ?? null;

  useEffect(() => {
    if (!chosen) {
      return;
    }
    let alive = true;
    const sid = chosen.subscription.id;
    void Promise.all([
      api<SubscriptionView>(`/api/client/subscriptions/${sid}`).catch(() => chosen),
      api<Device[]>(`/api/client/subscriptions/${sid}/devices`).catch(() => null),
    ]).then(([view, devices]) => {
      cache.current.set(sid, { view, devices });
      if (alive) {
        setTick((n) => n + 1);
      }
    });
    return () => {
      alive = false;
    };
  }, [chosen]);

  const put = useCallback(
    (patch: Partial<Detail>) => {
      if (id === null || !chosen) {
        return;
      }
      const prev = cache.current.get(id) ?? { view: chosen, devices: null };
      cache.current.set(id, { ...prev, ...patch });
      setTick((n) => n + 1);
    },
    [id, chosen],
  );

  const hit = id === null ? undefined : cache.current.get(id);
  return {
    view: hit?.view ?? chosen,
    devices: hit?.devices ?? null,
    ready: hit !== undefined,
    setView: (view) => put({ view }),
    setDevices: (devices) => put({ devices }),
  };
}

export function SubSwitch({ subs }: { subs: Subs }) {
  const { me } = useCabinet();
  const box = useRef<HTMLDivElement>(null);
  const list = subs.list ?? [];
  const chosenId = subs.chosen?.subscription.id ?? null;

  useShowPressed(box, `${chosenId}:${list.length}`);

  if (list.length < 2) {
    return null;
  }
  const blocked = me.client.status === 'blocked';
  return (
    <div ref={box} role="group" aria-label={t('Подписки')} className="cb-subs">
      {list.map((v) => {
        const on = v.subscription.id === chosenId;
        const state = subState(v, blocked);
        return (
          <button key={v.subscription.id} type="button" className="cb-subs-item" aria-pressed={on} onClick={() => !on && subs.choose(v.subscription.id)}>
            <strong>{subName(v, list)}</strong>
            <span className={`cb-subs-state ${state.tone}`}>{state.text}</span>
          </button>
        );
      })}
    </div>
  );
}
