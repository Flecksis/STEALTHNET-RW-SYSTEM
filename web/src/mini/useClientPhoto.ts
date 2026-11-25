import { useEffect, useState } from 'react';
import { fetchBlob } from '../lib/api';

let kept: { path: string; url: string } | null = null;

function keep(path: string, photo: Blob): string {
  if (kept) {
    URL.revokeObjectURL(kept.url);
  }
  kept = { path, url: URL.createObjectURL(photo) };
  return kept.url;
}

export function useClientPhoto(path: string | null): string | null {
  const [loaded, setLoaded] = useState(kept);

  useEffect(() => {
    if (!path || kept?.path === path) {
      setLoaded(kept);
      return;
    }
    let alive = true;
    fetchBlob(path)
      .then((photo) => {
        const url = kept?.path === path ? kept.url : keep(path, photo);
        if (alive) {
          setLoaded({ path, url });
        }
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [path]);

  return path && loaded?.path === path ? loaded.url : null;
}
