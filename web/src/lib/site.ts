import { useEffect, useState } from 'react';
import { api } from './api';
import type { Site } from './types';

export const DEFAULT_SITE: Site = {
  brand_name: 'STEALTHNET',
  currency: 'USD',
  support_url: null,
  bot_username: null,
  appearance: { admin: 'side', admin_theme: 'dark', client: 'accent', client_theme: 'client' },
};

export function useSite(): Site | null {
  const [site, setSite] = useState<Site | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    api<Site>('/api/public/site', { signal: controller.signal })
      .then(setSite)
      .catch((err: unknown) => {
        if (!(err instanceof DOMException)) {
          setSite(DEFAULT_SITE);
        }
      });
    return () => controller.abort();
  }, []);
  return site;
}
