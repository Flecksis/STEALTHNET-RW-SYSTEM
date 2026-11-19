import { useEffect, useMemo, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { count, money, NBSP } from '../lib/format';
import { countriesIn, countryName, joinAnd } from '../lib/geo';
import { applyLang, guessLang, NATIVE, otherLang, rememberLang, siteLangs, t, useApplied, useLang, type Lang } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { usePhone } from '../lib/media';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import './landing.css';
import { botLink, storedReferral } from '../lib/referral';
import { reportVisit, storedUtm } from '../lib/utm';
import { startParam } from '../lib/utmTags';
import {
  appsNote,
  CTA_BUTTONS,
  ctaText,
  ctaTitle,
  DEFAULT_APPS,
  DEFAULT_TEXT,
  defaultBlocks,
  findBlock,
  FOOTER_LINKS,
  footerText,
  HERO_BUTTONS,
  HERO_TITLE,
  heroLead,
  imageUrl,
  isOn,
  landingContext,
  menuLinks,
  normalizeBlocks,
  PREVIEW_MESSAGES,
  resolveLinks,
  serversNote,
  shownItems,
  text,
  trialWord,
  visibleSections,
  type Block,
  type BlockKind,
  type LandingContext,
  type LandingImage,
  type Offer,
  type PreviewDraft,
  type PublicCatalog,
  type PublicLanding,
  type Reset,
  type ResolvedLink,
} from './landingModel';

const THEME_KEY = 'sn.site.theme';
const GB = 1024 ** 3;

function termName(months: number, daysCount: number): string {
  return daysCount >= 28 ? count(months, 'месяц', 'месяца', 'месяцев') : count(daysCount, 'день', 'дня', 'дней');
}

function trafficText(offer: Offer): string {
  const limit = offer.tariff.traffic.limit_bytes ?? 0;
  if (!limit) {
    return t('трафик без лимита');
  }
  const traffic = `${Math.round(limit / GB)}${NBSP}${t('ГБ')}`;
  const per: Record<Reset, string> = {
    month: t('{traffic} в месяц', { traffic }),
    month_rolling: t('{traffic} в месяц', { traffic }),
    week: t('{traffic} в неделю', { traffic }),
    day: t('{traffic} в день', { traffic }),
    on_payment: t('{traffic} на оплаченный срок', { traffic }),
    no_reset: t('{traffic} на весь срок', { traffic }),
  };
  return per[offer.tariff.traffic.reset];
}

function everywhere(offer: Offer): string {
  const devices = count(offer.tariff.devices.included, 'устройство', 'устройства', 'устройств');
  const named = countriesIn(offer.countries);
  const where = offer.countries.length ? (named ? t('серверы в {where}', { where: named }) : t('серверы: {list}', { list: joinAnd(offer.countries.map(countryName)) })) : null;
  return t('Во всех вариантах: {list}.', { list: [devices, trafficText(offer), where].filter(Boolean).join(', ') });
}

function Go({ link, className, label }: { link: ResolvedLink; className?: string; label?: string }) {
  return link.route ? (
    <Link to={link.href} className={className} aria-label={label}>
      {link.label}
    </Link>
  ) : (
    <a href={link.href} className={className} aria-label={label}>
      {link.label}
    </a>
  );
}

function Picture({ image, className }: { image: LandingImage; className: string }) {
  const src = imageUrl(image);
  if (!src) {
    return null;
  }
  return <img className={className} src={src} width={image.width} height={image.height} alt="" loading="lazy" decoding="async" draggable={false} />;
}

function Illustration({ offer }: { offer: Offer | null }) {
  return (
    <div className="ld-art" aria-hidden="true">
      <div className="ld-win">
        <div className="ld-win-bar">
          <span>{window.location.host}</span>
        </div>
        <div className="ld-win-body">
          <span className="ld-muted small">{t('Личный кабинет')}</span>
          <strong>{offer ? `${offer.tariff.name} · ${t('3 месяца')}` : t('3 месяца')}</strong>
          <span className="ld-win-days">{t('90 дней')}</span>
          <span className="ld-win-bar2">
            <span />
          </span>
          <span className="ld-win-btns">
            <span className="ld-chip main">{t('Подключить устройство')}</span>
            <span className="ld-chip">{t('Продлить')}</span>
          </span>
        </div>
      </div>
      <div className="ld-phone-art">
        <div className="ld-screen">
          <span className="ld-screen-cap">{t('Мини-приложение в Telegram')}</span>
          <svg viewBox="0 0 130 130" width="130" height="130">
            <circle cx="65" cy="65" r="56" fill="none" stroke="rgba(255,255,255,.07)" strokeWidth="9" />
            <circle cx="65" cy="65" r="56" fill="none" stroke="var(--accent)" strokeWidth="9" />
          </svg>
          <span className="ld-screen-num">
            <strong>90</strong>
            <span>{t('дней')}</span>
          </span>
          <span className="ld-screen-btn">{t('Подключить')}</span>
          <span className="ld-screen-list">
            <span>{t('Продлить')}</span>
            <span>{t('Устройства 2/{n}', { n: offer?.tariff.devices.included ?? 3 })}</span>
          </span>
        </div>
      </div>
    </div>
  );
}

type Mode = 'live' | 'preview';

export function Landing() {
  return <LandingPage mode="live" />;
}

export function LandingPreview() {
  return <LandingPage mode="preview" />;
}

function LandingPage({ mode }: { mode: Mode }) {
  const framed = mode === 'preview' && window.parent !== window;
  const loadedSite = useSite();
  const site = loadedSite ?? DEFAULT_SITE;
  const [picked, setPicked] = useState<Lang | null>(null);
  const langs = siteLangs(loadedSite);
  const ready = useApplied(mode === 'preview' ? 'ru' : loadedSite ? (picked && langs.includes(picked) ? picked : guessLang(loadedSite)) : null);
  const lang = useLang();
  const nextLang = mode === 'preview' ? null : otherLang(lang, langs);
  const switchLang = (next: Lang) => {
    rememberLang(next);
    setPicked(next);
    void applyLang(next);
  };
  const system = useSystemTheme();
  const phone = usePhone();
  const [personal, setPersonal] = useState<Theme | null>(() => {
    if (framed) {
      return null;
    }
    const v = readStored(THEME_KEY);
    return v === 'dark' || v === 'light' ? v : null;
  });
  const [forced, setForced] = useState<Theme | null>(null);
  const [catalog, setCatalog] = useState<PublicCatalog | null>(null);
  const [cat, setCat] = useState<number | null>(null);
  const [term, setTerm] = useState<number | null>(null);
  const [blocks, setBlocks] = useState<Block[] | null>(null);
  const [focus, setFocus] = useState<{ kind: BlockKind; at: number } | null>(null);

  const choice = site.appearance.client_theme;
  const fixed = choice === 'dark' || choice === 'light' ? choice : null;
  const theme: Theme = forced ?? fixed ?? personal ?? system;
  useSurface('landing', theme, site.appearance.client === 'mono' ? 'b' : 'a');

  useEffect(() => {
    if (!framed) {
      document.title = site.title ?? site.brand_name;
    }
  }, [framed, site.title, site.brand_name]);

  useEffect(() => {
    if (mode !== 'preview') {
      reportVisit();
    }
  }, [mode]);

  useEffect(() => {
    api<PublicCatalog>('/api/public/catalog')
      .then((c) => {
        setCatalog(c);
        setCat(c.tariffs[0]?.category.id ?? null);
      })
      .catch(() => setCatalog({ currency: site.currency, tariffs: [] }));
  }, [site.currency]);

  useEffect(() => {
    if (framed) {
      const origin = window.location.origin;
      const onMessage = (event: MessageEvent) => {
        if (event.origin !== origin || event.source !== window.parent) {
          return;
        }
        const data = event.data as PreviewDraft | null;
        if (!data || data.type !== PREVIEW_MESSAGES.draft) {
          return;
        }
        setBlocks(normalizeBlocks(data.blocks));
        setForced(data.theme === 'dark' || data.theme === 'light' ? data.theme : null);
        if (data.focus) {
          setFocus({ kind: data.focus, at: Date.now() });
        }
      };
      window.addEventListener('message', onMessage);
      window.parent.postMessage({ type: PREVIEW_MESSAGES.ready }, origin);
      return () => window.removeEventListener('message', onMessage);
    }
    let alive = true;
    const slow = window.setTimeout(() => setBlocks((current) => current ?? defaultBlocks()), 2500);
    api<PublicLanding>('/api/public/landing')
      .then((landing) => {
        if (alive) {
          setBlocks(normalizeBlocks(landing.blocks));
        }
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'module.disabled') {
          window.location.replace(`/cabinet${window.location.search}`);
          return;
        }
        if (alive) {
          setBlocks((current) => current ?? defaultBlocks());
        }
      })
      .finally(() => window.clearTimeout(slow));
    return () => {
      alive = false;
      window.clearTimeout(slow);
    };
  }, [framed]);

  useEffect(() => {
    if (!focus) {
      return;
    }
    const target = focus.kind === 'hero' ? document.getElementById('top') : document.querySelector(`[data-block="${focus.kind}"]`);
    if (target) {
      window.scrollTo({ top: target.getBoundingClientRect().top + window.scrollY });
    }
  }, [focus, blocks]);

  const offers = useMemo(() => catalog?.tariffs ?? [], [catalog]);
  const categories = useMemo(() => {
    const seen = new Map<number, { id: number; name: string; sort: number }>();
    for (const o of offers) {
      seen.set(o.category.id, o.category);
    }
    return [...seen.values()].sort((a, b) => a.sort - b.sort);
  }, [offers]);
  const offer = offers.find((o) => o.category.id === cat) ?? offers[0] ?? null;
  const currency = catalog?.currency ?? site.currency;
  const start = startParam(storedUtm(), storedReferral());
  const bot = site.bot_username ? botLink(site.bot_username, start) : null;
  const trial = site.trial?.enabled ? site.trial.days : 0;
  const selected = offer?.periods.find((p) => p.period_id === term) ?? offer?.periods[Math.min(1, (offer?.periods.length ?? 1) - 1)] ?? null;
  const ctx: LandingContext = landingContext({ phone, currency, trial, bot, support: site.support_url, brand: site.brand_name, offer, offers, botName: site.bot_username, botStart: start });

  const flipTheme = () => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    if (framed) {
      setForced(next);
      return;
    }
    writeStored(THEME_KEY, next);
    setPersonal(next);
  };
  const themeLabel = theme === 'dark' ? t('Светлая тема') : t('Тёмная тема');
  const rootClass = phone ? 'ld ld-phone' : 'ld';

  if (!blocks || !ready) {
    return <div className={rootClass} aria-busy="true" />;
  }

  const sections = visibleSections(blocks);
  const nav = menuLinks(blocks);

  const onPreviewClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const anchor = target.closest('a');
    if (anchor) {
      event.preventDefault();
      const href = anchor.getAttribute('href') ?? '';
      const section = href.startsWith('#') && href.length > 1 ? document.getElementById(href.slice(1)) : null;
      if (section) {
        window.scrollTo({ top: section.getBoundingClientRect().top + window.scrollY });
      }
    }
    const kind = target.closest('[data-block]')?.getAttribute('data-block');
    if (kind) {
      window.parent.postMessage({ type: PREVIEW_MESSAGES.pick, kind }, window.location.origin);
    }
  };

  const serversLines = offers
    .filter((o, i, all) => all.findIndex((x) => x.tariff.id === o.tariff.id) === i)
    .map((o) => ({ name: o.tariff.name, text: o.countries.length ? joinAnd(o.countries.map(countryName)) : '' }))
    .filter((l) => l.text);

  const header = phone ? (
    <header className="ld-head">
      <a href="#top" className="ld-brand">
        <BrandMark name={site.brand_name} logo={site.logo_url} className="ld-mark" />
        <span>{site.brand_name.toUpperCase()}</span>
      </a>
      <span className="ld-grow" />
      {nextLang ? (
        <button type="button" className="ld-circ ld-lang" lang={nextLang} aria-label={NATIVE[nextLang]} title={NATIVE[nextLang]} onClick={() => switchLang(nextLang)}>
          {nextLang.toUpperCase()}
        </button>
      ) : null}
      {fixed ? null : (
        <button type="button" className="ld-circ" aria-label={themeLabel} onClick={flipTheme}>
          <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={20} />
        </button>
      )}
      <Link to="/cabinet" className="ld-enter">
        {t('Войти')}
      </Link>
    </header>
  ) : (
    <header className="ld-head">
      <div className="ld-head-in">
        <a href="#top" className="ld-brand">
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
            <button type="button" className="ld-ghost ld-lang hov" lang={nextLang} onClick={() => switchLang(nextLang)}>
              {NATIVE[nextLang]}
            </button>
          ) : null}
          {fixed ? null : (
            <button type="button" className="ld-icon hov" aria-label={themeLabel} title={themeLabel} onClick={flipTheme}>
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

  const hero = findBlock(blocks, 'hero');
  const heroButtons = resolveLinks(hero.buttons, HERO_BUTTONS, ctx, sections);
  const lead = text(hero.lead, heroLead(phone));
  const note = hero.note ?? '';
  const art = hero.art ?? 'default';
  const [heroFirst, ...heroRest] = t(HERO_TITLE).split(' ');
  const heroSection = (
    <section key="hero" aria-labelledby="hero-h" className="ld-sec ld-hero" data-block="hero">
      <div className="ld-hero-text">
        <h1 id="hero-h">
          {hero.title === null || hero.title === undefined ? (
            <>
              {heroFirst}
              {heroRest.length ? (phone ? <br /> : ' ') : null}
              {heroRest.join(' ')}
            </>
          ) : (
            hero.title
          )}
        </h1>
        {lead ? <p className="ld-lead">{lead}</p> : null}
        {heroButtons.length ? (
          <div className="ld-cta">
            {heroButtons.map((link, i) => (
              <Go key={`${link.href}-${i}`} link={link} className={i === 0 ? 'ld-btn big' : 'ld-btn2 big'} />
            ))}
          </div>
        ) : null}
        {hero.note === null || hero.note === undefined ? (
          trial ? <p className="ld-muted">{t('Первые {days} бесплатно, оплата не нужна', { days: trialWord(trial) })}</p> : null
        ) : note ? (
          <p className="ld-muted">{note}</p>
        ) : null}
      </div>
      {art === 'default' && !phone ? <Illustration offer={offer} /> : null}
      {art === 'image' && hero.image ? (
        <div className="ld-pic">
          <Picture image={hero.image} className="ld-pic-img" />
        </div>
      ) : null}
    </section>
  );

  const render = (block: Block): ReactNode => {
    if (!isOn(block)) {
      return null;
    }
    switch (block.kind) {
      case 'hero':
        return heroSection;
      case 'features': {
        const items = shownItems('features', block.items, ctx, sections);
        return (
          <section key="features" id="ways" aria-labelledby="ways-h" className="ld-sec" data-block="features">
            <h2 id="ways-h">{text(block.title, t(DEFAULT_TEXT.featuresTitle)) || t(DEFAULT_TEXT.featuresTitle)}</h2>
            {items.length ? (
              <div className="ld-grid two">
                {items.map((item) => (
                  <div key={item.key} className="ld-item">
                    {item.image ? <Picture image={item.image} className="ld-item-pic" /> : null}
                    <h3>{item.title}</h3>
                    {item.text ? <p>{item.text}</p> : null}
                    {item.link ? <Go link={item.link} className="ld-link" /> : null}
                  </div>
                ))}
              </div>
            ) : null}
          </section>
        );
      }
      case 'steps': {
        const items = shownItems('steps', block.items, ctx, sections);
        return (
          <section key="steps" id="steps" aria-labelledby="steps-h" className="ld-sec" data-block="steps">
            <h2 id="steps-h">{text(block.title, t(DEFAULT_TEXT.stepsTitle)) || t(DEFAULT_TEXT.stepsTitle)}</h2>
            {items.length ? (
              <ol className="ld-grid three ld-steps">
                {items.map((item, i) => (
                  <li key={item.key} className="ld-item">
                    {phone ? <span className="ld-num num">{i + 1}</span> : <span className="ld-step">{t('Шаг {n}', { n: i + 1 })}</span>}
                    <h3>{item.title}</h3>
                    {item.text ? <p>{item.text}</p> : null}
                  </li>
                ))}
              </ol>
            ) : null}
          </section>
        );
      }
      case 'plans': {
        if (!offer) {
          return null;
        }
        const plansText = text(block.text, '');
        return (
          <section key="plans" id="plans" aria-labelledby="plans-h" className="ld-sec ld-plans" data-block="plans">
            <div className="ld-plans-head">
              <div>
                <h2 id="plans-h">{text(block.title, t(DEFAULT_TEXT.plansTitle)) || t(DEFAULT_TEXT.plansTitle)}</h2>
                {plansText ? <p className="ld-ink2 ld-plans-text">{plansText}</p> : null}
                {!phone && offer.tariff.description ? <p className="ld-ink2">{offer.tariff.description}</p> : null}
              </div>
              {categories.length > 1 ? (
                <div role="group" aria-label={t('Тип тарифа')} className="ld-seg">
                  {categories.map((c) => (
                    <button
                      key={c.id}
                      type="button"
                      aria-pressed={offer.category.id === c.id}
                      onClick={() => {
                        setCat(c.id);
                        setTerm(null);
                      }}
                    >
                      {c.name}
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
            <p className={phone ? 'ld-ink2' : 'ld-strong'}>
              {phone && offer.tariff.description ? `${offer.tariff.description}. ` : ''}
              {everywhere(offer)}
            </p>
            {phone ? (
              <>
                <div role="radiogroup" aria-label={t('Срок подписки')} className="ld-terms">
                  {offer.periods.map((p) => (
                    <button key={p.period_id} type="button" role="radio" aria-checked={selected?.period_id === p.period_id} className="ld-term" onClick={() => setTerm(p.period_id)}>
                      <span className="ld-dot" aria-hidden="true" />
                      <span className="ld-term-name">{termName(p.months, p.days)}</span>
                      <span className="ld-term-price num">{money(p.price)}</span>
                      <span className="ld-term-sub">
                        {t('{price} в месяц', { price: money(p.per_month) })}
                        {p.savings_percent ? <span className="ld-save"> · {t('выгоднее на {n}%', { n: p.savings_percent })}</span> : null}
                      </span>
                    </button>
                  ))}
                </div>
                {selected ? (
                  <Link to={`/cabinet/pay?tariff=${offer.tariff.id}&period=${selected.period_id}`} className="ld-btn big full">
                    {t('Купить за {price}', { price: money(selected.price) })}
                  </Link>
                ) : null}
              </>
            ) : (
              <div className="ld-grid cards">
                {offer.periods.map((p, i) => (
                  <div key={p.period_id} className="ld-card">
                    <span className="ld-card-name">{termName(p.months, p.days)}</span>
                    <span className="ld-card-price">{money(p.price)}</span>
                    <span className="ld-muted">
                      {t('{price} в месяц', { price: money(p.per_month) })}
                      {p.savings_percent ? <span className="ld-save"> · {t('выгоднее на {n}%', { n: p.savings_percent })}</span> : null}
                    </span>
                    <Link
                      to={`/cabinet/pay?tariff=${offer.tariff.id}&period=${p.period_id}`}
                      className={i === 1 ? 'ld-btn mid' : 'ld-btn2 mid'}
                      aria-label={t('Выбрать: {plan}, {period}', { plan: offer.tariff.name, period: termName(p.months, p.days) })}
                    >
                      {t('Выбрать')}
                    </Link>
                  </div>
                ))}
              </div>
            )}
          </section>
        );
      }
      case 'servers': {
        const apps = block.apps ?? DEFAULT_APPS;
        const sNote = text(block.servers_note, serversNote(phone));
        const aNote = text(block.apps_note, appsNote(phone));
        if (!serversLines.length && !apps.length) {
          return null;
        }
        return (
          <section key="servers" id="servers" aria-label={t('Серверы и приложения')} className="ld-sec" data-block="servers">
            <div className="ld-grid two wide">
              {serversLines.length ? (
                <div className="ld-item">
                  <h2 className="ld-h2s">{text(block.servers_title, t(DEFAULT_TEXT.serversTitle)) || t(DEFAULT_TEXT.serversTitle)}</h2>
                  {phone ? (
                    <div className="ld-list">
                      {serversLines.map((l) => (
                        <span key={l.name}>
                          <strong>{l.name}</strong>
                          <span className="ld-ink2">{l.text}</span>
                        </span>
                      ))}
                      {sNote ? <span className="ld-muted small">{sNote}</span> : null}
                    </div>
                  ) : (
                    <p>
                      {serversLines.map((l) => `${l.name}: ${l.text}.`).join(' ')}
                      {sNote ? ` ${sNote}` : ''}
                    </p>
                  )}
                </div>
              ) : null}
              {apps.length ? (
                <div className="ld-item">
                  <h2 className="ld-h2s">{text(block.apps_title, t(DEFAULT_TEXT.appsTitle)) || t(DEFAULT_TEXT.appsTitle)}</h2>
                  {phone ? (
                    <>
                      <div className="ld-apps">
                        {apps.map((a, i) => (
                          <span key={`${a}-${i}`}>{a}</span>
                        ))}
                      </div>
                      {aNote ? <span className="ld-muted small">{aNote}</span> : null}
                    </>
                  ) : (
                    <p>{[`${joinAnd(apps)}.`, aNote].filter(Boolean).join(' ')}</p>
                  )}
                </div>
              ) : null}
            </div>
          </section>
        );
      }
      case 'faq': {
        const items = shownItems('faq', block.items, ctx, sections);
        return (
          <section key="faq" id="faq" aria-labelledby="faq-h" className="ld-sec ld-faq" data-block="faq">
            <h2 id="faq-h">{text(block.title, t(DEFAULT_TEXT.faqTitle)) || t(DEFAULT_TEXT.faqTitle)}</h2>
            {items.map((item) => (
              <details key={item.key}>
                <summary>
                  <span>{item.title}</span>
                  <Icon name="chevron-down" size={20} />
                </summary>
                <p>{item.text}</p>
              </details>
            ))}
          </section>
        );
      }
      case 'cta': {
        const buttons = resolveLinks(block.buttons, CTA_BUTTONS, ctx, sections);
        const title = text(block.title, ctaTitle(trial, ctx.from)) || ctaTitle(trial, ctx.from);
        const body = text(block.text, ctaText(trial));
        return (
          <section key="cta" id="start" aria-labelledby="band-h" className="ld-sec ld-band-wrap" data-block="cta">
            <div className="ld-band">
              <div>
                <h2 id="band-h">{title}</h2>
                {body ? <p>{body}</p> : null}
              </div>
              {buttons.length ? (
                <span className="ld-band-btns">
                  {buttons.map((link, i) => (
                    <Go key={`${link.href}-${i}`} link={link} className={i === 0 ? 'ld-btn dark' : 'ld-btn2 dark'} />
                  ))}
                </span>
              ) : null}
            </div>
          </section>
        );
      }
      case 'footer':
        return null;
    }
  };

  const footer = findBlock(blocks, 'footer');
  const footerLinks = resolveLinks(footer.links, FOOTER_LINKS, ctx, sections);

  return (
    <div className={rootClass} id="top" onClickCapture={framed ? onPreviewClick : undefined}>
      {header}
      <main>{blocks.map(render)}</main>
      <footer className="ld-foot" data-block="footer">
        <span className="ld-foot-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="ld-mark sm" />
          {footer.text === null || footer.text === undefined ? (
            <>
              © {ctx.year} {site.brand_name.toUpperCase()}
            </>
          ) : (
            footerText(footer.text, site.brand_name, ctx.year)
          )}
        </span>
        <nav aria-label={t('Ссылки')} className="ld-foot-nav">
          {footerLinks.map((link, i) => (
            <Go key={`${link.href}-${i}`} link={link} />
          ))}
          {(site.documents ?? []).map((doc) => (
            <a key={doc.url} href={doc.url} target="_blank" rel="noreferrer noopener">
              {doc.title}
            </a>
          ))}
        </nav>
      </footer>
    </div>
  );
}

