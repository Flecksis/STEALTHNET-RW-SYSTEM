import { useEffect, useState, useSyncExternalStore } from 'react';
import { readStored, writeStored } from './theme';
import type { Site } from './types';

export type Lang = 'ru' | 'en';
type Dict = Record<string, string>;
type Vars = Record<string, string | number>;

const STORE_KEY = 'sn.lang';
const COOKIE = 'sn_lang';
const RUSSIAN_LIKE = ['ru', 'uk', 'be', 'kk'];

let current: Lang = 'ru';
let english: Dict | null = null;
let plurals: Record<string, [string, string]> = {};
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) {
    listener();
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function lang(): Lang {
  return current;
}

export function locale(): string {
  return current === 'en' ? 'en-GB' : 'ru-RU';
}

function fill(text: string, vars?: Vars): string {
  if (!vars) {
    return text;
  }
  return text.replace(/\{([a-z0-9_]+)\}/gi, (whole, name: string) => (name in vars ? String(vars[name]) : whole));
}

export function msg(ru: string): string {
  return ru;
}

export function t(ru: string, vars?: Vars): string {
  const text = current === 'en' && english ? (english[ru] ?? ru) : ru;
  return fill(text, vars);
}

export function pluralEn(one: string): [string, string] | null {
  return current === 'en' ? (plurals[one] ?? null) : null;
}

async function ensureEnglish(): Promise<void> {
  if (english) {
    return;
  }
  const module = await import('./i18n/en');
  english = module.EN;
  plurals = module.PLURALS;
}

export async function applyLang(next: Lang): Promise<void> {
  if (next === 'en') {
    await ensureEnglish();
  }
  if (current !== next) {
    current = next;
    notify();
  }
  if (typeof document !== 'undefined') {
    document.documentElement.lang = next;
  }
}

export function useLang(): Lang {
  return useSyncExternalStore(subscribe, lang, lang);
}

export function siteLangs(site: Site | null): Lang[] {
  const list = site?.languages?.list ?? ['ru'];
  return list.filter((l): l is Lang => l === 'ru' || l === 'en');
}

function fromTag(tag: string | undefined | null): Lang | null {
  const base = (tag ?? '').trim().split(/[-_]/)[0]?.toLowerCase() ?? '';
  if (!/^[a-z]{2,3}$/.test(base)) {
    return null;
  }
  return RUSSIAN_LIKE.includes(base) ? 'ru' : 'en';
}

export function storedLang(): Lang | null {
  const value = readStored(STORE_KEY);
  return value === 'ru' || value === 'en' ? value : null;
}

export function rememberLang(value: Lang): void {
  writeStored(STORE_KEY, value);
}

export function guessLang(site: Site | null, hint?: string | null): Lang {
  const allowed = siteLangs(site);
  if (!allowed.includes('en')) {
    return 'ru';
  }
  const fallback: Lang = site?.languages?.default === 'en' ? 'en' : 'ru';
  const query = new URLSearchParams(window.location.search).get('lang');
  if (query === 'ru' || query === 'en') {
    return query;
  }
  const stored = storedLang();
  if (stored) {
    return stored;
  }
  if (hint) {
    return fromTag(hint) ?? fallback;
  }
  const browser = typeof navigator === 'undefined' ? [] : navigator.languages?.length ? navigator.languages : [navigator.language];
  for (const tag of browser) {
    const found = fromTag(tag);
    if (found) {
      return found;
    }
  }
  return fallback;
}

export function otherLang(current: Lang, list: Lang[]): Lang | null {
  return list.find((l) => l !== current) ?? null;
}

export const NATIVE: Record<Lang, string> = { ru: 'Русский', en: 'English' };

function rememberCookie(value: Lang): void {
  try {
    document.cookie = `${COOKIE}=${value}; path=/; max-age=31536000; samesite=lax`;
  } catch {
    return;
  }
}

export function useApplied(wanted: Lang | null): boolean {
  const active = useLang();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!wanted) {
      return;
    }
    let alive = true;
    rememberCookie(wanted);
    applyLang(wanted).catch(() => {
      if (alive) {
        setFailed(true);
      }
    });
    return () => {
      alive = false;
    };
  }, [wanted]);
  return wanted !== null && (active === wanted || failed);
}
