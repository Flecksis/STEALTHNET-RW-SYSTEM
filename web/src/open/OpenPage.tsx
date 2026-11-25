import { useCallback, useEffect, useRef, useState } from 'react';
import { local, type LinkButton } from '../cabinet/connect';
import { api, ApiError } from '../lib/api';
import { appScheme } from '../lib/appScheme';
import { BrandMark } from '../lib/brand';
import { guessLang, locale, msg, siteLangs, t, useApplied, type Lang } from '../lib/i18n';
import { usePhone } from '../lib/media';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, type Theme } from '../lib/theme';
import { takeVisit, watchStay, type Visit } from './visit';
import './open.css';

type Opened = {
  name: string;
  link: string;
  subscription_url: string | null;
  install: LinkButton[];
  expires_in: number;
  locale: Lang;
};

type Failure = { text: string; again: boolean };

const NO_LINK: Failure = { text: msg('Откройте «Подключение» в мини-приложении и нажмите кнопку ещё раз.'), again: false };
const EXPIRED: Failure = { text: msg('Ссылка устарела. Вернитесь в Telegram и нажмите кнопку ещё раз.'), again: false };
const VPN_DOWN = msg('Сервис VPN сейчас недоступен. Попробуйте через несколько минут.');

const TEXTS: Record<string, string> = {
  'app_link.expired': EXPIRED.text,
  'app_link.taken': msg('Эту ссылку уже открыли в другом окне. Вернитесь в Telegram и нажмите кнопку ещё раз.'),
  'app_link.no_app': msg('Эту кнопку убрали. Вернитесь в Telegram и откройте «Подключение» заново.'),
  'app_link.not_ready': msg('Подписка ещё не готова: ссылка появится, когда она заработает.'),
};

function failure(err: unknown): Failure {
  if (!(err instanceof ApiError)) {
    return { text: msg('Страница не загрузилась. Проверьте интернет и попробуйте ещё раз.'), again: true };
  }
  const text = TEXTS[err.code] ?? (err.code.startsWith('remnawave.') ? VPN_DOWN : err.message);
  return { text, again: err.status === 0 || err.status === 429 || err.status >= 500 || err.code === 'app_link.not_ready' };
}

function clock(at: number): string {
  return new Date(at).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' });
}

function useOpened() {
  const [visit, setVisit] = useState<Visit | null>(() => takeVisit(window));
  const [view, setView] = useState<Opened | null>(null);
  const [failed, setFailed] = useState<Failure | null>(visit ? null : NO_LINK);
  const [account, setAccount] = useState<Lang | null>(null);
  const [round, setRound] = useState(0);

  useEffect(() => {
    const onHash = () => {
      const next = takeVisit(window);
      if (next) {
        setVisit((known) => (known?.token === next.token ? known : next));
      }
    };
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  useEffect(() => {
    if (!visit) {
      return;
    }
    let gone = false;
    setView(null);
    setFailed(null);
    api<Opened>('/api/public/app-link', { method: 'POST', body: visit })
      .then((data) => {
        if (!gone) {
          if (appScheme(data.link)) {
            setAccount(data.locale);
            setView(data);
          } else {
            setFailed(NO_LINK);
          }
        }
      })
      .catch((err) => {
        if (!gone) {
          setFailed(failure(err));
        }
      });
    return () => {
      gone = true;
    };
  }, [visit, round]);

  useEffect(() => {
    if (!view) {
      return;
    }
    const timer = window.setTimeout(() => {
      setView(null);
      setFailed(EXPIRED);
    }, view.expires_in * 1000);
    return () => window.clearTimeout(timer);
  }, [view]);

  const again = useCallback(() => setRound((n) => n + 1), []);
  return { view, failed, account, again };
}

function Copy({ url }: { url: string }) {
  const [state, setState] = useState<'idle' | 'done' | 'failed'>('idle');
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(url);
      setState('done');
      window.setTimeout(() => setState('idle'), 2000);
    } catch {
      setState('failed');
    }
  };
  return (
    <>
      <button type="button" className="oa-btn line" onClick={() => void copy()}>
        {state === 'done' ? t('Скопировано') : t('Скопировать ссылку')}
      </button>
      {state === 'failed' ? (
        <p className="oa-sub" role="alert">
          {t('Браузер не дал скопировать: выделите ссылку вручную')}
          <span className="oa-url mono">{url}</span>
        </p>
      ) : null}
    </>
  );
}

function Launch({ view }: { view: Opened }) {
  const [stayed, setStayed] = useState(false);
  const [until] = useState(() => Date.now() + view.expires_in * 1000);
  const started = useRef(false);
  const stop = useRef<(() => void) | null>(null);

  const watch = useCallback(() => {
    stop.current?.();
    setStayed(false);
    stop.current = watchStay(window, () => setStayed(true));
  }, []);

  useEffect(() => {
    if (!started.current) {
      started.current = true;
      window.location.href = view.link;
    }
    watch();
    return () => stop.current?.();
  }, [view.link, watch]);

  const named = { app: view.name };
  const help = view.subscription_url ? t('Нажмите «Открыть {app}» ещё раз или скопируйте ссылку.', named) : t('Нажмите «Открыть {app}» ещё раз.', named);
  return (
    <>
      <section className="oa-card" aria-live="polite">
        <h1 className="oa-title">{stayed ? t('Не открылось?') : t('Открываем {app}', named)}</h1>
        <p className="oa-sub two">{stayed ? help : t('Разрешите открыть приложение, если браузер спросит.')}</p>
        <a className="oa-btn" href={view.link} onClick={watch}>
          {t('Открыть {app}', named)}
        </a>
        {view.subscription_url ? <Copy url={view.subscription_url} /> : null}
        {stayed && view.install.length ? (
          <>
            <p className="oa-sub">{t('Приложение ещё не установлено? Установите его и вернитесь сюда.')}</p>
            {view.install.map((button) => (
              <a key={`${button.url}-${local(button.label)}`} className="oa-btn line" href={button.url} target="_blank" rel="noreferrer">
                {local(button.label)}
              </a>
            ))}
          </>
        ) : null}
      </section>
      <p className="oa-note">{t('Страница работает до {time} и только в этом браузере.', { time: clock(until) })}</p>
    </>
  );
}

export function OpenPage() {
  const loadedSite = useSite();
  const site = loadedSite ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const phone = usePhone();
  const choice = site.appearance.client_theme;
  const stored = readStored('sn.client.theme');
  const personal: Theme | null = stored === 'dark' || stored === 'light' ? stored : null;
  const theme: Theme = choice === 'dark' || choice === 'light' ? choice : (personal ?? system);
  useSurface(phone ? 'cabinet-phone' : 'cabinet', theme, site.appearance.client === 'mono' ? 'b' : 'a');
  const { view, failed, account, again } = useOpened();
  const wanted = account && siteLangs(loadedSite).includes(account) ? account : loadedSite ? guessLang(loadedSite) : null;
  const ready = useApplied(loadedSite && (view || failed) ? wanted : null);
  const bot = site.bot_username ? `https://t.me/${site.bot_username.replace(/^@/, '')}` : null;

  if (!loadedSite) {
    return <main className="oa" aria-busy="true" />;
  }
  return (
    <main className="oa" aria-busy={!ready}>
      <div className="oa-box">
        <div className="oa-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="oa-mark" />
          <span>{site.brand_name.toUpperCase()}</span>
        </div>
        {ready && view ? <Launch key={view.link} view={view} /> : null}
        {ready && failed ? (
          <section className="oa-card">
            <h1 className="oa-title">{t('Приложение не открылось')}</h1>
            <p className="oa-sub" role="alert">
              {t(failed.text)}
            </p>
            {failed.again ? (
              <button type="button" className="oa-btn" onClick={again}>
                {t('Повторить')}
              </button>
            ) : bot ? (
              <a className="oa-btn" href={bot}>
                {t('Вернуться в бот')}
              </a>
            ) : null}
          </section>
        ) : null}
      </div>
    </main>
  );
}
