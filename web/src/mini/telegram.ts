type WebApp = {
  initData: string;
  colorScheme?: 'dark' | 'light';
  ready: () => void;
  expand?: () => void;
  openLink?: (url: string) => void;
  openTelegramLink?: (url: string) => void;
  onEvent?: (name: string, cb: () => void) => void;
  offEvent?: (name: string, cb: () => void) => void;
};

declare global {
  interface Window {
    Telegram?: { WebApp?: WebApp };
  }
}

const SDK = 'https://telegram.org/js/telegram-web-app.js';

export function webApp(): WebApp | null {
  return window.Telegram?.WebApp ?? null;
}

function fromHash(): string {
  const hash = window.location.hash.replace(/^#/, '');
  const params = new URLSearchParams(hash);
  return params.get('tgWebAppData') ?? '';
}

export function loadTelegram(): Promise<WebApp | null> {
  if (webApp()) {
    return Promise.resolve(webApp());
  }
  if (!fromHash()) {
    return Promise.resolve(null);
  }
  return new Promise((resolve) => {
    const script = document.createElement('script');
    script.src = SDK;
    script.async = true;
    const done = () => resolve(webApp());
    script.onload = done;
    script.onerror = done;
    window.setTimeout(done, 4000);
    document.head.appendChild(script);
  });
}

export function initData(): string {
  return webApp()?.initData || fromHash();
}

export function openExternal(url: string): void {
  const app = webApp();
  if (url.startsWith('https://t.me/') && app?.openTelegramLink) {
    app.openTelegramLink(url);
    return;
  }
  if (app?.openLink) {
    app.openLink(url);
    return;
  }
  window.location.href = url;
}
