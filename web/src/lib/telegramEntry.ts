type Place = { pathname: string; search: string; hash: string };

type Page = { querySelector(selectors: string): { getAttribute(name: string): string | null } | null };

const SITE = /^\/(?:index\.html)?$|^\/cabinet(?:\/|$)/;

export function servedApp(page: Page | null): string | null {
  const path = page?.querySelector('meta[name="telegram-app"]')?.getAttribute('content')?.trim() ?? '';
  return /^\/[a-z]/.test(path) ? path : null;
}

export function telegramEntry(place: Place, app: string | null): string | null {
  if (!app || !SITE.test(place.pathname)) {
    return null;
  }
  const launch = new URLSearchParams(place.hash.replace(/^#/, '').replace(/^[^=?]*\?/, ''));
  return launch.get('tgWebAppData') ? `${app}${place.search}${place.hash}` : null;
}
