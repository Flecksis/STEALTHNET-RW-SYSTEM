import type { WebApp } from './telegram';

export const FULLSCREEN_FROM = '8.0';
export const SWIPES_FROM = '7.7';
export const ANSWER_MS = 900;
export const TRIES = 3;

const PHONES = ['ios', 'android', 'android_x'];

type Root = { dataset: Record<string, string | undefined> };

function attempt(call: () => void): boolean {
  try {
    call();
    return true;
  } catch {
    return false;
  }
}

function supports(app: WebApp, version: string): boolean {
  let yes = false;
  attempt(() => {
    yes = app.isVersionAtLeast?.(version) === true;
  });
  return yes;
}

export function takesFullscreen(app: WebApp): boolean {
  return PHONES.includes(app.platform ?? '') && supports(app, FULLSCREEN_FROM) && typeof app.requestFullscreen === 'function';
}

export function takesSwipes(app: WebApp): boolean {
  return supports(app, SWIPES_FROM) && typeof app.disableVerticalSwipes === 'function';
}

export function holdScreen(app: WebApp, root: Root): () => void {
  const full = takesFullscreen(app);
  const swipes = takesSwipes(app);
  let refused = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const stopWaiting = () => {
    clearTimeout(timer);
    timer = undefined;
  };
  const ask = (tries: number) => {
    stopWaiting();
    if (tries > 1) {
      timer = setTimeout(() => ask(tries - 1), ANSWER_MS);
    }
    if (!attempt(() => app.requestFullscreen?.())) {
      refused = true;
      stopWaiting();
    }
  };
  const holdSwipes = () => {
    if (swipes) {
      attempt(() => app.disableVerticalSwipes?.());
    }
  };
  const onFailed = (payload?: unknown) => {
    stopWaiting();
    refused = (payload as { error?: unknown } | null | undefined)?.error !== 'ALREADY_FULLSCREEN';
  };
  const onActivated = () => {
    holdSwipes();
    if (full && !refused && !app.isFullscreen) {
      ask(TRIES);
    }
  };
  const events: Array<[string, (payload?: unknown) => void]> = [
    ['fullscreenChanged', stopWaiting],
    ['fullscreenFailed', onFailed],
    ['activated', onActivated],
  ];

  if (!swipes) {
    root.dataset.tgSwipes = 'css';
  }
  for (const [name, cb] of events) {
    attempt(() => app.onEvent?.(name, cb));
  }
  attempt(() => app.ready());
  attempt(() => app.expand?.());
  holdSwipes();
  if (full) {
    ask(TRIES);
  }

  return () => {
    stopWaiting();
    for (const [name, cb] of events) {
      attempt(() => app.offEvent?.(name, cb));
    }
    delete root.dataset.tgSwipes;
  };
}
