import type { InsetBox } from './insets';
import type { WebApp, WebAppListener } from './telegram';

export type Launch = Record<string, string>;

export type NativeProxy = { postEvent: (type: string, data: string) => void };

type Receive = (type: string, data?: unknown) => void;

export type BridgeHost = {
  Telegram?: { WebApp?: WebApp; WebView?: { receiveEvent: Receive } };
  TelegramGameProxy?: { receiveEvent: Receive };
  location?: { href: string };
};

type Store = Pick<Storage, 'getItem' | 'setItem'>;

const SAVED = '__telegram__initParams';

export function launchFrom(hash: string): Launch {
  const query = hash.replace(/^#/, '').replace(/^[^=?]*\?/, '');
  const launch: Launch = {};
  for (const [key, value] of new URLSearchParams(query)) {
    if (key.startsWith('tgWebApp')) {
      launch[key] = value;
    }
  }
  return launch;
}

export function saveLaunch(store: Store, launch: Launch): void {
  try {
    store.setItem(SAVED, JSON.stringify(launch));
  } catch {
    return;
  }
}

export function savedLaunch(store: Store): Launch {
  const launch: Launch = {};
  try {
    const saved: unknown = JSON.parse(store.getItem(SAVED) ?? 'null');
    if (saved && typeof saved === 'object') {
      for (const [key, value] of Object.entries(saved)) {
        if (key.startsWith('tgWebApp') && typeof value === 'string') {
          launch[key] = value;
        }
      }
    }
  } catch {
    return {};
  }
  return launch;
}

export function versionAtLeast(version: string, wanted: string): boolean {
  const have = version.split('.');
  const need = wanted.split('.');
  for (let i = 0; i < Math.max(have.length, need.length); i += 1) {
    const diff = (Number.parseInt(have[i] ?? '', 10) || 0) - (Number.parseInt(need[i] ?? '', 10) || 0);
    if (diff !== 0) {
      return diff > 0;
    }
  }
  return true;
}

function schemeOf(themeParams: string | undefined): 'dark' | 'light' | undefined {
  try {
    const color: unknown = (JSON.parse(themeParams ?? 'null') as { bg_color?: unknown } | null)?.bg_color;
    const hex = typeof color === 'string' ? /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color) : null;
    if (!hex) {
      return undefined;
    }
    const [r = 0, g = 0, b = 0] = hex.slice(1).map((part) => Number.parseInt(part, 16));
    return Math.sqrt(0.299 * r * r + 0.587 * g * g + 0.114 * b * b) < 120 ? 'dark' : 'light';
  } catch {
    return undefined;
  }
}

function fields(data: unknown): Record<string, unknown> {
  return data && typeof data === 'object' ? (data as Record<string, unknown>) : {};
}

export function bridgeApp(launch: Launch, proxy: NativeProxy, host: BridgeHost): WebApp {
  const atLeast = (wanted: string) => versionAtLeast(launch.tgWebAppVersion || '6.0', wanted);
  const post = (type: string, data: unknown = '') => proxy.postEvent(type, JSON.stringify(data));
  const listeners = new Map<string, Set<WebAppListener>>();
  const on = (name: string, cb: WebAppListener) => {
    listeners.set(name, (listeners.get(name) ?? new Set()).add(cb));
  };
  const off = (name: string, cb: WebAppListener) => {
    listeners.get(name)?.delete(cb);
  };
  const emit = (name: string, payload?: unknown) => {
    for (const cb of [...(listeners.get(name) ?? [])]) {
      try {
        cb(payload);
      } catch {
        continue;
      }
    }
  };
  const button = (visible: boolean) => {
    if (atLeast('6.1')) {
      post('web_app_setup_back_button', { is_visible: visible });
    }
  };
  const webLink = (url: string, protocols: string[]) => {
    const link = new URL(url, host.location?.href);
    if (!protocols.includes(link.protocol)) {
      throw new Error('WebAppTgUrlInvalid');
    }
    return link;
  };
  const safe: InsetBox = { top: 0, right: 0, bottom: 0, left: 0 };
  const content: InsetBox = { top: 0, right: 0, bottom: 0, left: 0 };

  const app: WebApp = {
    initData: launch.tgWebAppData ?? '',
    platform: launch.tgWebAppPlatform ?? 'unknown',
    colorScheme: schemeOf(launch.tgWebAppThemeParams),
    isFullscreen: Boolean(launch.tgWebAppFullscreen),
    safeAreaInset: safe,
    contentSafeAreaInset: content,
    isVersionAtLeast: atLeast,
    setHeaderColor: (color) => post('web_app_set_header_color', { color }),
    setBackgroundColor: (color) => post('web_app_set_background_color', { color }),
    ready: () => post('web_app_ready'),
    expand: () => post('web_app_expand'),
    requestFullscreen: () => post('web_app_request_fullscreen'),
    disableVerticalSwipes: () => post('web_app_setup_swipe_behavior', { allow_vertical_swipe: false }),
    close: () => post('web_app_close', {}),
    openLink: (url) => post('web_app_open_link', { url: webLink(url, ['https:', 'http:']).href }),
    openTelegramLink: (url) => {
      const link = webLink(url, ['https:', 'http:']);
      post('web_app_open_tg_link', { path_full: link.pathname + link.search });
    },
    downloadFile: (params) => {
      if (!atLeast('8.0')) {
        throw new Error('WebAppMethodUnsupported');
      }
      post('web_app_request_file_download', { url: webLink(params.url, ['https:']).href, file_name: params.file_name });
    },
    onEvent: on,
    offEvent: off,
    BackButton: {
      show: () => button(true),
      hide: () => button(false),
      onClick: (cb) => on('backButtonClicked', cb),
      offClick: (cb) => off('backButtonClicked', cb),
    },
  };

  const receive: Receive = (type, data) => {
    const body = fields(data);
    switch (type) {
      case 'safe_area_changed':
        Object.assign(safe, body);
        emit('safeAreaChanged');
        break;
      case 'content_safe_area_changed':
        Object.assign(content, body);
        emit('contentSafeAreaChanged');
        break;
      case 'fullscreen_changed':
        app.isFullscreen = body.is_fullscreen === true;
        emit('fullscreenChanged');
        break;
      case 'fullscreen_failed':
        app.isFullscreen = app.isFullscreen || body.error === 'ALREADY_FULLSCREEN';
        emit('fullscreenFailed', { error: body.error });
        break;
      case 'visibility_changed':
        emit(body.is_visible === true ? 'activated' : 'deactivated');
        break;
      case 'back_button_pressed':
        emit('backButtonClicked');
        break;
      default:
        break;
    }
  };

  host.Telegram = { ...host.Telegram, WebView: { receiveEvent: receive } };
  host.TelegramGameProxy = { receiveEvent: receive };
  if (atLeast('8.0')) {
    post('web_app_request_safe_area');
    post('web_app_request_content_safe_area');
  }
  return app;
}
