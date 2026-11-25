import type { WebApp } from './telegram';

export const FULLSCREEN_FROM = '8.0';
export const SWIPES_FROM = '7.7';
export const ANSWER_MS = 900;
export const TRIES = 3;

const PHONES = ['ios', 'android', 'android_x'];

type Root = { dataset: Record<string, string | undefined> };

type Events = Array<[string, (payload?: unknown) => void]>;

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

function listen(app: WebApp, events: Events): () => void {
  for (const [name, cb] of events) {
    attempt(() => app.onEvent?.(name, cb));
  }
  return () => {
    for (const [name, cb] of events) {
      attempt(() => app.offEvent?.(name, cb));
    }
  };
}

export function fullscreenOn(site: { client_access?: { miniapp_fullscreen?: boolean } | null } | null | undefined): boolean {
  return site?.client_access?.miniapp_fullscreen === true;
}

export function takesFullscreen(app: WebApp): boolean {
  return PHONES.includes(app.platform ?? '') && supports(app, FULLSCREEN_FROM) && typeof app.requestFullscreen === 'function';
}

export function takesSwipes(app: WebApp): boolean {
  return supports(app, SWIPES_FROM) && typeof app.disableVerticalSwipes === 'function';
}

export function holdScreen(app: WebApp, root: Root): () => void {
  const swipes = takesSwipes(app);
  const holdSwipes = () => {
    if (swipes) {
      attempt(() => app.disableVerticalSwipes?.());
    }
  };
  if (!swipes) {
    root.dataset.tgSwipes = 'css';
  }
  const stop = listen(app, [['activated', holdSwipes]]);
  attempt(() => app.ready());
  attempt(() => app.expand?.());
  holdSwipes();
  return () => {
    stop();
    delete root.dataset.tgSwipes;
  };
}

export function holdFullscreen(app: WebApp): () => void {
  if (!takesFullscreen(app)) {
    return () => undefined;
  }
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
  const onFailed = (payload?: unknown) => {
    stopWaiting();
    refused = (payload as { error?: unknown } | null | undefined)?.error !== 'ALREADY_FULLSCREEN';
  };
  const onActivated = () => {
    if (!refused && !app.isFullscreen) {
      ask(TRIES);
    }
  };

  const stop = listen(app, [
    ['fullscreenChanged', stopWaiting],
    ['fullscreenFailed', onFailed],
    ['activated', onActivated],
  ]);
  ask(TRIES);
  return () => {
    stopWaiting();
    stop();
  };
}
