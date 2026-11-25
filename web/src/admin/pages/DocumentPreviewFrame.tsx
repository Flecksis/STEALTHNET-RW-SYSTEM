import { useEffect, useState } from 'react';
import type { Lang } from '../../lib/i18n';
import type { Theme } from '../../lib/theme';
import { PreviewFail, usePreviewFrame } from '../previewFrame';
import { DOC_PREVIEW, type DocDraft } from '../../site/documentPreview';

export type Device = 'phone' | 'desktop';

const PREVIEW_PATH = '/admin/document-preview';

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

export type PreviewData = { title: string; html: string; lang: Lang; path: string | null; updatedAt: string | null };

type FrameProps = { data: PreviewData; device: Device; theme: Theme; height: number };

export function DocumentPreviewFrame({ data, device, theme, height }: FrameProps) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const { width } = useSize(box);
  const virtual = device === 'desktop' ? 1280 : 390;
  const scale = width > 0 ? Math.min(1, width / virtual) : 0;
  const { frame, ready, attempt, problem, retry, send } = usePreviewFrame(PREVIEW_PATH, DOC_PREVIEW.ready, scale > 0);

  useEffect(() => {
    if (!ready) {
      return;
    }
    const timer = window.setTimeout(() => {
      const message: DocDraft = {
        type: DOC_PREVIEW.draft,
        title: data.title,
        html: data.html,
        lang: data.lang,
        theme,
        path: data.path,
        updated_at: data.updatedAt,
      };
      send(message);
    }, 150);
    return () => window.clearTimeout(timer);
  }, [ready, send, data, theme]);

  return (
    <div ref={setBox} className={`dce-frame ${device}`} style={{ height }}>
      {scale > 0 ? (
        <div className="dce-frame-in" style={{ width: Math.floor(virtual * scale), height }}>
          <iframe
            key={attempt}
            ref={frame}
            src={PREVIEW_PATH}
            title="Превью страницы"
            style={{ width: virtual, height: Math.ceil(height / scale), transform: `scale(${scale})` }}
          />
        </div>
      ) : null}
      <PreviewFail problem={problem} onRetry={retry} />
    </div>
  );
}

type ToolsProps = { device: Device; theme: Theme; onDevice: (device: Device) => void; onTheme: (theme: Theme) => void; phoneFirst?: boolean };

export function PreviewTools({ device, theme, onDevice, onTheme, phoneFirst }: ToolsProps) {
  const devices: Array<{ value: Device; label: string }> = phoneFirst
    ? [
        { value: 'phone', label: 'Телефон' },
        { value: 'desktop', label: 'Компьютер' },
      ]
    : [
        { value: 'desktop', label: 'Компьютер' },
        { value: 'phone', label: 'Телефон' },
      ];
  return (
    <div className="dce-pv-tools">
      <div role="group" aria-label="Превью на устройстве" className="segs">
        {devices.map((d) => (
          <button key={d.value} type="button" aria-pressed={device === d.value} onClick={() => onDevice(d.value)}>
            {d.label}
          </button>
        ))}
      </div>
      <div role="group" aria-label="Тема превью" className="segs">
        <button type="button" aria-pressed={theme === 'light'} onClick={() => onTheme('light')}>
          Светлая
        </button>
        <button type="button" aria-pressed={theme === 'dark'} onClick={() => onTheme('dark')}>
          Тёмная
        </button>
      </div>
    </div>
  );
}
