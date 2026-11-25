const FIXED: Array<[string, string]> = [
  ['minimum-scale', '1'],
  ['maximum-scale', '1'],
  ['user-scalable', 'no'],
];

const GESTURES = ['gesturestart', 'gesturechange', 'gestureend'] as const;

type Meta = { getAttribute: (name: string) => string | null; setAttribute: (name: string, value: string) => void };

type Listen = (type: string, cb: (event: Event) => void, options?: AddEventListenerOptions) => void;

export type ZoomPage = {
  documentElement: { dataset: Record<string, string | undefined> };
  querySelector: (selector: string) => unknown;
  addEventListener: Listen;
  removeEventListener: Listen;
};

export function fixedViewport(content: string): string {
  const parts = new Map<string, string>();
  for (const part of content.split(',')) {
    const [name = '', value = ''] = part.split('=').map((piece) => piece.trim());
    if (name) {
      parts.set(name.toLowerCase(), value);
    }
  }
  for (const [name, value] of FIXED) {
    parts.set(name, value);
  }
  return [...parts].map(([name, value]) => `${name}=${value}`).join(', ');
}

export function lockZoom(page: ZoomPage, framed: boolean): () => void {
  const meta = page.querySelector('meta[name="viewport"]') as Meta | null;
  const before = meta?.getAttribute('content') ?? null;
  const block = (event: Event) => {
    if (event.cancelable) {
      event.preventDefault();
    }
  };
  const blockPinch = (event: Event) => {
    if ((event as WheelEvent).ctrlKey) {
      block(event);
    }
  };
  if (meta && before !== null) {
    meta.setAttribute('content', fixedViewport(before));
  }
  page.documentElement.dataset.tgZoom = 'fixed';
  for (const name of GESTURES) {
    page.addEventListener(name, block, { passive: false });
  }
  if (!framed) {
    page.addEventListener('wheel', blockPinch, { passive: false });
  }
  return () => {
    if (meta && before !== null) {
      meta.setAttribute('content', before);
    }
    delete page.documentElement.dataset.tgZoom;
    for (const name of GESTURES) {
      page.removeEventListener(name, block);
    }
    if (!framed) {
      page.removeEventListener('wheel', blockPinch);
    }
  };
}
