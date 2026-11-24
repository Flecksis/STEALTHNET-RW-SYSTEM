import { lang } from './i18n';
import type { SiteDocument } from './types';

export function documentTitle(doc: SiteDocument): string {
  const english = doc.title_en?.trim();
  return lang() === 'en' && english ? english : doc.title;
}

export function documentUrl(doc: SiteDocument): string {
  if (!doc.page) {
    return doc.url;
  }
  try {
    return new URL(doc.url, window.location.origin).href;
  } catch {
    return doc.url;
  }
}
