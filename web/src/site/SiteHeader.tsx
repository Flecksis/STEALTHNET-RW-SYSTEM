import { Link } from 'react-router';
import { BrandMark } from '../lib/brand';
import { NATIVE, t, type Lang } from '../lib/i18n';
import { Icon } from '../lib/icons';
import type { Theme } from '../lib/theme';
import type { Site } from '../lib/types';

type Props = {
  site: Site;
  phone: boolean;
  home: string;
  nav: Array<{ label: string; href: string }>;
  theme: Theme;
  fixed: boolean;
  onTheme: () => void;
  nextLang: Lang | null;
  onLang: (next: Lang) => void;
  bot: string | null;
};

export function SiteHeader({ site, phone, home, nav, theme, fixed, onTheme, nextLang, onLang, bot }: Props) {
  const themeLabel = theme === 'dark' ? t('Светлая тема') : t('Тёмная тема');
  if (phone) {
    return (
      <header className="ld-head">
        <a href={home} className="ld-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="ld-mark" />
          <span>{site.brand_name.toUpperCase()}</span>
        </a>
        <span className="ld-grow" />
        {nextLang ? (
          <button type="button" className="ld-circ ld-lang" lang={nextLang} aria-label={NATIVE[nextLang]} title={NATIVE[nextLang]} onClick={() => onLang(nextLang)}>
            {nextLang.toUpperCase()}
          </button>
        ) : null}
        {fixed ? null : (
          <button type="button" className="ld-circ" aria-label={themeLabel} onClick={onTheme}>
            <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={20} />
          </button>
        )}
        <Link to="/cabinet" className="ld-enter">
          {t('Войти')}
        </Link>
      </header>
    );
  }
  return (
    <header className="ld-head">
      <div className="ld-head-in">
        <a href={home} className="ld-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="ld-mark" />
          <span>{site.brand_name.toUpperCase()}</span>
        </a>
        {nav.length ? (
          <nav aria-label={t('Разделы страницы')} className="ld-nav">
            {nav.map((item) => (
              <a key={item.href} href={item.href}>
                {item.label}
              </a>
            ))}
          </nav>
        ) : null}
        <span className="ld-head-end">
          {nextLang ? (
            <button type="button" className="ld-ghost ld-lang hov" lang={nextLang} onClick={() => onLang(nextLang)}>
              {NATIVE[nextLang]}
            </button>
          ) : null}
          {fixed ? null : (
            <button type="button" className="ld-icon hov" aria-label={themeLabel} title={themeLabel} onClick={onTheme}>
              <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={20} />
            </button>
          )}
          {bot ? (
            <a href={bot} className="ld-ghost hov">
              {t('Telegram-бот')}
            </a>
          ) : null}
          <Link to="/cabinet" className="ld-btn sm">
            {t('Личный кабинет')}
          </Link>
        </span>
      </div>
    </header>
  );
}
