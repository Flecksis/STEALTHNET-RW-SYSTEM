import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type ReactNode } from 'react';
import { api, ApiError } from '../../lib/api';
import { count } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { botLink } from '../../lib/referral';
import { useSite } from '../../lib/site';
import {
  appsNote,
  BLOCK_NAMES,
  CTA_BUTTONS,
  ctaText,
  ctaTitle,
  DEFAULT_APPS,
  DEFAULT_MENU,
  DEFAULT_ORDER,
  DEFAULT_TEXT,
  defaultBlocks,
  defaultItems,
  FOOTER_LINKS,
  HERO_BUTTONS,
  HERO_TITLE,
  heroLead,
  heroNote,
  isOn,
  landingContext,
  normalizeBlocks,
  PREVIEW_MESSAGES,
  serversNote,
  text,
  visibleSections,
  type Art,
  type Block,
  type BlockKind,
  type BlockOf,
  type ItemKind,
  type LandingContext,
  type LandingImage,
  type LandingItem,
  type LandingLink,
  type MenuKind,
  type PreviewDraft,
  type PublicCatalog,
  type SectionKind,
} from '../../site/landingModel';
import type { ModulesData, ModuleView } from '../modules/model';
import { Switch } from '../ui';
import { CssEditor, useCssCheck } from './CssEditor';
import { CSS_PLACES, cssOut, joinStyles, lengthProblem, readTexts, sameCss, type CssPlace, type CssProblems, type CssTexts } from './landingCss';
import {
  chars,
  IMAGE_RULES,
  ImageField,
  itemProblem,
  ItemsEditor,
  LIMITS,
  linkProblem,
  LinksEditor,
  sameItems,
  TextField,
  textProblem,
  weight,
  type ImageRules,
  type Limits,
} from './landingFields';
import { SNIPPETS } from './landingSnippets';
import { SettingsLayout } from './SettingsLayout';
import '../form.css';
import './landing-editor.css';

type AdminLanding = { blocks?: unknown; css?: unknown; styles?: unknown; limits?: Partial<Limits>; image?: ImageRules };
type Device = 'desktop' | 'phone';
type PreviewTheme = 'light' | 'dark';
type Focus = { kind: BlockKind; n: number };
type Row = BlockKind | 'css';

const PREVIEW_PATH = '/admin/landing-preview';
const KIND_SET = new Set<string>(DEFAULT_ORDER);

function pinned(kind: BlockKind): boolean {
  return kind === 'hero' || kind === 'footer';
}

function compact<T extends object>(value: T): T {
  const out: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (entry !== undefined && entry !== null) {
      out[key] = entry;
    }
  }
  return out as T;
}

function sameLinks(a: LandingLink[], b: LandingLink[]): boolean {
  return a.length === b.length && a.every((link, i) => link.label === b[i]?.label && link.to === b[i]?.to);
}

function linksOut(links: LandingLink[] | null | undefined, defaults: LandingLink[]): LandingLink[] | undefined {
  if (!links) {
    return undefined;
  }
  const out = links.map((link) => ({ label: link.label, to: link.to }));
  return sameLinks(out, defaults) ? undefined : out;
}

function imageOut(image: LandingImage | null | undefined): LandingImage | undefined {
  return image?.tag ? { tag: image.tag } : undefined;
}

function itemsOut(kind: ItemKind, items: LandingItem[] | null | undefined): LandingItem[] | undefined {
  if (!items || sameItems(kind, items)) {
    return undefined;
  }
  return items.map((item) =>
    item.std
      ? { std: item.std }
      : compact({
          title: item.title ?? '',
          text: item.text ? item.text : undefined,
          link: item.link ? { label: item.link.label, to: item.link.to } : undefined,
          image: imageOut(item.image),
        }),
  );
}

function appsOut(apps: string[] | null | undefined): string[] | undefined {
  if (!apps) {
    return undefined;
  }
  return apps.length === DEFAULT_APPS.length && apps.every((app, i) => app === DEFAULT_APPS[i]) ? undefined : apps;
}

function canonical(blocks: Block[]): Block[] {
  return blocks.map((block): Block => {
    switch (block.kind) {
      case 'hero': {
        const art: Art = block.art ?? 'default';
        return compact({
          kind: 'hero' as const,
          title: block.title,
          lead: block.lead,
          note: block.note,
          buttons: linksOut(block.buttons, HERO_BUTTONS),
          art: art === 'default' ? undefined : art,
          image: art === 'image' ? imageOut(block.image) : undefined,
        });
      }
      case 'features':
      case 'steps':
      case 'faq':
        return compact({ kind: block.kind, on: isOn(block), title: block.title, menu: block.menu, items: itemsOut(block.kind, block.items) }) as Block;
      case 'plans':
        return compact({ kind: 'plans' as const, on: isOn(block), title: block.title, menu: block.menu, text: block.text });
      case 'servers':
        return compact({
          kind: 'servers' as const,
          on: isOn(block),
          menu: block.menu,
          servers_title: block.servers_title,
          servers_note: block.servers_note,
          apps_title: block.apps_title,
          apps: appsOut(block.apps),
          apps_note: block.apps_note,
        });
      case 'cta':
        return compact({ kind: 'cta' as const, on: isOn(block), title: block.title, text: block.text, buttons: linksOut(block.buttons, CTA_BUTTONS) });
      case 'footer':
        return compact({ kind: 'footer' as const, text: block.text, links: linksOut(block.links, FOOTER_LINKS) });
    }
  });
}

function customized(block: Block): boolean {
  const plain = canonical([block])[0] as Record<string, unknown>;
  return Object.keys(plain).some((key) => key !== 'kind' && key !== 'on');
}

function problemsOf(blocks: Block[], limits: Limits): string[] {
  const out: string[] = [];
  const add = (kind: BlockKind, what: string, problem: string | null) => {
    if (problem) {
      out.push(`${BLOCK_NAMES[kind]}, ${what}: ${problem.charAt(0).toLowerCase()}${problem.slice(1)}`);
    }
  };
  const field = (kind: BlockKind, what: string, value: string | null | undefined, max: number, required = false) => {
    if (value !== null && value !== undefined) {
      add(kind, what, textProblem(value, max, required));
    }
  };
  const links = (kind: BlockKind, what: string, list: LandingLink[] | null | undefined, max: number) => {
    if (list && list.length > max) {
      add(kind, what, `не больше ${max}`);
    }
    (list ?? []).forEach((link, i) => add(kind, `${what} ${i + 1}`, linkProblem(link, limits)));
  };
  const items = (kind: ItemKind, list: LandingItem[] | null | undefined) => {
    (list ?? []).forEach((item, i) => {
      const problem = itemProblem(kind, item, limits);
      if (problem) {
        out.push(`${BLOCK_NAMES[kind]}, пункт ${i + 1}, ${problem}`);
      }
    });
  };
  for (const block of blocks) {
    switch (block.kind) {
      case 'hero':
        field('hero', 'заголовок', block.title, limits.title, true);
        field('hero', 'текст', block.lead, limits.text);
        field('hero', 'строка под кнопками', block.note, limits.note);
        links('hero', 'кнопка', block.buttons, limits.hero_buttons);
        if (block.art === 'image' && !block.image?.tag) {
          out.push('Первый экран: загрузите картинку или выберите другой вариант');
        }
        break;
      case 'features':
      case 'steps':
      case 'faq':
        field(block.kind, 'заголовок', block.title, limits.title, true);
        field(block.kind, 'пункт меню', block.menu, limits.menu);
        items(block.kind, block.items);
        break;
      case 'plans':
        field('plans', 'заголовок', block.title, limits.title, true);
        field('plans', 'пункт меню', block.menu, limits.menu);
        field('plans', 'текст', block.text, limits.plans_text);
        break;
      case 'servers':
        field('servers', 'заголовок про серверы', block.servers_title, limits.title, true);
        field('servers', 'строка про серверы', block.servers_note, limits.text);
        field('servers', 'заголовок про приложения', block.apps_title, limits.title, true);
        field('servers', 'строка про приложения', block.apps_note, limits.text);
        field('servers', 'пункт меню', block.menu, limits.menu);
        if (block.apps && block.apps.length > limits.apps) {
          add('servers', 'приложения', `не больше ${limits.apps}`);
        }
        (block.apps ?? []).forEach((app) => {
          if (chars(app) > limits.app) {
            add('servers', `приложение «${app.slice(0, 20)}»`, `не длиннее ${limits.app} символов`);
          }
        });
        break;
      case 'cta':
        field('cta', 'заголовок', block.title, limits.title, true);
        field('cta', 'текст', block.text, limits.text);
        links('cta', 'кнопка', block.buttons, limits.cta_buttons);
        break;
      case 'footer':
        field('footer', 'строка с годом', block.text, limits.footer_text);
        links('footer', 'ссылка', block.links, limits.footer_links);
        break;
    }
  }
  return out;
}

function placeName(place: CssPlace): string {
  return place === 'page' ? 'Общий CSS' : `${BLOCK_NAMES[place]}, CSS блока`;
}

function cssProblemsOf(texts: CssTexts, problems: CssProblems, limits: Limits): string[] {
  const out: string[] = [];
  for (const place of CSS_PLACES) {
    const max = place === 'page' ? limits.page_css : limits.block_css;
    const problem = lengthProblem(texts[place] ?? '', max) ?? problems[place];
    if (problem) {
      out.push(problem.line ? `${placeName(place)}, строка ${problem.line}: ${problem.text}` : `${placeName(place)}: ${problem.text}`);
    }
  }
  return out;
}

function summary(block: Block, ctx: LandingContext): string {
  if (!isOn(block)) {
    return 'Скрыт';
  }
  switch (block.kind) {
    case 'hero':
      return text(block.title, HERO_TITLE) || HERO_TITLE;
    case 'features':
      return text(block.title, DEFAULT_TEXT.featuresTitle) || DEFAULT_TEXT.featuresTitle;
    case 'steps': {
      const n = (block.items ?? defaultItems('steps')).length;
      return n ? count(n, 'шаг', 'шага', 'шагов') : 'Без шагов';
    }
    case 'plans':
      return 'Тарифы из каталога';
    case 'servers': {
      const n = (block.apps ?? DEFAULT_APPS).length;
      return n ? count(n, 'приложение', 'приложения', 'приложений') : 'Только серверы';
    }
    case 'faq': {
      const n = (block.items ?? defaultItems('faq')).length;
      return n ? count(n, 'вопрос', 'вопроса', 'вопросов') : 'Без вопросов';
    }
    case 'cta':
      return text(block.title, ctaTitle(ctx.trial, ctx.from)) || ctaTitle(ctx.trial, ctx.from);
    case 'footer':
      return 'Ссылки и документы';
  }
}

function useSize(node: HTMLElement | null): { width: number; height: number } {
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

function useViewportHeight(): number {
  const [height, setHeight] = useState(() => window.innerHeight);
  useEffect(() => {
    const onResize = () => setHeight(window.innerHeight);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);
  return height;
}

type FrameProps = {
  blocks: Block[];
  css: string;
  device: Device;
  theme: PreviewTheme;
  focus: Focus | null;
  height: number;
  onPick: (kind: BlockKind) => void;
};

function PreviewFrame({ blocks, css, device, theme, focus, height, onPick }: FrameProps) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const { width } = useSize(box);
  const frame = useRef<HTMLIFrameElement>(null);
  const [ready, setReady] = useState(0);
  const sent = useRef(0);
  const pick = useRef(onPick);

  useEffect(() => {
    pick.current = onPick;
  }, [onPick]);

  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== window.location.origin || !frame.current || event.source !== frame.current.contentWindow) {
        return;
      }
      const data = event.data as { type?: unknown; kind?: unknown } | null;
      if (data?.type === PREVIEW_MESSAGES.ready) {
        setReady((n) => n + 1);
      } else if (data?.type === PREVIEW_MESSAGES.pick && typeof data.kind === 'string' && KIND_SET.has(data.kind)) {
        pick.current(data.kind as BlockKind);
      }
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, []);

  useEffect(() => {
    if (!ready) {
      return;
    }
    const timer = window.setTimeout(() => {
      const target = frame.current?.contentWindow;
      if (!target) {
        return;
      }
      const jump = focus && focus.n !== sent.current ? focus.kind : null;
      if (focus) {
        sent.current = focus.n;
      }
      const message: PreviewDraft = { type: PREVIEW_MESSAGES.draft, blocks, theme, css, focus: jump };
      target.postMessage(message, window.location.origin);
    }, 120);
    return () => window.clearTimeout(timer);
  }, [ready, blocks, css, theme, focus]);

  const virtual = device === 'desktop' ? 1280 : 390;
  const scale = width > 0 ? Math.min(1, width / virtual) : 0;
  return (
    <div ref={setBox} className={`lde-frame ${device}`} style={{ height }}>
      {scale > 0 ? (
        <div className="lde-frame-in" style={{ width: Math.floor(virtual * scale), height }}>
          <iframe
            ref={frame}
            src={PREVIEW_PATH}
            title="Превью лендинга"
            style={{ width: virtual, height: Math.ceil(height / scale), transform: `scale(${scale})` }}
          />
        </div>
      ) : null}
    </div>
  );
}

type ToolsProps = { device: Device; theme: PreviewTheme; onDevice: (device: Device) => void; onTheme: (theme: PreviewTheme) => void };

function PreviewTools({ device, theme, onDevice, onTheme }: ToolsProps) {
  return (
    <div className="lde-pv-tools">
      <div role="group" aria-label="Превью на устройстве" className="segs">
        <button type="button" aria-pressed={device === 'desktop'} onClick={() => onDevice('desktop')}>
          Компьютер
        </button>
        <button type="button" aria-pressed={device === 'phone'} onClick={() => onDevice('phone')}>
          Телефон
        </button>
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

function AppsField({ apps, limits, onChange }: { apps: string[] | null | undefined; limits: Limits; onChange: (next: string[] | null) => void }) {
  const list = apps ?? DEFAULT_APPS;
  const [raw, setRaw] = useState(list.join('\n'));
  const joined = list.join('\n');
  useEffect(() => {
    setRaw((current) => {
      const parsed = current
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean)
        .join('\n');
      return parsed === joined ? current : joined;
    });
  }, [joined]);
  const parse = (value: string) =>
    value
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean);
  const parsed = parse(raw);
  const long = parsed.find((app) => chars(app) > limits.app);
  const problem = parsed.length > limits.apps ? `Не больше ${limits.apps}, сейчас ${parsed.length}` : long ? `«${long.slice(0, 24)}» длиннее ${limits.app} символов` : null;
  return (
    <div className="lde-field">
      <label htmlFor="lde-apps" className="lde-label">
        Приложения
      </label>
      <textarea
        id="lde-apps"
        className="field"
        rows={6}
        value={raw}
        aria-invalid={problem ? true : undefined}
        aria-describedby="lde-apps-n"
        onChange={(e) => {
          setRaw(e.target.value);
          const next = parse(e.target.value);
          const same = next.length === DEFAULT_APPS.length && next.every((app, i) => app === DEFAULT_APPS[i]);
          onChange(same ? null : next);
        }}
      />
      {problem ? (
        <span id="lde-apps-n" className="err">
          {problem}
        </span>
      ) : (
        <span id="lde-apps-n" className="cap">
          По одному в строке. Пустой список — колонки «Приложения» не будет
        </span>
      )}
    </div>
  );
}

type BodyProps<K extends BlockKind> = {
  block: BlockOf<K>;
  change: (next: BlockOf<K>) => void;
  ctx: LandingContext;
  sections: Set<SectionKind>;
  limits: Limits;
  rules: ImageRules;
};

function MenuField({ kind, value, limits, onChange }: { kind: MenuKind; value: string | null | undefined; limits: Limits; onChange: (next: string | null) => void }) {
  return <TextField label="Пункт в меню сайта" value={value} fallback={DEFAULT_MENU[kind]} max={limits.menu} hint="Пусто — блока нет в верхнем меню" onChange={onChange} />;
}

function HeroFields({ block, change, ctx, sections, limits, rules }: BodyProps<'hero'>) {
  const art: Art = block.art ?? 'default';
  const arts: Array<[Art, string]> = [
    ['default', 'Стандартная'],
    ['image', 'Своя'],
    ['none', 'Без картинки'],
  ];
  const noteHint =
    block.note === null || block.note === undefined
      ? ctx.trial
        ? 'Пока не меняли, срок берётся из пробного периода'
        : 'Пробный период выключен, строки сейчас нет'
      : 'Пустое поле — строки не будет';
  return (
    <>
      <TextField label="Заголовок" value={block.title} fallback={HERO_TITLE} max={limits.title} required onChange={(title) => change({ ...block, title })} />
      <TextField label="Текст" value={block.lead} fallback={heroLead(false)} max={limits.text} multiline rows={4} onChange={(lead) => change({ ...block, lead })} />
      <div className="lde-field">
        <span className="lde-label">Кнопки</span>
        <LinksEditor
          links={block.buttons ?? HERO_BUTTONS}
          max={limits.hero_buttons}
          addLabel="Добавить кнопку"
          what="Кнопка"
          ctx={ctx}
          sections={sections}
          limits={limits}
          onChange={(next) => change({ ...block, buttons: sameLinks(next, HERO_BUTTONS) ? null : next })}
        />
        <span className="cap">Первая кнопка главная, вторая — обычная</span>
      </div>
      <TextField label="Строка под кнопками" value={block.note} fallback={heroNote(ctx.trial)} max={limits.note} hint={noteHint} onChange={(note) => change({ ...block, note })} />
      <div className="lde-field">
        <span className="lde-label">Картинка справа</span>
        <div role="group" aria-label="Картинка на первом экране" className="segs lde-segs">
          {arts.map(([key, name]) => (
            <button key={key} type="button" aria-pressed={art === key} onClick={() => change({ ...block, art: key === 'default' ? null : key })}>
              {name}
            </button>
          ))}
        </div>
        {art === 'image' ? (
          <ImageField
            image={block.image}
            rules={rules}
            what="первый экран"
            hint={`PNG, JPEG или WebP до ${weight(rules.max_bytes)}. На телефоне картинка встаёт под кнопки`}
            onChange={(image) => change({ ...block, image })}
          />
        ) : (
          <span className="cap">{art === 'default' ? 'Окно кабинета и телефон с мини-приложением. На телефоне не показывается' : 'Текст займёт всю ширину'}</span>
        )}
      </div>
    </>
  );
}

function ItemsFields({ block, change, ctx, sections, limits, rules }: BodyProps<'features' | 'steps' | 'faq'>) {
  const fallback = block.kind === 'features' ? DEFAULT_TEXT.featuresTitle : block.kind === 'steps' ? DEFAULT_TEXT.stepsTitle : DEFAULT_TEXT.faqTitle;
  return (
    <>
      <TextField label="Заголовок" value={block.title} fallback={fallback} max={limits.title} required onChange={(title) => change({ ...block, title })} />
      <MenuField kind={block.kind} value={block.menu} limits={limits} onChange={(menu) => change({ ...block, menu })} />
      <div className="lde-field">
        <span className="lde-label">{block.kind === 'faq' ? 'Вопросы' : block.kind === 'steps' ? 'Шаги' : 'Пункты'}</span>
        <ItemsEditor kind={block.kind} items={block.items} ctx={ctx} sections={sections} limits={limits} rules={rules} onChange={(items) => change({ ...block, items })} />
      </div>
    </>
  );
}

function PlansFields({ block, change, limits }: BodyProps<'plans'>) {
  return (
    <>
      <TextField label="Заголовок" value={block.title} fallback={DEFAULT_TEXT.plansTitle} max={limits.title} required onChange={(title) => change({ ...block, title })} />
      <MenuField kind="plans" value={block.menu} limits={limits} onChange={(menu) => change({ ...block, menu })} />
      <TextField
        label="Текст под заголовком"
        value={block.text}
        fallback=""
        max={limits.plans_text}
        multiline
        hint="Необязательно. Тарифы, цены и сроки берутся из раздела «Тарифы»"
        onChange={(next) => change({ ...block, text: next })}
      />
    </>
  );
}

function ServersFields({ block, change, limits }: BodyProps<'servers'>) {
  return (
    <>
      <TextField label="Заголовок про серверы" value={block.servers_title} fallback={DEFAULT_TEXT.serversTitle} max={limits.title} required onChange={(servers_title) => change({ ...block, servers_title })} />
      <TextField
        label="Строка про серверы"
        value={block.servers_note}
        fallback={serversNote(false)}
        max={limits.text}
        hint="Страны берутся из сквадов тарифов. Пустое поле — без этой строки"
        onChange={(servers_note) => change({ ...block, servers_note })}
      />
      <TextField label="Заголовок про приложения" value={block.apps_title} fallback={DEFAULT_TEXT.appsTitle} max={limits.title} required onChange={(apps_title) => change({ ...block, apps_title })} />
      <AppsField apps={block.apps} limits={limits} onChange={(apps) => change({ ...block, apps })} />
      <TextField label="Строка про приложения" value={block.apps_note} fallback={appsNote(false)} max={limits.text} onChange={(apps_note) => change({ ...block, apps_note })} />
      <MenuField kind="servers" value={block.menu} limits={limits} onChange={(menu) => change({ ...block, menu })} />
    </>
  );
}

function CtaFields({ block, change, ctx, sections, limits }: BodyProps<'cta'>) {
  const dynamic = block.title === null || block.title === undefined;
  return (
    <>
      <TextField
        label="Заголовок"
        value={block.title}
        fallback={ctaTitle(ctx.trial, ctx.from)}
        max={limits.title}
        required
        hint={dynamic ? 'Пока не меняли, текст зависит от пробного периода и цен' : undefined}
        onChange={(title) => change({ ...block, title })}
      />
      <TextField label="Текст" value={block.text} fallback={ctaText(ctx.trial)} max={limits.text} multiline onChange={(next) => change({ ...block, text: next })} />
      <div className="lde-field">
        <span className="lde-label">Кнопки</span>
        <LinksEditor
          links={block.buttons ?? CTA_BUTTONS}
          max={limits.cta_buttons}
          addLabel="Добавить кнопку"
          what="Кнопка"
          ctx={ctx}
          sections={sections}
          limits={limits}
          onChange={(next) => change({ ...block, buttons: sameLinks(next, CTA_BUTTONS) ? null : next })}
        />
      </div>
    </>
  );
}

function FooterFields({ block, change, ctx, sections, limits }: BodyProps<'footer'>) {
  const fallback = `© {year} ${ctx.brand.toUpperCase()}`;
  return (
    <>
      <TextField
        label="Строка с названием"
        value={block.text}
        fallback={fallback}
        max={limits.footer_text}
        hint="{year} — текущий год, меняется сам"
        onChange={(next) => change({ ...block, text: next })}
      />
      <div className="lde-field">
        <span className="lde-label">Ссылки</span>
        <LinksEditor
          links={block.links ?? FOOTER_LINKS}
          max={limits.footer_links}
          addLabel="Добавить ссылку"
          what="Ссылка"
          ctx={ctx}
          sections={sections}
          limits={limits}
          onChange={(next) => change({ ...block, links: sameLinks(next, FOOTER_LINKS) ? null : next })}
        />
        <span className="cap">Документы из раздела «Документы» добавляются в подвал сами</span>
      </div>
    </>
  );
}

function BlockFields(props: BodyProps<BlockKind>) {
  const { block } = props;
  switch (block.kind) {
    case 'hero':
      return <HeroFields {...(props as BodyProps<'hero'>)} />;
    case 'features':
    case 'steps':
    case 'faq':
      return <ItemsFields {...(props as BodyProps<'features' | 'steps' | 'faq'>)} />;
    case 'plans':
      return <PlansFields {...(props as BodyProps<'plans'>)} />;
    case 'servers':
      return <ServersFields {...(props as BodyProps<'servers'>)} />;
    case 'cta':
      return <CtaFields {...(props as BodyProps<'cta'>)} />;
    case 'footer':
      return <FooterFields {...(props as BodyProps<'footer'>)} />;
  }
}

type Drop = { kind: BlockKind; after: boolean };

export function SettingsLanding() {
  const site = useSite();
  const phone = usePhone();
  const viewport = useViewportHeight();
  const [saved, setSaved] = useState<Block[] | null>(null);
  const [draft, setDraft] = useState<Block[] | null>(null);
  const [cssSaved, setCssSaved] = useState<CssTexts>({});
  const [cssDraft, setCssDraft] = useState<CssTexts>({});
  const css = useCssCheck(cssDraft);
  const resetCss = css.reset;
  const [limits, setLimits] = useState<Limits>(LIMITS);
  const [rules, setRules] = useState<ImageRules>(IMAGE_RULES);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [catalog, setCatalog] = useState<PublicCatalog | null>(null);
  const [open, setOpen] = useState<Row | null>(null);
  const [focus, setFocus] = useState<Focus | null>(null);
  const [device, setDevice] = useState<Device>(() => (phone ? 'phone' : 'desktop'));
  const [theme, setTheme] = useState<PreviewTheme>('light');
  const [sheet, setSheet] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState('');
  const [asking, setAsking] = useState(false);
  const [drag, setDrag] = useState<BlockKind | null>(null);
  const [drop, setDrop] = useState<Drop | null>(null);
  const [wrap, setWrap] = useState<HTMLDivElement | null>(null);
  const [sheetBody, setSheetBody] = useState<HTMLDivElement | null>(null);
  const [module, setModule] = useState<ModuleView | null>(null);
  const [enabling, setEnabling] = useState(false);
  const [enableError, setEnableError] = useState<string | null>(null);
  const { width } = useSize(wrap);
  const sheetSize = useSize(sheetBody);
  const wide = !phone && width >= 860;

  const apply = useCallback((view: AdminLanding) => {
    const blocks = normalizeBlocks(view.blocks);
    const texts = readTexts(view.css);
    setSaved(blocks);
    setDraft(blocks);
    setCssSaved(texts);
    setCssDraft(texts);
    resetCss(texts, readTexts(view.styles));
    setLimits({ ...LIMITS, ...(view.limits ?? {}) });
    setRules(view.image ?? IMAGE_RULES);
  }, [resetCss]);

  useEffect(() => {
    api<AdminLanding>('/api/admin/settings/landing')
      .then(apply)
      .catch((err: unknown) => setLoadError(err instanceof ApiError ? err.message : 'Настройки лендинга не загрузились'));
    api<PublicCatalog>('/api/public/catalog')
      .then(setCatalog)
      .catch(() => setCatalog(null));
    api<ModulesData>('/api/admin/modules')
      .then((list) => setModule(list.modules.find((m) => m.info.id === 'landing') ?? null))
      .catch(() => setModule(null));
  }, [apply]);

  const enableModule = async () => {
    setEnabling(true);
    setEnableError(null);
    try {
      setModule(await api<ModuleView>('/api/admin/modules/landing', { method: 'PATCH', body: { enabled: true } }));
    } catch (err) {
      setEnableError(err instanceof ApiError ? err.message : 'Модуль не включился');
    } finally {
      setEnabling(false);
    }
  };

  const clientTheme = site?.appearance.client_theme;
  useEffect(() => {
    if (clientTheme === 'dark' || clientTheme === 'light') {
      setTheme(clientTheme);
    }
  }, [clientTheme]);

  const ctx = useMemo(
    () =>
      landingContext({
        phone: false,
        currency: catalog?.currency ?? site?.currency ?? 'USD',
        trial: site?.trial?.enabled ? site.trial.days : 0,
        bot: site?.bot_username ? botLink(site.bot_username) : null,
        support: site?.support_url ?? null,
        brand: site?.brand_name ?? 'STEALTHNET',
        offer: catalog?.tariffs[0] ?? null,
        offers: catalog?.tariffs ?? [],
      }),
    [catalog, site],
  );
  const sections = useMemo(() => visibleSections(draft ?? []), [draft]);
  const dirty = useMemo(
    () => (saved && draft ? JSON.stringify(canonical(draft)) !== JSON.stringify(canonical(saved)) || !sameCss(cssDraft, cssSaved) : false),
    [saved, draft, cssDraft, cssSaved],
  );
  const problems = useMemo(
    () => (draft ? [...problemsOf(draft, limits), ...cssProblemsOf(cssDraft, css.problems, limits)] : []),
    [draft, limits, cssDraft, css.problems],
  );
  const previewCss = useMemo(() => joinStyles(css.styles), [css.styles]);

  useEffect(() => {
    if (!dirty) {
      return;
    }
    const onLeave = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', onLeave);
    return () => window.removeEventListener('beforeunload', onLeave);
  }, [dirty]);

  useEffect(() => {
    if (!sheet && !asking) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setSheet(false);
        setAsking(false);
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [sheet, asking]);

  useEffect(() => {
    if (wide) {
      setSheet(false);
    }
  }, [wide]);

  const touched = () => {
    setNote('');
    setError(null);
  };

  const replace = (next: Block) => {
    setDraft((current) => current?.map((block) => (block.kind === next.kind ? next : block)) ?? current);
    touched();
  };

  const changeCss = (place: CssPlace, text: string) => {
    setCssDraft((current) => ({ ...current, [place]: text }));
    touched();
  };

  const toggle = (kind: BlockKind) => {
    setDraft((current) => current?.map((block) => (block.kind === kind && !pinned(kind) ? ({ ...block, on: !isOn(block) } as Block) : block)) ?? current);
    touched();
  };

  const place = (kind: BlockKind, target: (rest: Block[]) => number) => {
    setDraft((current) => {
      if (!current) {
        return current;
      }
      const from = current.findIndex((block) => block.kind === kind);
      if (from < 0) {
        return current;
      }
      const rest = [...current];
      const [moved] = rest.splice(from, 1);
      if (!moved) {
        return current;
      }
      const at = Math.min(Math.max(target(rest), 1), rest.length - 1);
      rest.splice(at, 0, moved);
      return rest;
    });
    touched();
  };

  const shift = (kind: BlockKind, delta: number) => {
    const from = draft?.findIndex((block) => block.kind === kind) ?? -1;
    if (from < 0) {
      return;
    }
    place(kind, () => from + delta);
  };

  const expand = useCallback(
    (kind: BlockKind, fromPreview = false) => {
      if (fromPreview) {
        setOpen(kind);
        window.setTimeout(() => document.getElementById(`lde-${kind}`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }), 30);
        return;
      }
      setOpen((current) => (current === kind ? null : kind));
      setFocus((current) => ({ kind, n: (current?.n ?? 0) + 1 }));
    },
    [],
  );

  const onPick = useCallback((kind: BlockKind) => expand(kind, true), [expand]);

  const expandCss = () => setOpen((current) => (current === 'css' ? null : 'css'));

  const save = async () => {
    if (!draft || !dirty || problems.length || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    setNote('');
    try {
      const view = await api<AdminLanding>('/api/admin/settings/landing', { method: 'PUT', body: { blocks: canonical(draft), css: cssOut(cssDraft) } });
      apply(view);
      setNote('Сохранено, на сайте уже новая версия');
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Не получилось сохранить');
    } finally {
      setBusy(false);
    }
  };

  const cancel = () => {
    setDraft(saved);
    setCssDraft(cssSaved);
    touched();
  };

  const resetAll = () => {
    setDraft(defaultBlocks());
    setCssDraft({});
    setAsking(false);
    setOpen(null);
    touched();
  };

  const dragStart = (kind: BlockKind) => (event: DragEvent<HTMLSpanElement>) => {
    setDrag(kind);
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/plain', kind);
    const row = event.currentTarget.closest('.lde-block');
    if (row instanceof HTMLElement) {
      event.dataTransfer.setDragImage(row, 24, 24);
    }
  };

  const dragOver = (kind: BlockKind) => (event: DragEvent<HTMLDivElement>) => {
    if (!drag || drag === kind) {
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
    const rect = event.currentTarget.getBoundingClientRect();
    const after = kind === 'hero' ? true : kind === 'footer' ? false : event.clientY > rect.top + Math.min(rect.height, 58) / 2;
    setDrop((current) => (current && current.kind === kind && current.after === after ? current : { kind, after }));
  };

  const dropOn = (kind: BlockKind) => (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    const moving = drag;
    const target = drop;
    setDrag(null);
    setDrop(null);
    if (!moving || !target || moving === kind) {
      return;
    }
    place(moving, (rest) => {
      const index = rest.findIndex((block) => block.kind === target.kind);
      return target.after ? index + 1 : index;
    });
  };

  const dragEnd = () => {
    setDrag(null);
    setDrop(null);
  };

  const previewHeight = Math.max(420, Math.min(980, viewport - 260));

  let status: ReactNode;
  if (error) {
    status = (
      <span role="status" className="bad">
        {error}
      </span>
    );
  } else if (problems.length) {
    status = (
      <span role="status" className="bad">
        {problems[0]}
        {problems.length > 1 ? ` и ещё ${count(problems.length - 1, 'ошибка', 'ошибки', 'ошибок')}` : ''}
      </span>
    );
  } else {
    status = <span role="status">{dirty ? 'Изменения появятся на сайте после сохранения' : note}</span>;
  }
  const quiet = !error && !problems.length && !dirty && !note;

  const head = (
    <div className="lde-head">
      <div className="head-stack">
        <h2 className="section-title">Лендинг</h2>
        <span className="muted">Страница сервиса на вашем домене. Тарифы и цены берутся из каталога</span>
      </div>
      <a href="/" target="_blank" rel="noopener noreferrer" className="lde-open">
        Открыть сайт
        <Icon name="external" size={16} />
      </a>
    </div>
  );

  if (loadError) {
    return (
      <SettingsLayout>
        {head}
        <div className="card card-pad">
          <span className="bad" role="alert">
            {loadError}
          </span>
        </div>
      </SettingsLayout>
    );
  }

  if (!draft) {
    return (
      <SettingsLayout>
        {head}
        <div className="card lde-list" aria-busy="true" style={{ minHeight: 320 }} />
      </SettingsLayout>
    );
  }

  const list = (
    <div className="card lde-list">
      {draft.map((block, index) => {
        const kind = block.kind;
        const isOpen = open === kind;
        const on = isOn(block);
        const movable = !pinned(kind);
        const classes = ['lde-block'];
        if (isOpen) {
          classes.push('open');
        }
        if (!on) {
          classes.push('off');
        }
        if (drag === kind) {
          classes.push('dragging');
        }
        if (drop?.kind === kind) {
          classes.push(drop.after ? 'drop-after' : 'drop-before');
        }
        const name = BLOCK_NAMES[kind];
        const own = Boolean(cssDraft[kind]?.trim());
        return (
          <div key={kind} id={`lde-${kind}`} className={classes.join(' ')} onDragOver={dragOver(kind)} onDrop={dropOn(kind)}>
            <div className="lde-row">
              {movable && !phone ? (
                <span className="lde-grip" draggable onDragStart={dragStart(kind)} onDragEnd={dragEnd} title="Перетащите, чтобы поменять порядок" aria-hidden="true">
                  <Icon name="menu" size={18} />
                </span>
              ) : (
                <span className="lde-grip-space" aria-hidden="true" />
              )}
              <button type="button" className="lde-row-main" aria-expanded={isOpen} aria-controls={`lde-${kind}-body`} onClick={() => expand(kind)}>
                <span className="lde-row-text">
                  <span className="lde-row-name">{name}</span>
                  <span className="lde-row-sum">
                    {summary(block, ctx)}
                    {own ? ' · свой CSS' : ''}
                  </span>
                </span>
                <Icon name="chevron-down" size={18} />
              </button>
              {movable ? (
                <Switch on={on} onToggle={() => toggle(kind)} label={`Показывать блок «${name}» на сайте`} />
              ) : (
                <span className="lde-pinned">{kind === 'hero' ? 'Всегда сверху' : 'Всегда внизу'}</span>
              )}
            </div>
            {isOpen ? (
              <div className="lde-body" id={`lde-${kind}-body`}>
                <BlockFields block={block} change={(next) => replace(next)} ctx={ctx} sections={sections} limits={limits} rules={rules} />
                <CssEditor
                  label="CSS блока"
                  value={cssDraft[kind] ?? ''}
                  max={limits.block_css}
                  problem={css.problems[kind] ?? null}
                  page={false}
                  rows={[4, 18]}
                  hint={`Действует только на блок «${name}»`}
                  placeholder={`${kind === 'hero' ? 'h1' : kind === 'footer' ? 'a' : 'h2'} {\n  color: #0f766e;\n}`}
                  snippets={SNIPPETS[kind]}
                  rules={rules}
                  onChange={(next) => changeCss(kind, next)}
                />
                <div className="lde-body-foot">
                  {movable ? (
                    <>
                      <button type="button" className="b36" disabled={index <= 1} onClick={() => shift(kind, -1)}>
                        <Icon name="arrow-up" size={16} />
                        Выше
                      </button>
                      <button type="button" className="b36" disabled={index >= draft.length - 2} onClick={() => shift(kind, 1)}>
                        <Icon name="arrow-down" size={16} />
                        Ниже
                      </button>
                    </>
                  ) : null}
                  {customized(block) || own ? (
                    <button
                      type="button"
                      className="lde-reset"
                      onClick={() => {
                        replace(kind === 'hero' || kind === 'footer' ? ({ kind } as Block) : ({ kind, on } as Block));
                        changeCss(kind, '');
                      }}
                    >
                      Вернуть стандартный блок
                    </button>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>
        );
      })}
      <div id="lde-css" className={open === 'css' ? 'lde-block open' : 'lde-block'}>
        <div className="lde-row">
          <span className="lde-grip-space" aria-hidden="true" />
          <button type="button" className="lde-row-main" aria-expanded={open === 'css'} aria-controls="lde-css-body" onClick={expandCss}>
            <span className="lde-row-text">
              <span className="lde-row-name">Общий CSS</span>
              <span className="lde-row-sum">{cssDraft.page?.trim() ? count(cssDraft.page.trim().split('\n').length, 'строка', 'строки', 'строк') : 'Вся страница: цвета, шрифт, кнопки'}</span>
            </span>
            <Icon name="chevron-down" size={18} />
          </button>
        </div>
        {open === 'css' ? (
          <div className="lde-body" id="lde-css-body">
            <CssEditor
              label="CSS страницы"
              value={cssDraft.page ?? ''}
              max={limits.page_css}
              problem={css.problems.page ?? null}
              page
              rows={[14, 28]}
              hint="Действует только на лендинг: кабинет, мини-приложение и админка не изменятся"
              placeholder={'.ld-btn {\n  border-radius: 22px;\n}'}
              snippets={SNIPPETS.page}
              rules={rules}
              onChange={(next) => changeCss('page', next)}
            />
          </div>
        ) : null}
      </div>
    </div>
  );

  const resetLink = (
    <button type="button" className="lde-save-reset" onClick={() => setAsking(true)} disabled={busy}>
      Вернуть стандартный лендинг
    </button>
  );

  return (
    <SettingsLayout>
      <div className="lde" ref={setWrap}>
        {head}
        {module && !module.active ? (
          <div className="card lde-off">
            <span className="lde-off-text">
              <span className="lde-off-title">Модуль «Лендинг» выключен</span>
              <span className="muted">Главная страница сайта ведёт в кабинет. Лендинг можно готовить здесь, клиенты увидят его после включения.</span>
              {enableError ? (
                <span className="err" role="alert">
                  {enableError}
                </span>
              ) : null}
            </span>
            <button type="button" className="b34 hov" onClick={() => void enableModule()} disabled={enabling}>
              {enabling ? 'Включаем…' : 'Включить'}
            </button>
          </div>
        ) : null}
        <div className={wide ? 'lde-grid' : 'lde-grid single'}>
          {list}
          {wide ? (
            <div className="card lde-pv">
              <PreviewTools device={device} theme={theme} onDevice={setDevice} onTheme={setTheme} />
              <PreviewFrame blocks={draft} css={previewCss} device={device} theme={theme} focus={focus} height={previewHeight} onPick={onPick} />
            </div>
          ) : null}
        </div>
        {phone ? <div className="lde-under">{resetLink}</div> : null}
        <div className="lde-save">
          {phone ? null : (
            <>
              <button type="button" className="btn-main" onClick={() => void save()} disabled={!dirty || busy || problems.length > 0}>
                {busy ? 'Сохраняем…' : 'Сохранить'}
              </button>
              <button type="button" className="btn-line hov" onClick={cancel} disabled={!dirty || busy}>
                Отменить
              </button>
              {wide ? null : (
                <button type="button" className="btn-line hov" onClick={() => setSheet(true)}>
                  Превью
                </button>
              )}
              {status}
              <span className="lde-grow" />
              {resetLink}
            </>
          )}
          {phone ? (
            <>
              <span className={quiet ? 'lde-save-status quiet' : 'lde-save-status'}>{status}</span>
              <span className="lde-save-btns">
                {dirty ? (
                  <button type="button" className="btn-line hov" onClick={cancel} disabled={busy}>
                    Отменить
                  </button>
                ) : null}
                <button type="button" className="btn-line hov" onClick={() => setSheet(true)}>
                  Превью
                </button>
                <button type="button" className="btn-main" onClick={() => void save()} disabled={!dirty || busy || problems.length > 0}>
                  {busy ? 'Сохраняем…' : 'Сохранить'}
                </button>
              </span>
            </>
          ) : null}
        </div>
      </div>
      {sheet ? (
        <div className="lde-sheet" role="dialog" aria-modal="true" aria-label="Превью лендинга">
          <div className="lde-sheet-head">
            <span className="lde-sheet-title">Превью</span>
            <button type="button" className="lde-icon hov" aria-label="Закрыть превью" onClick={() => setSheet(false)} autoFocus>
              <Icon name="close" size={20} />
            </button>
            <PreviewTools device={device} theme={theme} onDevice={setDevice} onTheme={setTheme} />
          </div>
          <div className="lde-sheet-body" ref={setSheetBody}>
            {sheetSize.height > 0 ? (
              <PreviewFrame
                blocks={draft}
                css={previewCss}
                device={device}
                theme={theme}
                focus={focus}
                height={sheetSize.height}
                onPick={(kind) => {
                  setSheet(false);
                  expand(kind, true);
                }}
              />
            ) : null}
          </div>
        </div>
      ) : null}
      {asking ? (
        <div className="dialog-back" role="presentation" onClick={() => setAsking(false)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="lde-reset-h" onClick={(event) => event.stopPropagation()}>
            <h2 id="lde-reset-h">Вернуть стандартный лендинг?</h2>
            <p className="muted">Тексты, порядок и видимость блоков станут как при установке, свои картинки и свой CSS уйдут со страницы. На сайте это появится после сохранения.</p>
            <div className="dialog-actions">
              <button type="button" className="btn-main" onClick={resetAll} autoFocus>
                Вернуть
              </button>
              <button type="button" className="btn-line hov" onClick={() => setAsking(false)}>
                Отменить
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </SettingsLayout>
  );
}
