import { useCallback, useEffect, useRef, useState, type RefObject } from 'react';
import { FINE, previewReason, type PreviewAnswer } from './previewReason';

const WAIT_MS = 4000;
const SLOW_MS = 15_000;
const STEP_MS = 1000;
const ANSWER_MS = 5000;

const SILENT = 'Страница превью не отвечает. Проверьте соединение и повторите.';

async function ask(path: string): Promise<string> {
  try {
    const response = await fetch(path, { cache: 'no-store', credentials: 'same-origin', redirect: 'manual', signal: AbortSignal.timeout(ANSWER_MS) });
    const answer: PreviewAnswer = {
      status: response.status,
      redirected: response.type === 'opaqueredirect',
      frame: response.headers.get('x-frame-options'),
      policy: response.headers.get('content-security-policy'),
    };
    return previewReason(answer, path, window.location.origin);
  } catch {
    return SILENT;
  }
}

function loading(frame: HTMLIFrameElement | null): boolean {
  try {
    return !!frame?.contentDocument;
  } catch {
    return false;
  }
}

type PreviewFrame = {
  frame: RefObject<HTMLIFrameElement | null>;
  ready: number;
  attempt: number;
  problem: string | null;
  retry: () => void;
  send: (message: unknown) => void;
};

export function usePreviewFrame(path: string, readyType: string, shown: boolean, onMessage?: (data: Record<string, unknown>) => void): PreviewFrame {
  const frame = useRef<HTMLIFrameElement>(null);
  const [ready, setReady] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const [problem, setProblem] = useState<string | null>(null);
  const listen = useRef(onMessage);

  useEffect(() => {
    listen.current = onMessage;
  }, [onMessage]);

  useEffect(() => {
    const onWindowMessage = (event: MessageEvent) => {
      if (event.origin !== window.location.origin || !frame.current || event.source !== frame.current.contentWindow) {
        return;
      }
      const data = event.data as Record<string, unknown> | null;
      if (data?.type === readyType) {
        setProblem(null);
        setReady((n) => n + 1);
      } else if (data) {
        listen.current?.(data);
      }
    };
    window.addEventListener('message', onWindowMessage);
    return () => window.removeEventListener('message', onWindowMessage);
  }, [readyType]);

  const opened = ready > 0;
  useEffect(() => {
    if (!shown || opened) {
      return;
    }
    let alive = true;
    let timer = 0;
    const started = Date.now();
    const settle = (reason: string) => {
      if (!alive) {
        return;
      }
      if (reason === FINE && loading(frame.current) && Date.now() - started < SLOW_MS) {
        timer = window.setTimeout(() => settle(reason), STEP_MS);
        return;
      }
      setProblem(reason);
    };
    timer = window.setTimeout(() => {
      ask(path)
        .then(settle)
        .catch(() => undefined);
    }, WAIT_MS);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [path, shown, opened, attempt]);

  const retry = useCallback(() => {
    setProblem(null);
    setReady(0);
    setAttempt((n) => n + 1);
  }, []);

  const send = useCallback((message: unknown) => frame.current?.contentWindow?.postMessage(message, window.location.origin), []);

  return { frame, ready, attempt, problem, retry, send };
}

export function PreviewFail({ problem, onRetry }: { problem: string | null; onRetry: () => void }) {
  return problem ? (
    <div className="pv-fail" role="alert">
      <strong>Превью не открылось</strong>
      <span className="muted">{problem}</span>
      <button type="button" className="btn-line hov" onClick={onRetry}>
        Повторить
      </button>
    </div>
  ) : null;
}
