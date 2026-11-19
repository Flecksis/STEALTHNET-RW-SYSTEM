const KEY = 'sn.chunk-reload';
export const RELOAD_PAUSE_MS = 30_000;

export type Memory = {
  read(): string | null;
  write(value: string): boolean;
};

export function shouldReload(now: number, memory: Memory): boolean {
  const last = Number(memory.read() ?? 0) || 0;
  if (now - last < RELOAD_PAUSE_MS) {
    return false;
  }
  return memory.write(String(now));
}

const session: Memory = {
  read() {
    try {
      return window.sessionStorage.getItem(KEY);
    } catch {
      return null;
    }
  },
  write(value) {
    try {
      window.sessionStorage.setItem(KEY, value);
      return true;
    } catch {
      return false;
    }
  },
};

export function reloadOnStaleChunks(): void {
  window.addEventListener('vite:preloadError', (event) => {
    if (!shouldReload(Date.now(), session)) {
      return;
    }
    event.preventDefault();
    window.location.reload();
  });
}
