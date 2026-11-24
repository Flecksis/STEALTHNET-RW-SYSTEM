import { useEffect, useRef, useState } from 'react';
import { lang, t } from './i18n';
import './captcha.css';

export type CaptchaProvider = 'turnstile' | 'yandex';

export type CaptchaWidget = {
  provider: CaptchaProvider;
  site_key: string;
  action: string;
  cdata?: string;
};

type TurnstileApi = {
  render: (el: HTMLElement, options: Record<string, unknown>) => string;
  remove: (id: string) => void;
  reset: (id: string) => void;
};

type SmartCaptchaApi = {
  render: (el: HTMLElement, options: Record<string, unknown>) => number;
  destroy: (id: number) => void;
  reset: (id: number) => void;
  subscribe: (id: number, event: string, cb: (...args: unknown[]) => void) => () => void;
};

declare global {
  interface Window {
    turnstile?: TurnstileApi;
    smartCaptcha?: SmartCaptchaApi;
    snTurnstileReady?: () => void;
    snSmartCaptchaReady?: () => void;
  }
}

const SCRIPTS: Record<CaptchaProvider, string> = {
  turnstile: 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit&onload=snTurnstileReady',
  yandex: 'https://smartcaptcha.cloud.yandex.ru/captcha.js?render=onload&onload=snSmartCaptchaReady',
};

const LOAD_TIMEOUT_MS = 15_000;
const WIDGET_WIDTH = 300;
const loading = new Map<CaptchaProvider, Promise<void>>();

function ready(provider: CaptchaProvider): boolean {
  return provider === 'turnstile' ? !!window.turnstile : !!window.smartCaptcha;
}

function originReferrer(): void {
  const current = document.querySelector('meta[name="referrer"]');
  if (current) {
    current.setAttribute('content', 'strict-origin-when-cross-origin');
    return;
  }
  const meta = document.createElement('meta');
  meta.name = 'referrer';
  meta.content = 'strict-origin-when-cross-origin';
  document.head.appendChild(meta);
}

export function loadCaptcha(provider: CaptchaProvider): Promise<void> {
  if (ready(provider)) {
    return Promise.resolve();
  }
  const known = loading.get(provider);
  if (known) {
    return known;
  }
  originReferrer();
  const promise = new Promise<void>((resolve, reject) => {
    const done = () => (ready(provider) ? resolve() : reject(new Error('captcha')));
    if (provider === 'turnstile') {
      window.snTurnstileReady = done;
    } else {
      window.snSmartCaptchaReady = done;
    }
    const script = document.createElement('script');
    script.src = SCRIPTS[provider];
    script.async = true;
    script.defer = true;
    script.onerror = () => {
      loading.delete(provider);
      script.remove();
      reject(new Error('captcha'));
    };
    window.setTimeout(() => {
      if (!ready(provider)) {
        loading.delete(provider);
        reject(new Error('captcha'));
      }
    }, LOAD_TIMEOUT_MS);
    document.head.appendChild(script);
  });
  loading.set(provider, promise);
  return promise;
}

type Props = {
  widget: CaptchaWidget;
  onToken: (token: string) => void;
  onExpire?: () => void;
  theme?: 'light' | 'dark';
  round?: number;
  className?: string;
};

export function Captcha({ widget, onToken, onExpire, theme, round = 0, className }: Props) {
  const outer = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const [fit, setFit] = useState({ scale: 1, height: 0 });
  const tokenRef = useRef(onToken);
  const expireRef = useRef(onExpire);
  tokenRef.current = onToken;
  expireRef.current = onExpire;
  const [state, setState] = useState<'loading' | 'ready' | 'failed'>('loading');
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    let gone = false;
    let remove: (() => void) | null = null;
    setState('loading');
    loadCaptcha(widget.provider)
      .then(() => {
        const el = box.current;
        if (gone || !el) {
          return;
        }
        el.innerHTML = '';
        const hl = lang() === 'en' ? 'en' : 'ru';
        if (widget.provider === 'turnstile' && window.turnstile) {
          const api = window.turnstile;
          const id = api.render(el, {
            sitekey: widget.site_key,
            action: widget.action,
            ...(widget.cdata ? { cData: widget.cdata } : {}),
            theme: theme ?? 'auto',
            language: hl,
            callback: (token: string) => tokenRef.current(token),
            'expired-callback': () => expireRef.current?.(),
            'error-callback': () => setState('failed'),
          });
          remove = () => api.remove(id);
        } else if (window.smartCaptcha) {
          const api = window.smartCaptcha;
          const id = api.render(el, {
            sitekey: widget.site_key,
            hl,
            callback: (token: string) => tokenRef.current(token),
          });
          const off = [
            api.subscribe(id, 'token-expired', () => expireRef.current?.()),
            api.subscribe(id, 'network-error', () => setState('failed')),
            api.subscribe(id, 'javascript-error', () => setState('failed')),
          ];
          remove = () => {
            off.forEach((stop) => stop());
            api.destroy(id);
          };
        }
        setState('ready');
      })
      .catch(() => {
        if (!gone) {
          setState('failed');
        }
      });
    return () => {
      gone = true;
      try {
        remove?.();
      } catch {
        return;
      }
    };
  }, [widget.provider, widget.site_key, widget.action, widget.cdata, theme, round, retry]);

  useEffect(() => {
    const frame = outer.current;
    const widget = box.current;
    if (!frame || !widget || typeof ResizeObserver === 'undefined') {
      return;
    }
    const measure = () => {
      const scale = Math.min(1, frame.clientWidth / WIDGET_WIDTH);
      setFit({ scale, height: Math.ceil(widget.offsetHeight * scale) });
    };
    const watch = new ResizeObserver(measure);
    watch.observe(frame);
    watch.observe(widget);
    measure();
    return () => watch.disconnect();
  }, []);

  const shrink = fit.scale < 1;
  return (
    <div ref={outer} className={className ? `snc ${className}` : 'snc'}>
      <div className="snc-fit" style={shrink ? { height: fit.height } : undefined}>
        <div ref={box} className="snc-box" style={shrink ? { transform: `scale(${fit.scale})` } : undefined} />
      </div>
      {state === 'loading' ? (
        <p className="snc-note" aria-live="polite">
          {t('Загружаем проверку…')}
        </p>
      ) : null}
      {state === 'failed' ? (
        <p className="snc-note" role="alert">
          {t('Проверка не загрузилась. Проверьте интернет и попробуйте ещё раз.')}{' '}
          <button type="button" className="snc-retry" onClick={() => setRetry((n) => n + 1)}>
            {t('Повторить')}
          </button>
        </p>
      ) : null}
    </div>
  );
}
