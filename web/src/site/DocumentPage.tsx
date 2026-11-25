import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { cleanHtml } from '../lib/cleanHtml';
import { documentTitle } from '../lib/documents';
import { dateLong } from '../lib/format';
import { applyLang, guessLang, otherLang, rememberLang, siteLangs, t, useApplied, useLang, type Lang } from '../lib/i18n';
import { usePhone } from '../lib/media';
import { botLink, storedReferral } from '../lib/referral';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import { DEFAULT_SITE, showTitle, useSite } from '../lib/site';
import type { Site, SiteDocument } from '../lib/types';
import { storedUtm } from '../lib/utm';
import { startParam } from '../lib/utmTags';
import { findBlock, FOOTER_LINKS, footerText, landingContext, normalizeBlocks, resolveLinks, visibleSections, type Block, type PublicLanding } from './landingModel';
import { DOC_PREVIEW, type DocDraft } from './documentPreview';
import { wrapLongWords } from './longWords';
import { SiteHeader } from './SiteHeader';
import { Soon } from './Soon';
import './landing.css';
import './document.css';

const THEME_KEY = 'sn.site.theme';

type PublicPage = {
  path: string;
  title: string;
  lang: Lang;
  html: string;
  description: string | null;
  updated_at: string;
  english: boolean;
};

type Props = { site: Site; path: string };

function FooterLink({ doc, current }: { doc: SiteDocument; current: string }) {
  const title = documentTitle(doc);
  if (doc.page) {
    return (
      <Link to={doc.url} aria-current={doc.url === current ? 'page' : undefined}>
        {title}
      </Link>
    );
  }
  return (
    <a href={doc.url} target="_blank" rel="noreferrer noopener">
      {title}
    </a>
  );
}

type ViewProps = {
  site: Site;
  phone: boolean;
  theme: Theme;
  fixed: boolean;
  onTheme: () => void;
  nextLang: Lang | null;
  onLang: (next: Lang) => void;
  bot: string | null;
  path: string | null;
  title: string;
  html: string;
  lang: Lang;
  updatedAt: string | null;
  blocks: Block[] | null;
  start: string | null;
};

function DocumentView({ site, phone, theme, fixed, onTheme, nextLang, onLang, bot, path, title, html, lang, updatedAt, blocks, start }: ViewProps) {
  const ctx = landingContext({ phone, currency: site.currency, trial: 0, bot, support: site.support_url, brand: site.brand_name, offer: null, offers: [], botName: site.bot_username, botStart: start });
  const footer = blocks ? findBlock(blocks, 'footer') : null;
  const footerLinks = resolveLinks(footer?.links, FOOTER_LINKS, ctx, blocks ? visibleSections(blocks) : new Set()).map((link) =>
    link.href.startsWith('#') ? { ...link, href: `/${link.href}`, route: false } : link,
  );
  const documents = site.documents ?? [];
  return (
    <div className={phone ? 'ld ld-phone' : 'ld'} id="top">
      <SiteHeader site={site} phone={phone} home="/" nav={[]} theme={theme} fixed={fixed} onTheme={onTheme} nextLang={nextLang} onLang={onLang} bot={bot} />
      <main className="dp-main">
        <article className="dp-doc">
          <h1 lang={lang}>{title}</h1>
          <p className="dp-meta">{t('Обновлено {date}', { date: dateLong(updatedAt ?? new Date().toISOString(), true) })}</p>
          <div className="dp-body" lang={lang} dangerouslySetInnerHTML={{ __html: html }} />
        </article>
      </main>
      <footer className="ld-foot">
        <span className="ld-foot-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="ld-mark sm" />
          {footerText(footer?.text, site.brand_name, ctx.year)}
        </span>
        <nav aria-label={t('Ссылки')} className="ld-foot-nav dp-foot-nav">
          {footerLinks.map((link, i) =>
            link.route ? (
              <Link key={`${link.href}-${i}`} to={link.href}>
                {link.label}
              </Link>
            ) : (
              <a key={`${link.href}-${i}`} href={link.href}>
                {link.label}
              </a>
            ),
          )}
          {documents.map((doc) => (
            <FooterLink key={doc.url} doc={doc} current={path ?? ''} />
          ))}
        </nav>
      </footer>
    </div>
  );
}

function useBlocks(): Block[] | null {
  const [blocks, setBlocks] = useState<Block[] | null>(null);
  useEffect(() => {
    let alive = true;
    api<PublicLanding>('/api/public/landing')
      .then((landing) => {
        if (alive) {
          setBlocks(normalizeBlocks(landing.blocks));
        }
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);
  return blocks;
}

export function DocumentPreview() {
  const loaded = useSite();
  const site = loaded ?? DEFAULT_SITE;
  const phone = usePhone();
  const blocks = useBlocks();
  const [draft, setDraft] = useState<DocDraft | null>(null);
  const ready = useApplied(draft ? draft.lang : null);
  useSurface('landing', draft?.theme ?? 'light', site.appearance.client === 'mono' ? 'b' : 'a');

  useEffect(() => {
    const origin = window.location.origin;
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== origin || event.source !== window.parent) {
        return;
      }
      const data = event.data as DocDraft | null;
      if (data && data.type === DOC_PREVIEW.draft) {
        setDraft(data);
      }
    };
    window.addEventListener('message', onMessage);
    if (window.parent !== window) {
      window.parent.postMessage({ type: DOC_PREVIEW.ready }, origin);
    }
    return () => window.removeEventListener('message', onMessage);
  }, []);

  const html = useMemo(() => (draft ? wrapLongWords(cleanHtml(draft.html)) : ''), [draft]);

  if (!loaded || !draft || !ready) {
    return <div className={phone ? 'ld ld-phone' : 'ld'} aria-busy="true" />;
  }
  return (
    <div
      className="dp-frame"
      onClickCapture={(event) => {
        if ((event.target as HTMLElement).closest('a, button')) {
          event.preventDefault();
        }
      }}
    >
      <DocumentView
        site={site}
        phone={phone}
        theme={draft.theme}
        fixed={false}
        onTheme={() => undefined}
        nextLang={null}
        onLang={() => undefined}
        bot={site.bot_username ? botLink(site.bot_username, null) : null}
        path={draft.path}
        title={draft.title}
        html={html}
        lang={draft.lang}
        updatedAt={draft.updated_at}
        blocks={blocks}
        start={null}
      />
    </div>
  );
}

export function DocumentPage({ site, path }: Props) {
  const [picked, setPicked] = useState<Lang | null>(null);
  const langs = siteLangs(site);
  const ready = useApplied(picked && langs.includes(picked) ? picked : guessLang(site));
  const lang = useLang();
  const nextLang = otherLang(lang, langs);
  const switchLang = (next: Lang) => {
    rememberLang(next);
    setPicked(next);
    void applyLang(next);
  };
  const system = useSystemTheme();
  const phone = usePhone();
  const [personal, setPersonal] = useState<Theme | null>(() => {
    const stored = readStored(THEME_KEY);
    return stored === 'dark' || stored === 'light' ? stored : null;
  });
  const choice = site.appearance.client_theme;
  const fixed = choice === 'dark' || choice === 'light' ? choice : null;
  const theme: Theme = fixed ?? personal ?? system;
  useSurface('landing', theme, site.appearance.client === 'mono' ? 'b' : 'a');
  const [page, setPage] = useState<PublicPage | null>(null);
  const [missing, setMissing] = useState(false);
  const blocks = useBlocks();

  useEffect(() => {
    if (!ready) {
      return;
    }
    let alive = true;
    api<PublicPage>(`/api/public/pages${path}`)
      .then((loaded) => {
        if (alive) {
          setPage(loaded);
          setMissing(false);
        }
      })
      .catch((err: unknown) => {
        if (alive && err instanceof ApiError && err.status === 404) {
          setMissing(true);
        }
      });
    return () => {
      alive = false;
    };
  }, [path, ready, lang]);

  useEffect(() => {
    if (page) {
      showTitle(site, (known) => `${page.title} — ${known.brand_name}`);
    }
  }, [page, site]);

  useEffect(() => {
    window.scrollTo({ top: 0 });
  }, [path]);

  const html = useMemo(() => (page ? wrapLongWords(cleanHtml(page.html)) : ''), [page]);

  if (missing) {
    return <Soon title={t('Страница не найдена')} text={t('Такой страницы нет. Перейдите на главную или откройте Telegram-бота.')} />;
  }
  if (!ready || !page) {
    return <div className={phone ? 'ld ld-phone' : 'ld'} aria-busy="true" />;
  }

  const start = startParam(storedUtm(), storedReferral());
  const flipTheme = () => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    writeStored(THEME_KEY, next);
    setPersonal(next);
  };
  return (
    <DocumentView
      site={site}
      phone={phone}
      theme={theme}
      fixed={fixed !== null}
      onTheme={flipTheme}
      nextLang={nextLang}
      onLang={switchLang}
      bot={site.bot_username ? botLink(site.bot_username, start) : null}
      path={path}
      title={page.title}
      html={html}
      lang={page.lang}
      updatedAt={page.updated_at}
      blocks={blocks}
      start={start}
    />
  );
}
