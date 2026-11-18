import { useEffect, useState } from 'react';
import { api } from '../lib/api';
import { lang } from '../lib/i18n';

export type ConnectPlatform = 'ios' | 'android' | 'windows' | 'macos' | 'linux' | 'android_tv' | 'apple_tv';
export type Words = { ru: string; en: string };
export type LinkButton = { label: Words; url: string };
export type Step =
  | { kind: 'install'; text: Words; buttons: LinkButton[] }
  | { kind: 'add'; text: Words; link: string | null }
  | { kind: 'connect'; text: Words }
  | { kind: 'note'; title: Words; text: Words; buttons: LinkButton[] };
export type ClientApp = { id: string; name: string; icon: string | null; steps: Step[] };
export type ClientApps = Partial<Record<ConnectPlatform, ClientApp[]>>;
export type Group = 'ios' | 'android' | 'pc' | 'tv';

export const ALL_PLATFORMS: ConnectPlatform[] = ['ios', 'android', 'windows', 'macos', 'linux', 'android_tv', 'apple_tv'];

export const PLATFORM_NAME: Record<ConnectPlatform, string> = {
  ios: 'iPhone',
  android: 'Android',
  windows: 'Windows',
  macos: 'macOS',
  linux: 'Linux',
  android_tv: 'Android TV',
  apple_tv: 'Apple TV',
};

export const GROUP_OF: Record<ConnectPlatform, Group> = {
  ios: 'ios',
  android: 'android',
  windows: 'pc',
  macos: 'pc',
  linux: 'pc',
  android_tv: 'tv',
  apple_tv: 'tv',
};

export function local(words: Words | undefined): string {
  if (!words) {
    return '';
  }
  return lang() === 'en' ? words.en || words.ru : words.ru || words.en;
}

export function fillLink(link: string, url: string, brand: string): string {
  return link.split('{url}').join(url).split('{name}').join(encodeURIComponent(brand));
}

export function isWebLink(link: string): boolean {
  return /^https:\/\//i.test(link);
}

export function guessConnectPlatform(): ConnectPlatform {
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent;
  if (/iPhone|iPad|iPod/i.test(ua)) {
    return 'ios';
  }
  if (/Android/i.test(ua)) {
    return /\bTV\b|AFT|BRAVIA|SmartTV/i.test(ua) ? 'android_tv' : 'android';
  }
  if (/AppleTV|tvOS/i.test(ua)) {
    return 'apple_tv';
  }
  if (/Mac OS X|Macintosh/i.test(ua)) {
    return 'macos';
  }
  if (/Linux|X11/i.test(ua)) {
    return 'linux';
  }
  return 'windows';
}

export function shownPlatforms(apps: ClientApps): ConnectPlatform[] {
  return ALL_PLATFORMS.filter((platform) => (apps[platform]?.length ?? 0) > 0);
}

export function firstIn(apps: ClientApps, group: Group, prefer: ConnectPlatform): ConnectPlatform | null {
  const list = shownPlatforms(apps).filter((platform) => GROUP_OF[platform] === group);
  if (list.includes(prefer)) {
    return prefer;
  }
  return list[0] ?? null;
}

export function startPlatform(apps: ClientApps, phone: boolean): ConnectPlatform | null {
  const shown = shownPlatforms(apps);
  const guess = guessConnectPlatform();
  if (shown.includes(guess)) {
    return guess;
  }
  const group = phone && (guess === 'ios' || guess === 'android') ? guess : GROUP_OF[guess];
  return firstIn(apps, group, guess) ?? shown[0] ?? null;
}

let cached: ClientApps | null = null;
let pending: Promise<ClientApps> | null = null;

function load(): Promise<ClientApps> {
  if (cached) {
    return Promise.resolve(cached);
  }
  if (!pending) {
    pending = api<{ version: string; apps: ClientApps }>('/api/public/connect-apps')
      .then((body) => {
        cached = body.apps ?? {};
        return cached;
      })
      .finally(() => {
        pending = null;
      });
  }
  return pending;
}

export function useConnectApps(enabled: boolean): ClientApps | null | undefined {
  const [apps, setApps] = useState<ClientApps | null | undefined>(cached ?? undefined);
  useEffect(() => {
    if (!enabled || cached) {
      return;
    }
    let alive = true;
    load()
      .then((loaded) => alive && setApps(loaded))
      .catch(() => alive && setApps(null));
    return () => {
      alive = false;
    };
  }, [enabled]);
  return apps;
}

export type Picked = {
  platform: ConnectPlatform | null;
  group: Group | null;
  platforms: ConnectPlatform[];
  apps: ClientApp[];
  app: ClientApp | null;
  setGroup: (group: Group) => void;
  setPlatform: (platform: ConnectPlatform) => void;
  setApp: (id: string) => void;
};

export function usePicked(apps: ClientApps | null | undefined, phone: boolean): Picked {
  const [platform, setPlatformState] = useState<ConnectPlatform | null>(null);
  const [chosen, setChosen] = useState<Partial<Record<ConnectPlatform, string>>>({});
  const shown = apps ? shownPlatforms(apps) : [];
  const current = platform && shown.includes(platform) ? platform : apps ? startPlatform(apps, phone) : null;
  const list = current && apps ? (apps[current] ?? []) : [];
  const app = list.find((item) => item.id === (current ? chosen[current] : undefined)) ?? list[0] ?? null;
  return {
    platform: current,
    group: current ? GROUP_OF[current] : null,
    platforms: shown,
    apps: list,
    app,
    setGroup: (group) => {
      if (!apps) {
        return;
      }
      const next = firstIn(apps, group, current && GROUP_OF[current] === group ? current : guessConnectPlatform());
      if (next) {
        setPlatformState(next);
      }
    },
    setPlatform: (next) => setPlatformState(next),
    setApp: (id) => current && setChosen({ ...chosen, [current]: id }),
  };
}
