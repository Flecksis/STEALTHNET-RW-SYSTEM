import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { count, money, NBSP, type Money } from '../lib/format';
import { countriesIn, countryName, joinAnd } from '../lib/geo';
import { Icon } from '../lib/icons';
import { usePhone } from '../lib/media';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { Currency } from '../lib/types';
import './landing.css';

type Reset = 'no_reset' | 'day' | 'week' | 'month' | 'month_rolling' | 'on_payment';

type Offer = {
  category: { id: number; name: string; sort: number };
  tariff: {
    id: number;
    name: string;
    description: string | null;
    devices: { included: number; extra: { max: number; monthly: { usd?: number; rub?: number } } | null };
    traffic: { limit_bytes: number | null; reset: Reset };
  };
  periods: Array<{ period_id: number; days: number; months: number; price: Money; per_month: Money; savings_percent: number | null }>;
  countries: string[];
};

type PublicCatalog = { currency: Currency; tariffs: Offer[] };

const THEME_KEY = 'sn.site.theme';
const GB = 1024 ** 3;

function termName(months: number, daysCount: number): string {
  return daysCount >= 28 ? count(months, 'месяц', 'месяца', 'месяцев') : count(daysCount, 'день', 'дня', 'дней');
}

function trafficText(offer: Offer): string {
  const limit = offer.tariff.traffic.limit_bytes ?? 0;
  if (!limit) {
    return 'трафик без лимита';
  }
  const per: Record<Reset, string> = {
    month: 'в месяц',
    month_rolling: 'в месяц',
    week: 'в неделю',
    day: 'в день',
    on_payment: 'на оплаченный срок',
    no_reset: 'на весь срок',
  };
  return `${Math.round(limit / GB)}${NBSP}ГБ ${per[offer.tariff.traffic.reset]}`;
}

function everywhere(offer: Offer): string {
  const devices = count(offer.tariff.devices.included, 'устройство', 'устройства', 'устройств');
  const where = offer.countries.length ? (countriesIn(offer.countries) ? `серверы в ${countriesIn(offer.countries)}` : `серверы: ${joinAnd(offer.countries.map(countryName))}`) : null;
  return `Во всех вариантах: ${[devices, trafficText(offer), where].filter(Boolean).join(', ')}.`;
}

function extraPrice(offer: Offer, currency: Currency): Money | null {
  const extra = offer.tariff.devices.extra;
  if (!extra || !extra.max) {
    return null;
  }
  const minor = currency === 'RUB' ? extra.monthly.rub : extra.monthly.usd;
  return minor ? { minor, currency } : null;
}

export function Landing() {
  const site = useSite() ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const phone = usePhone();
  const [personal, setPersonal] = useState<Theme | null>(() => {
    const v = readStored(THEME_KEY);
    return v === 'dark' || v === 'light' ? v : null;
  });
  const [catalog, setCatalog] = useState<PublicCatalog | null>(null);
  const [cat, setCat] = useState<number | null>(null);
  const [term, setTerm] = useState<number | null>(null);

  const choice = site.appearance.client_theme;
  const fixed = choice === 'dark' || choice === 'light' ? choice : null;
  const theme: Theme = fixed ?? personal ?? system;
  useSurface('landing', theme, site.appearance.client === 'mono' ? 'b' : 'a');

  useEffect(() => {
    document.title = site.title ?? site.brand_name;
  }, [site.title, site.brand_name]);

  useEffect(() => {
    api<PublicCatalog>('/api/public/catalog')
      .then((c) => {
        setCatalog(c);
        setCat(c.tariffs[0]?.category.id ?? null);
      })
      .catch(() => setCatalog({ currency: site.currency, tariffs: [] }));
  }, [site.currency]);

  const offers = catalog?.tariffs ?? [];
  const categories = useMemo(() => {
    const seen = new Map<number, { id: number; name: string; sort: number }>();
    for (const o of offers) {
      seen.set(o.category.id, o.category);
    }
    return [...seen.values()].sort((a, b) => a.sort - b.sort);
  }, [offers]);
  const offer = offers.find((o) => o.category.id === cat) ?? offers[0] ?? null;
  const currency = catalog?.currency ?? site.currency;
  const bot = site.bot_username ? `https://t.me/${site.bot_username.replace(/^@/, '')}` : null;
  const trial = site.trial?.enabled ? site.trial.days : 0;
  const year = new Date().getFullYear();
  const selected = offer?.periods.find((p) => p.period_id === term) ?? offer?.periods[Math.min(1, (offer?.periods.length ?? 1) - 1)] ?? null;
  const extra = offer ? extraPrice(offer, currency) : null;

  const flipTheme = () => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    writeStored(THEME_KEY, next);
    setPersonal(next);
  };
  const themeLabel = theme === 'dark' ? 'Светлая тема' : 'Тёмная тема';

  const payWord =
    currency === 'RUB' ? 'Через СБП, картой российского банка, ЮMoney или звёздами Telegram.' : 'Криптовалютой (USDT, TON, BTC), картой Visa или Mastercard любой страны или звёздами Telegram.';

  const serversLines = offers
    .filter((o, i, all) => all.findIndex((x) => x.tariff.id === o.tariff.id) === i)
    .map((o) => ({ name: o.tariff.name, text: o.countries.length ? joinAnd(o.countries.map(countryName)) : '' }))
    .filter((l) => l.text);

  const faq: Array<[string, string]> = [
    ['Чем кабинет отличается от бота?', 'По возможностям ничем: подписка, оплата, устройства и поддержка есть в обоих. Кабинет удобнее с компьютера, бот — с телефона.'],
    [
      'Сколько устройств можно подключить?',
      offer
        ? `В тариф входят ${count(offer.tariff.devices.included, 'устройство', 'устройства', 'устройств')}.${offer.tariff.devices.extra?.max && extra ? ` Ещё ${offer.tariff.devices.extra.max} можно докупить в кабинете или в боте, ${money(extra)} в месяц за каждое.` : ''}`
        : 'Сколько устройств входит, указано в тарифе.',
    ],
    ['Что будет, когда подписка закончится?', `Напомним заранее в боте. Если включено автопродление, оплата спишется с баланса${currency === 'RUB' ? ' или сохранённой карты' : ''}.`],
    ['Можно вернуть деньги?', 'Да, если сервис у вас не заработал. Напишите в поддержку.'],
    ['Вы храните, какие сайты я открываю?', 'Нет. Мы не записываем, какие сайты вы открываете.'],
  ];

  const header = phone ? (
    <header className="ld-head">
      <a href="#top" className="ld-brand">
        <BrandMark name={site.brand_name} logo={site.logo_url} className="ld-mark" />
        <span>{site.brand_name.toUpperCase()}</span>
      </a>
      <span className="ld-grow" />
      {fixed ? null : (
        <button type="button" className="ld-circ" aria-label={themeLabel} onClick={flipTheme}>
          <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={20} />
        </button>
      )}
      <Link to="/cabinet" className="ld-enter">
        Войти
      </Link>
    </header>
  ) : (
    <header className="ld-head">
      <div className="ld-head-in">
        <a href="#top" className="ld-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="ld-mark" />
          <span>{site.brand_name.toUpperCase()}</span>
        </a>
        <nav aria-label="Разделы страницы" className="ld-nav">
          <a href="#ways">Кабинет и бот</a>
          <a href="#plans">Тарифы</a>
          <a href="#faq">Вопросы</a>
        </nav>
        <span className="ld-head-end">
          {fixed ? null : (
            <button type="button" className="ld-icon hov" aria-label={themeLabel} title={themeLabel} onClick={flipTheme}>
              <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={20} />
            </button>
          )}
          {bot ? (
            <a href={bot} className="ld-ghost hov">
              Telegram-бот
            </a>
          ) : null}
          <Link to="/cabinet" className="ld-btn sm">
            Личный кабинет
          </Link>
        </span>
      </div>
    </header>
  );

  return (
    <div className={phone ? 'ld ld-phone' : 'ld'} id="top">
      {header}
      <main>
        <section aria-labelledby="hero-h" className="ld-sec ld-hero">
          <div className="ld-hero-text">
            <h1 id="hero-h">
              Подключение{phone ? <br /> : ' '}за минуту
            </h1>
            <p className="ld-lead">
              {phone
                ? 'Купите подписку в личном кабинете на сайте или в Telegram-боте. После оплаты придёт ссылка для приложения на iPhone, Android, Windows, macOS или Linux.'
                : 'Купите подписку в личном кабинете на сайте или в Telegram-боте, как вам удобнее. После оплаты придёт ссылка для приложения на iPhone, Android, Windows, macOS или Linux.'}
            </p>
            <div className="ld-cta">
              <Link to="/cabinet" className="ld-btn big">
                Начать в браузере
              </Link>
              {bot ? (
                <a href={bot} className="ld-btn2 big">
                  Начать в Telegram
                </a>
              ) : null}
            </div>
            {trial ? <p className="ld-muted">Первые {count(trial, 'день', 'дня', 'дней')} бесплатно, оплата не нужна</p> : null}
          </div>
          {phone ? null : (
            <div className="ld-art" aria-hidden="true">
              <div className="ld-win">
                <div className="ld-win-bar">
                  <span>{window.location.host}</span>
                </div>
                <div className="ld-win-body">
                  <span className="ld-muted small">Личный кабинет</span>
                  <strong>{offer ? `${offer.tariff.name} · 3 месяца` : '3 месяца'}</strong>
                  <span className="ld-win-days">90 дней</span>
                  <span className="ld-win-bar2">
                    <span />
                  </span>
                  <span className="ld-win-btns">
                    <span className="ld-chip main">Подключить устройство</span>
                    <span className="ld-chip">Продлить</span>
                  </span>
                </div>
              </div>
              <div className="ld-phone-art">
                <div className="ld-screen">
                  <span className="ld-screen-cap">Мини-приложение в Telegram</span>
                  <svg viewBox="0 0 130 130" width="130" height="130">
                    <circle cx="65" cy="65" r="56" fill="none" stroke="rgba(255,255,255,.07)" strokeWidth="9" />
                    <circle cx="65" cy="65" r="56" fill="none" stroke="var(--accent)" strokeWidth="9" />
                  </svg>
                  <span className="ld-screen-num">
                    <strong>90</strong>
                    <span>дней</span>
                  </span>
                  <span className="ld-screen-btn">Подключить</span>
                  <span className="ld-screen-list">
                    <span>Продлить</span>
                    <span>Устройства 2/{offer?.tariff.devices.included ?? 3}</span>
                  </span>
                </div>
              </div>
            </div>
          )}
        </section>

        <section id="ways" aria-labelledby="ways-h" className="ld-sec">
          <h2 id="ways-h">Где удобнее, там и пользуйтесь</h2>
          <div className="ld-grid two">
            <div className="ld-item">
              <h3>Личный кабинет в браузере</h3>
              <p>
                {phone
                  ? 'Вход через Telegram. Удобно с компьютера: оплата, устройства, инструкции и поддержка.'
                  : 'Вход через Telegram. Удобно с компьютера: тарифы и оплата, устройства, инструкции по подключению и поддержка.'}
              </p>
              <Link to="/cabinet" className="ld-link">
                Открыть кабинет
              </Link>
            </div>
            {bot ? (
              <div className="ld-item">
                <h3>Telegram-бот и мини-приложение</h3>
                <p>Без регистрации: открыли бота, и подписка уже в чате. Там же оплата, ссылка для приложения и напоминания о продлении.</p>
                <a href={bot} className="ld-link">
                  Открыть бота
                </a>
              </div>
            ) : null}
          </div>
        </section>

        <section aria-labelledby="steps-h" className="ld-sec">
          <h2 id="steps-h">Как подключиться</h2>
          <ol className="ld-grid three ld-steps">
            {[
              ['Выберите тариф', 'В кабинете или в боте: месяц, три месяца или год.'],
              ['Оплатите', payWord],
              ['Подключитесь', phone ? 'Ссылка подписки появится в кабинете или в боте. Откройте её в приложении.' : 'Откройте ссылку подписки в приложении. Кабинет и бот подскажут, какое поставить.'],
            ].map(([title, text], i) => (
              <li key={title} className="ld-item">
                {phone ? <span className="ld-num num">{i + 1}</span> : <span className="ld-step">Шаг {i + 1}</span>}
                <h3>{title}</h3>
                <p>{text}</p>
              </li>
            ))}
          </ol>
        </section>

        {offer ? (
          <section id="plans" aria-labelledby="plans-h" className="ld-sec ld-plans">
            <div className="ld-plans-head">
              <div>
                <h2 id="plans-h">Тарифы</h2>
                {!phone && offer.tariff.description ? <p className="ld-ink2">{offer.tariff.description}</p> : null}
              </div>
              {categories.length > 1 ? (
                <div role="group" aria-label="Тип тарифа" className="ld-seg">
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
                <div role="radiogroup" aria-label="Срок подписки" className="ld-terms">
                  {offer.periods.map((p) => (
                    <button key={p.period_id} type="button" role="radio" aria-checked={selected?.period_id === p.period_id} className="ld-term" onClick={() => setTerm(p.period_id)}>
                      <span className="ld-dot" aria-hidden="true" />
                      <span className="ld-term-name">{termName(p.months, p.days)}</span>
                      <span className="ld-term-price num">{money(p.price)}</span>
                      <span className="ld-term-sub">
                        {money(p.per_month)} в месяц
                        {p.savings_percent ? <span className="ld-save"> · выгоднее на {p.savings_percent}%</span> : null}
                      </span>
                    </button>
                  ))}
                </div>
                {selected ? (
                  <Link to={`/cabinet/pay?tariff=${offer.tariff.id}&period=${selected.period_id}`} className="ld-btn big full">
                    Купить за {money(selected.price)}
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
                      {money(p.per_month)} в месяц
                      {p.savings_percent ? <span className="ld-save"> · выгоднее на {p.savings_percent}%</span> : null}
                    </span>
                    <Link
                      to={`/cabinet/pay?tariff=${offer.tariff.id}&period=${p.period_id}`}
                      className={i === 1 ? 'ld-btn mid' : 'ld-btn2 mid'}
                      aria-label={`Выбрать: ${offer.tariff.name}, ${termName(p.months, p.days)}`}
                    >
                      Выбрать
                    </Link>
                  </div>
                ))}
              </div>
            )}
          </section>
        ) : null}

        <section aria-label="Серверы и приложения" className="ld-sec">
          <div className="ld-grid two wide">
            {serversLines.length ? (
              <div className="ld-item">
                <h2 className="ld-h2s">Серверы</h2>
                {phone ? (
                  <div className="ld-list">
                    {serversLines.map((l) => (
                      <span key={l.name}>
                        <strong>{l.name}</strong>
                        <span className="ld-ink2">{l.text}</span>
                      </span>
                    ))}
                    <span className="ld-muted small">Страну можно сменить в приложении</span>
                  </div>
                ) : (
                  <p>
                    {serversLines.map((l) => `${l.name}: ${l.text}.`).join(' ')} Переключаться между странами можно в приложении.
                  </p>
                )}
              </div>
            ) : null}
            <div className="ld-item">
              <h2 className="ld-h2s">Приложения</h2>
              {phone ? (
                <>
                  <div className="ld-apps">
                    {['Happ', 'v2rayTun', 'Hiddify', 'Streisand', 'Clash Verge', 'NekoBox'].map((a) => (
                      <span key={a}>{a}</span>
                    ))}
                  </div>
                  <span className="ld-muted small">Кабинет и бот подскажут, какое поставить</span>
                </>
              ) : (
                <p>Happ, v2rayTun, Hiddify, Streisand, Clash Verge и NekoBox. Кабинет и бот подскажут, какое поставить на ваше устройство.</p>
              )}
            </div>
          </div>
        </section>

        <section id="faq" aria-labelledby="faq-h" className="ld-sec ld-faq">
          <h2 id="faq-h">Вопросы</h2>
          {faq.map(([q, a]) => (
            <details key={q}>
              <summary>
                <span>{q}</span>
                <Icon name="chevron-down" size={20} />
              </summary>
              <p>{a}</p>
            </details>
          ))}
        </section>

        <section aria-labelledby="band-h" className="ld-sec ld-band-wrap">
          <div className="ld-band">
            <div>
              <h2 id="band-h">{trial ? `${count(trial, 'день', 'дня', 'дней')} бесплатно` : 'Подключитесь сегодня'}</h2>
              <p>{trial ? 'Откройте кабинет или бота и включите пробный период.' : 'Откройте кабинет или бота и выберите тариф.'}</p>
            </div>
            <span className="ld-band-btns">
              <Link to="/cabinet" className="ld-btn dark">
                Личный кабинет
              </Link>
              {bot ? (
                <a href={bot} className="ld-btn2 dark">
                  Telegram-бот
                </a>
              ) : null}
            </span>
          </div>
        </section>
      </main>
      <footer className="ld-foot">
        <span className="ld-foot-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="ld-mark sm" />
          © {year} {site.brand_name.toUpperCase()}
        </span>
        <nav aria-label="Ссылки" className="ld-foot-nav">
          {site.support_url ? <a href={site.support_url}>Поддержка</a> : null}
          {bot ? <a href={bot}>Telegram-бот</a> : null}
          <Link to="/cabinet">Личный кабинет</Link>
        </nav>
      </footer>
    </div>
  );
}
