import { useSite, DEFAULT_SITE } from '../lib/site';
import { BrandMark } from '../lib/brand';
import { useSurface, useSystemTheme } from '../lib/theme';
import './soon.css';

type Props = {
  title: string;
  text: string;
};

export function Soon({ title, text }: Props) {
  const site = useSite() ?? DEFAULT_SITE;
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
        <p>{text}</p>
        {site.bot_username ? (
          <a className="soon-btn" href={`https://t.me/${site.bot_username.replace(/^@/, '')}`}>
            Открыть бота
          </a>
        ) : null}
      </div>
    </main>
  );
}
