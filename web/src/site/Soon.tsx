import { BrandMark } from '../lib/brand';
import { t } from '../lib/i18n';
import { useSurface, useSystemTheme } from '../lib/theme';
import type { Site } from '../lib/types';
import './soon.css';

type Retry = { busy: boolean; run: () => void };

type Props = {
  site: Site;
  title: string;
  text: string;
  retry?: Retry;
};

function Soon({ site, title, text, retry }: Props) {
  const system = useSystemTheme();
  const choice = site.appearance.client_theme;
  const theme = choice === 'dark' || choice === 'light' ? choice : system;
  useSurface('landing', theme, site.appearance.client === 'mono' ? 'b' : 'a');
  return (
    <main className="soon">
      <div className="soon-box">
        <div className="soon-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="mark" />
          <span>{site.brand_name.toUpperCase()}</span>
        </div>
        <h1>{title}</h1>
        <p role={retry ? 'alert' : undefined}>{text}</p>
        {retry ? (
          <button type="button" className="soon-btn" onClick={retry.run} disabled={retry.busy}>
            {retry.busy ? t('Загружаем…') : t('Повторить')}
          </button>
        ) : site.bot_username ? (
          <a className="soon-btn" href={`https://t.me/${site.bot_username.replace(/^@/, '')}`}>
            {t('Открыть бота')}
          </a>
        ) : null}
      </div>
    </main>
  );
}

export function PageMissing({ site }: { site: Site }) {
  return <Soon site={site} title={t('Страница не найдена')} text={t('Такой страницы нет. Перейдите на главную или откройте Telegram-бота.')} />;
}

export function PageFailed({ site, busy, onRetry }: { site: Site; busy: boolean; onRetry: () => void }) {
  return (
    <Soon
      site={site}
      title={t('Страница не загрузилась')}
      text={t('Проверьте интернет и попробуйте ещё раз. Если не помогло, зайдите чуть позже.')}
      retry={{ busy, run: onRetry }}
    />
  );
}
