import { useEffect, useState, useSyncExternalStore } from 'react';
import { api } from '../lib/api';
import { avatarColor, initials } from '../lib/format';

const BATCH = 200;
const RECHECK_MS = 20_000;
const STORE_KEY = 'sn.admin.photos';
const STORE_MAX = 3000;

type Entry = { version: string | null; fresh: boolean };

const known = new Map<number, Entry>();
const queued = new Set<number>();
const listeners = new Set<() => void>();
let timer: number | null = null;
let saveTimer: number | null = null;

function load() {
  try {
    const raw = window.localStorage.getItem(STORE_KEY);
    if (!raw) {
      return;
    }
    const saved = JSON.parse(raw) as Record<string, unknown>;
    for (const [key, value] of Object.entries(saved)) {
      const id = Number(key);
      if (id > 0 && (typeof value === 'string' || value === null)) {
        known.set(id, { version: value, fresh: false });
      }
    }
  } catch {
    known.clear();
  }
}

function save() {
  saveTimer = null;
  try {
    const entries = [...known.entries()].slice(-STORE_MAX);
    window.localStorage.setItem(STORE_KEY, JSON.stringify(Object.fromEntries(entries.map(([id, e]) => [id, e.version]))));
  } catch {
    return;
  }
}

load();

function emit() {
  for (const listener of listeners) {
    listener();
  }
  if (saveTimer === null) {
    saveTimer = window.setTimeout(save, 500);
  }
}

async function flush() {
  timer = null;
  const ids = [...queued];
  queued.clear();
  for (let start = 0; start < ids.length; start += BATCH) {
    const part = ids.slice(start, start + BATCH);
    try {
      const found = await api<Record<string, string>>(`/api/admin/clients/photos?ids=${part.join(',')}`);
      for (const id of part) {
        known.delete(id);
        known.set(id, { version: found[String(id)] ?? null, fresh: true });
      }
    } catch {
      for (const id of part) {
        const old = known.get(id);
        known.set(id, { version: old?.version ?? null, fresh: true });
      }
    }
  }
  emit();
}

function want(id: number) {
  if (!(id > 0) || known.get(id)?.fresh || queued.has(id)) {
    return;
  }
  queued.add(id);
  if (timer === null) {
    timer = window.setTimeout(() => void flush(), 30);
  }
}

function recheck(id: number) {
  const entry = known.get(id);
  if (entry && entry.version === null) {
    known.set(id, { version: null, fresh: false });
    want(id);
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function usePhoto(id: number | null | undefined, retry = false): string | null | undefined {
  const key = id ?? 0;
  const version = useSyncExternalStore(subscribe, () => (key > 0 ? known.get(key)?.version : null));
  useEffect(() => {
    want(key);
    if (!retry || !(key > 0)) {
      return;
    }
    const later = window.setTimeout(() => recheck(key), RECHECK_MS);
    return () => window.clearTimeout(later);
  }, [key, retry]);
  if (version === undefined) {
    return undefined;
  }
  return version ? `/api/admin/clients/${key}/photo?v=${encodeURIComponent(version)}` : null;
}

type Props = {
  id: number | null | undefined;
  name: string;
  className: string;
  retry?: boolean;
};

export function Avatar({ id, name, className, retry = false }: Props) {
  const src = usePhoto(id, retry);
  const [failed, setFailed] = useState<string | null>(null);
  if (src === undefined) {
    return <span className={`${className} ava-wait`} aria-hidden="true" />;
  }
  if (src && failed !== src) {
    return <img className={`${className} ava-img`} src={src} alt="" aria-hidden="true" decoding="async" onError={() => setFailed(src)} />;
  }
  return (
    <span className={className} aria-hidden="true" style={{ background: avatarColor(id ?? 0) }}>
      {initials(name)}
    </span>
  );
}
