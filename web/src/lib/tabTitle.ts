export type Named = { brand_name: string; title?: string | null };

type Page = {
  querySelector(selectors: string): { getAttribute(name: string): string | null } | null;
  documentElement?: { getAttribute(name: string): string | null } | null;
};

export type ServedLanguages = { list: Array<'ru' | 'en'>; default: 'ru' | 'en' };

export function servedBrand(page: Page | null): string {
  return page?.querySelector('meta[name="application-name"]')?.getAttribute('content')?.trim() ?? '';
}

export function servedLanguages(page: Page | null): ServedLanguages | undefined {
  return page?.documentElement?.getAttribute('lang') === 'en' ? { list: ['ru', 'en'], default: 'en' } : undefined;
}

export function tabTitle<S extends Named>(site: S | null, spare: S, make: (site: S) => string): string | null {
  if (!site || site === spare) {
    return null;
  }
  return make(site) || null;
}
