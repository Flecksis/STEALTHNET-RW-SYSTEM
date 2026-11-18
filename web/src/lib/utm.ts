import { api } from './api';
import { savedTags, tagsFrom, type Utm } from './utmTags';

const KEY = 'sn_utm';
const VISIT_KEY = 'sn_utm_visit';

export function storedUtm(): Utm | null {
  try {
    return savedTags(window.localStorage.getItem(KEY), Date.now());
  } catch {
    return null;
  }
}

export function captureUtm(search: string = window.location.search) {
  const utm = tagsFrom(search);
  if (!utm || storedUtm()) {
    return;
  }
  try {
    window.localStorage.setItem(KEY, JSON.stringify({ ...utm, at: Date.now() }));
  } catch {
    return;
  }
}

export function reportVisit(search: string = window.location.search) {
  const utm = tagsFrom(search);
  if (!utm) {
    return;
  }
  try {
    if (window.sessionStorage.getItem(VISIT_KEY)) {
      return;
    }
    window.sessionStorage.setItem(VISIT_KEY, '1');
  } catch {
    return;
  }
  api('/api/public/m/utm/visit', { method: 'POST', body: utm, keepalive: true }).catch(() => undefined);
}
