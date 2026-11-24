import { useEffect, useRef, useState } from 'react';
import type { Lang } from '../../lib/i18n';
import type { Theme } from '../../lib/theme';
import { AUDIENCES, isPart, MINI_PREVIEW, type Audience, type MiniConfig, type MiniDraft, type Part, type PreviewScreen, type SectionKey } from '../../mini/config';
import { canonical, STATE_NAMES } from './model';

const PATH = '/admin/mini-preview';
const WIDTH = 390;
const TALL = 660;
const SMALLEST = 0.7;

export type Focus = { part: Part; n: number };

export function useSize(node: HTMLElement | null): { width: number; height: number } {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    if (!node) {
      return;
    }
    const measure = () => setSize((s) => (s.width === node.clientWidth && s.height === node.clientHeight ? s : { width: node.clientWidth, height: node.clientHeight }));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, [node]);
  return size;
}

export function useViewportHeight(): number {
  const [height, setHeight] = useState(() => window.innerHeight);
  useEffect(() => {
    const onResize = () => setHeight(window.innerHeight);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);
  return height;
}

type FrameProps = {
  config: MiniConfig;
  theme: Theme;
  lang: Lang;
  state: Audience;
  screen: PreviewScreen;
  off: SectionKey[];
  percent: number;
  focus: Focus | null;
  height: number;
  onPick: (part: Part) => void;
};

export function PreviewFrame({ config, theme, lang, state, screen, off, percent, focus, height, onPick }: FrameProps) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const { width } = useSize(box);
  const frame = useRef<HTMLIFrameElement>(null);
  const [ready, setReady] = useState(0);
  const sent = useRef(0);
  const pick = useRef(onPick);

  useEffect(() => {
    pick.current = onPick;
  }, [onPick]);

  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== window.location.origin || !frame.current || event.source !== frame.current.contentWindow) {
        return;
      }
      const data = event.data as { type?: unknown; part?: unknown } | null;
      if (data?.type === MINI_PREVIEW.ready) {
        setReady((n) => n + 1);
      } else if (data?.type === MINI_PREVIEW.pick && isPart(data.part)) {
        pick.current(data.part);
      }
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, []);

  useEffect(() => {
    if (!ready) {
      return;
    }
    const timer = window.setTimeout(() => {
      const target = frame.current?.contentWindow;
      if (!target) {
        return;
      }
      const jump = focus && focus.n !== sent.current ? focus.part : null;
      if (focus) {
        sent.current = focus.n;
      }
      const message: MiniDraft = { type: MINI_PREVIEW.draft, config: withImages(config), theme, lang, state, screen, off, percent, focus: jump };
      target.postMessage(message, window.location.origin);
    }, 120);
    return () => window.clearTimeout(timer);
  }, [ready, config, theme, lang, state, screen, off, percent, focus]);

  const scale = width > 0 ? Math.min(1, width / WIDTH, Math.max(SMALLEST, height / TALL)) : 0;
  return (
    <div ref={setBox} className="mce-frame" style={{ height }}>
      {scale > 0 ? (
        <div className="mce-frame-in" style={{ width: Math.floor(WIDTH * scale), height }}>
          <iframe ref={frame} src={PATH} title="Превью мини-приложения" style={{ width: WIDTH, height: Math.ceil(height / scale), transform: `scale(${scale})` }} />
        </div>
      ) : null}
    </div>
  );
}

function withImages(config: MiniConfig): MiniConfig {
  const plain = canonical(config);
  return {
    ...plain,
    home: plain.home.map((block) => {
      const source = config.home.find((other) => other.kind === block.kind);
      return block.kind === 'banners' && source?.kind === 'banners'
        ? { ...block, items: (block.items ?? []).map((banner, index) => ({ ...banner, image: source.items?.[index]?.image ?? undefined, image_en: source.items?.[index]?.image_en ?? undefined })) }
        : block;
    }),
  };
}

export function StateSegs({ state, onState }: { state: Audience; onState: (state: Audience) => void }) {
  return (
    <div role="group" aria-label="Какого клиента показать" className="segs">
      {AUDIENCES.map((key) => (
        <button key={key} type="button" aria-pressed={state === key} onClick={() => onState(key)}>
          {STATE_NAMES[key]}
        </button>
      ))}
    </div>
  );
}

export function ThemeSegs({ theme, onTheme }: { theme: Theme; onTheme: (theme: Theme) => void }) {
  return (
    <div role="group" aria-label="Тема превью" className="segs">
      <button type="button" aria-pressed={theme === 'light'} onClick={() => onTheme('light')}>
        Светлая
      </button>
      <button type="button" aria-pressed={theme === 'dark'} onClick={() => onTheme('dark')}>
        Тёмная
      </button>
    </div>
  );
}
