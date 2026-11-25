import { useEffect, useState } from 'react';
import { api } from './api';
import { servedBrand, tabTitle } from './tabTitle';
import type { Site } from './types';

export const DEFAULT_SITE: Site = {
  brand_name: servedBrand(typeof document === 'undefined' ? null : document),
  currency: 'USD',
  support_url: null,
  bot_username: null,
  appearance: { admin: 'side', admin_theme: 'dark', client: 'accent', client_theme: 'client' },
};

export function showTitle(site: Site | null, make: (site: Site) => string): void {
  const title = tabTitle(site, DEFAULT_SITE, make);
  if (title !== null) {
    document.title = title;
  }
}

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
