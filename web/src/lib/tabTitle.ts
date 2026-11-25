export type Named = { brand_name: string; title?: string | null };

type Page = { querySelector(selectors: string): { getAttribute(name: string): string | null } | null };

export function servedBrand(page: Page | null): string {
  return page?.querySelector('meta[name="application-name"]')?.getAttribute('content')?.trim() ?? '';
}

export function tabTitle<S extends Named>(site: S | null, spare: S, make: (site: S) => string): string | null {
  if (!site || site === spare) {
    return null;
  }
  return make(site) || null;
}
