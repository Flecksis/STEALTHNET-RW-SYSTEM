import { useEffect, useState, useSyncExternalStore } from 'react';
import { api } from '../lib/api';
import { avatarColor, initials } from '../lib/format';

const BATCH = 200;
const RECHECK_MS = 20_000;

const known = new Map<number, string | null>();
const queued = new Set<number>();
const listeners = new Set<() => void>();
let timer: number | null = null;

function emit() {
  for (const listener of listeners) {
    listener();
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
        known.set(id, found[String(id)] ?? null);
      }
    } catch {
      for (const id of part) {
        known.set(id, null);
      }
    }
  }
  emit();
}

function want(id: number) {
  if (!(id > 0) || known.has(id) || queued.has(id)) {
    return;
  }
  queued.add(id);
  if (timer === null) {
    timer = window.setTimeout(() => void flush(), 30);
  }
}

function recheck(id: number) {
  if (known.get(id) === null) {
    known.delete(id);
    want(id);
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function usePhoto(id: number | null | undefined, retry = false): string | null {
  const key = id ?? 0;
  const version = useSyncExternalStore(subscribe, () => known.get(key) ?? null);
  useEffect(() => {
    want(key);
    if (!retry || !(key > 0)) {
      return;
    }
    const later = window.setTimeout(() => recheck(key), RECHECK_MS);
    return () => window.clearTimeout(later);
  }, [key, retry]);
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
  if (src && failed !== src) {
    return <img className={`${className} ava-img`} src={src} alt="" aria-hidden="true" decoding="async" onError={() => setFailed(src)} />;
  }
  return (
    <span className={className} aria-hidden="true" style={{ background: avatarColor(id ?? 0) }}>
      {initials(name)}
    </span>
  );
}
