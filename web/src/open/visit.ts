export type Visit = { token: string; holder: string };

export type VisitPage = {
  location: { hash: string; pathname: string };
  history: { state: unknown; replaceState: (state: unknown, title: string, url: string) => void };
  sessionStorage: Pick<Storage, 'getItem' | 'setItem'>;
};

export type StayPage = {
  document: Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'>;
  setTimeout: (run: () => void, ms: number) => number;
  clearTimeout: (timer: number) => void;
};

export const STAY_MS = 2000;
const SAVED = 'sn.open';
const TOKEN = /^[0-9a-f]{64}$/;
const HOLDER = /^[0-9a-f]{32}$/;

function saved(page: VisitPage): Visit | null {
  try {
    const visit = JSON.parse(page.sessionStorage.getItem(SAVED) ?? 'null') as Partial<Visit> | null;
    if (visit && TOKEN.test(visit.token ?? '') && HOLDER.test(visit.holder ?? '')) {
      return { token: visit.token as string, holder: visit.holder as string };
    }
  } catch {
    return null;
  }
  return null;
}

function newHolder(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function keep(page: VisitPage, visit: Visit): void {
  try {
    page.sessionStorage.setItem(SAVED, JSON.stringify(visit));
  } catch {
    return;
  }
}

export function takeVisit(page: VisitPage): Visit | null {
  const token = page.location.hash.slice(1);
  const before = saved(page);
  if (!TOKEN.test(token)) {
    return before;
  }
  const visit = before?.token === token ? before : { token, holder: newHolder() };
  keep(page, visit);
  page.history.replaceState(page.history.state, '', page.location.pathname);
  return visit;
}

export function watchStay(page: StayPage, onStay: () => void): () => void {
  let left = false;
  const look = () => {
    if (page.document.visibilityState === 'hidden') {
      left = true;
    } else if (left) {
      onStay();
    }
  };
  const timer = page.setTimeout(() => {
    if (page.document.visibilityState !== 'hidden') {
      onStay();
    }
  }, STAY_MS);
  page.document.addEventListener('visibilitychange', look);
  return () => {
    page.clearTimeout(timer);
    page.document.removeEventListener('visibilitychange', look);
  };
}
