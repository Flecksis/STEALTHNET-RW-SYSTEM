import { useSyncExternalStore } from 'react';
import { clock } from './text';

export type Caught = { at: number; text: string; times: number };

const LIMIT = 20;
const SHOWN = 5;
const FRAMES = 6;
const NOISE = [/ResizeObserver loop/i, /^Script error\.?$/i];

let caught: Caught[] = [];
const listeners = new Set<() => void>();

function frames(stack: string | undefined): string[] {
  return (stack ?? '')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => /^at\s/.test(line) || /^[^\s@]*@\S+:\d+:\d+$/.test(line))
    .slice(0, FRAMES);
}

function describe(value: unknown): string {
  if (value instanceof Error) {
    const code = 'code' in value && typeof value.code === 'string' ? ` [${value.code}]` : '';
    return [`${value.name}${code}: ${value.message}`, ...frames(value.stack).map((line) => `  ${line}`)].join('\n');
  }
  if (typeof value === 'string') {
    return value;
  }
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function remember(raw: string) {
  const origin = window.location.origin;
  const text = (origin ? raw.split(origin).join('') : raw).slice(0, 1500);
  if (!text.trim() || NOISE.some((pattern) => pattern.test(text))) {
    return;
  }
  const last = caught[caught.length - 1];
  caught =
    last && last.text === text
      ? [...caught.slice(0, -1), { ...last, at: Date.now(), times: last.times + 1 }]
      : [...caught, { at: Date.now(), text, times: 1 }].slice(-LIMIT);
  listeners.forEach((listener) => listener());
}

function onError(event: ErrorEvent) {
  if (event.error !== undefined && event.error !== null) {
    remember(describe(event.error));
    return;
  }
  const where = event.filename ? `\n  at ${event.filename}:${event.lineno}:${event.colno}` : '';
  remember(`${event.message}${where}`);
}

function onRejection(event: PromiseRejectionEvent) {
  remember(`Ошибка в промисе без обработчика: ${describe(event.reason)}`);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useCaught(): Caught[] {
  return useSyncExternalStore(subscribe, () => caught);
}

export function caughtLines(list: Caught[]): string[] {
  return list.slice(-SHOWN).map((entry) => `${clock(new Date(entry.at))}${entry.times > 1 ? ` (×${entry.times})` : ''} ${entry.text}`);
}

if (typeof window !== 'undefined') {
  window.addEventListener('error', onError);
  window.addEventListener('unhandledrejection', onRejection);
}
