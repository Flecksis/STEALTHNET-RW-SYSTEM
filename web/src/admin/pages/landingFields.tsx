import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { ApiError, upload } from '../../lib/api';
import { Icon } from '../../lib/icons';
import {
  BLOCK_NAMES,
  defaultItems,
  imageUrl,
  itemContent,
  linkTarget,
  SECTION_KINDS,
  stdItem,
  type ImageKind,
  type ItemKind,
  type LandingContext,
  type LandingImage,
  type LandingItem,
  type LandingLink,
  type SectionKind,
} from '../../site/landingModel';

export type Limits = {
  title: number;
  text: number;
  answer: number;
  note: number;
  plans_text: number;
  label: number;
  menu: number;
  app: number;
  footer_text: number;
  url: number;
  features: number;
  steps: number;
  faq: number;
  apps: number;
  hero_buttons: number;
  cta_buttons: number;
  footer_links: number;
};

export const LIMITS: Limits = {
  title: 120,
  text: 600,
  answer: 1500,
  note: 200,
  plans_text: 300,
  label: 40,
  menu: 40,
  app: 40,
  footer_text: 200,
  url: 500,
  features: 6,
  steps: 6,
  faq: 20,
  apps: 12,
  hero_buttons: 2,
  cta_buttons: 2,
  footer_links: 8,
};

export type ImageRules = { max_bytes: number; min_side: number; max_side: number; types: string[] };

export const IMAGE_RULES: ImageRules = {
  max_bytes: 1024 * 1024,
  min_side: 100,
  max_side: 4096,
  types: ['image/png', 'image/jpeg', 'image/webp'],
};

type Uploaded = { tag: string; kind: ImageKind; width: number; height: number; size: number; url: string };

export function chars(value: string): number {
  return [...value].length;
}

export function weight(size: number): string {
  if (size >= 1024 * 1024) {
    return `${(size / 1024 / 1024).toFixed(1).replace('.', ',').replace(',0', '')} МБ`;
  }
  return `${Math.max(1, Math.round(size / 1024))} КБ`;
}

export function textProblem(value: string, max: number, required: boolean): string | null {
  if (required && !value.trim()) {
    return 'Нужно заполнить';
  }
  const length = chars(value);
  if (length > max) {
    return `Не длиннее ${max} символов, сейчас ${length}`;
  }
  return null;
}

export function normalizeUrl(raw: string): string {
  const value = raw.trim();
  if (/^t\.me\//i.test(value)) {
    return `https://t.me/${value.slice(5)}`;
  }
  return value;
}

export function urlProblem(raw: string, max: number): string | null {
  const url = normalizeUrl(raw);
  if (!url) {
    return 'Укажите ссылку';
  }
  if (chars(url) > max) {
    return `Не длиннее ${max} символов`;
  }
  const scheme = /^https?:\/\//i.exec(url);
  if (!scheme) {
    return 'Нужна ссылка вида https://… или t.me/…';
  }
  if (/[\s"'<>`\\{}|^\u0000-\u001f\u007f]/.test(url)) {
    return 'В ссылке не должно быть пробелов, кавычек и скобок';
  }
  const rest = url.slice(scheme[0].length);
  const end = rest.search(/[/?#]/);
  const authority = end < 0 ? rest : rest.slice(0, end);
  const colon = authority.lastIndexOf(':');
  const host = colon >= 0 ? authority.slice(0, colon) : authority;
  const port = colon >= 0 ? authority.slice(colon + 1) : null;
  const hostOk = /^[\p{L}\p{N}.-]+$/u.test(host) && /[\p{L}\p{N}]/u.test(host) && !/^[.-]/.test(host) && !/[.-]$/.test(host);
  if (!hostOk || (port !== null && !/^\d{1,5}$/.test(port))) {
    return 'В ссылке нет адреса сайта';
  }
  return null;
}

const NAMED: Array<[string, string]> = [
  ['cabinet', 'Личный кабинет'],
  ['bot', 'Telegram-бот'],
  ['support', 'Поддержка'],
  ...SECTION_KINDS.map((kind): [string, string] => [`#${kind}`, `Блок «${BLOCK_NAMES[kind]}»`]),
];

function isNamed(to: string): boolean {
  return NAMED.some(([key]) => key === to);
}

export function linkProblem(link: LandingLink, limits: Limits): string | null {
  const label = textProblem(link.label, limits.label, true);
  if (label) {
    return label === 'Нужно заполнить' ? 'Нужен текст кнопки' : label;
  }
  if (isNamed(link.to)) {
    return null;
  }
  return urlProblem(link.to, limits.url);
}

type FieldProps = {
  label: string;
  value: string | null | undefined;
  fallback: string;
  max: number;
  multiline?: boolean;
  rows?: number;
  required?: boolean;
  hint?: ReactNode;
  placeholder?: string;
  onChange: (next: string | null) => void;
};

export function TextField({ label, value, fallback, max, multiline = false, rows = 3, required = false, hint, placeholder, onChange }: FieldProps) {
  const id = useId();
  const shown = value ?? fallback;
  const problem = textProblem(shown, max, required);
  const set = (next: string) => onChange(next === fallback ? null : next);
  const note = problem ? (
    <span id={`${id}-n`} className="err">
      {problem}
    </span>
  ) : hint ? (
    <span id={`${id}-n`} className="cap">
      {hint}
    </span>
  ) : null;
  return (
    <div className="lde-field">
      <label htmlFor={id} className="lde-label">
        {label}
      </label>
      {multiline ? (
        <textarea
          id={id}
          className="field"
          rows={rows}
          value={shown}
          placeholder={placeholder}
          aria-invalid={problem ? true : undefined}
          aria-describedby={note ? `${id}-n` : undefined}
          onChange={(e) => set(e.target.value)}
        />
      ) : (
        <input
          id={id}
          className="field"
          value={shown}
          placeholder={placeholder}
          aria-invalid={problem ? true : undefined}
          aria-describedby={note ? `${id}-n` : undefined}
          onChange={(e) => set(e.target.value)}
        />
      )}
      {note}
    </div>
  );
}

type LinkProps = {
  link: LandingLink | null;
  onChange: (next: LandingLink | null) => void;
  onRemove?: () => void;
  allowNone?: boolean;
  ctx: LandingContext;
  sections: Set<SectionKind>;
  limits: Limits;
  what: string;
};

export function LinkEditor({ link, onChange, onRemove, allowNone = false, ctx, sections, limits, what }: LinkProps) {
  const id = useId();
  const custom = link !== null && !isNamed(link.to);
  const [raw, setRaw] = useState(custom && link ? link.to : '');
  const outer = custom && link ? link.to : null;
  useEffect(() => {
    if (outer !== null) {
      setRaw((current) => (normalizeUrl(current) === outer ? current : outer));
    }
  }, [outer]);
  const choice = link === null ? 'none' : custom ? 'url' : link.to;
  const problem = link ? linkProblem(link, limits) : null;
  const target = link && !problem ? linkTarget(link.to, ctx, sections) : null;
  const pick = (value: string) => {
    if (value === 'none') {
      onChange(null);
      return;
    }
    const label = link?.label ?? '';
    onChange({ label, to: value === 'url' ? normalizeUrl(raw) : value });
  };
  return (
    <div className="lde-link">
      <div className={link ? 'lde-link-row' : 'lde-link-row none'}>
        {link ? (
          <input
            className="field"
            value={link.label}
            aria-label={`${what}: текст`}
            aria-invalid={problem && !link.label.trim() ? true : undefined}
            placeholder="Текст"
            onChange={(e) => onChange({ ...link, label: e.target.value })}
          />
        ) : null}
        <select className="field" value={choice} aria-label={`${what}: куда ведёт`} onChange={(e) => pick(e.target.value)}>
          {allowNone ? <option value="none">Без ссылки</option> : null}
          {NAMED.map(([key, name]) => (
            <option key={key} value={key}>
              {name}
            </option>
          ))}
          <option value="url">Своя ссылка</option>
        </select>
        {onRemove ? (
          <button type="button" className="lde-icon hov" aria-label={`Убрать: ${what.toLowerCase()}`} onClick={onRemove}>
            <Icon name="close" size={16} />
          </button>
        ) : null}
      </div>
      {link && custom ? (
        <input
          id={id}
          className="field mono"
          value={raw}
          placeholder="https://… или t.me/…"
          aria-label={`${what}: ссылка`}
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          aria-invalid={problem && link.label.trim() ? true : undefined}
          onChange={(e) => {
            setRaw(e.target.value);
            onChange({ ...link, to: normalizeUrl(e.target.value) });
          }}
        />
      ) : null}
      {problem ? <span className="err">{problem}</span> : target && !target.ok ? <span className="cap">Сейчас не видна на сайте: {target.reason}</span> : null}
    </div>
  );
}

type LinksProps = {
  links: LandingLink[];
  onChange: (next: LandingLink[]) => void;
  max: number;
  addLabel: string;
  what: string;
  ctx: LandingContext;
  sections: Set<SectionKind>;
  limits: Limits;
};

export function LinksEditor({ links, onChange, max, addLabel, what, ctx, sections, limits }: LinksProps) {
  return (
    <div className="lde-links">
      {links.map((link, index) => (
        <LinkEditor
          key={index}
          link={link}
          what={`${what} ${index + 1}`}
          ctx={ctx}
          sections={sections}
          limits={limits}
          onChange={(next) => next && onChange(links.map((l, i) => (i === index ? next : l)))}
          onRemove={() => onChange(links.filter((_, i) => i !== index))}
        />
      ))}
      {links.length < max ? (
        <button type="button" className="lde-add" onClick={() => onChange([...links, { label: '', to: 'cabinet' }])}>
          {addLabel}
        </button>
      ) : null}
    </div>
  );
}

type ImageProps = {
  image: LandingImage | null | undefined;
  onChange: (next: LandingImage | null) => void;
  rules: ImageRules;
  hint: string;
  what: string;
};

export function ImageField({ image, onChange, rules, hint, what }: ImageProps) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const src = imageUrl(image);

  const send = async (file: File | undefined) => {
    if (!file) {
      return;
    }
    if (!rules.types.includes(file.type)) {
      setError('Подходят только PNG, JPEG и WebP');
      return;
    }
    if (file.size > rules.max_bytes) {
      setError(`Файл больше ${weight(rules.max_bytes)}: уменьшите картинку`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const saved = await upload<Uploaded>('/api/admin/settings/landing/images', new Blob([file], { type: file.type }));
      onChange({ tag: saved.tag, kind: saved.kind, width: saved.width, height: saved.height, url: saved.url });
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Картинка не загрузилась');
    } finally {
      setBusy(false);
      if (input.current) {
        input.current.value = '';
      }
    }
  };

  return (
    <div className="lde-image">
      {src ? <img className="lde-thumb" src={src} alt="" draggable={false} /> : null}
      <div className="lde-image-side">
        <span className="lde-btns">
          <button type="button" className="b36" onClick={() => input.current?.click()} disabled={busy} aria-label={src ? `Заменить картинку: ${what}` : `Загрузить картинку: ${what}`}>
            {busy ? 'Загружаем…' : src ? 'Заменить' : 'Загрузить'}
          </button>
          {src ? (
            <button type="button" className="b36" onClick={() => onChange(null)} disabled={busy} aria-label={`Убрать картинку: ${what}`}>
              Убрать
            </button>
          ) : null}
        </span>
        <span className="cap">
          {image?.width && image.height ? `${image.width}×${image.height}. ` : ''}
          {hint}
        </span>
        {error ? (
          <span className="err" role="alert">
            {error}
          </span>
        ) : null}
      </div>
      <input ref={input} type="file" accept={rules.types.join(',')} hidden onChange={(e) => void send(e.target.files?.[0])} />
    </div>
  );
}

const STD_NOTES: Record<string, string> = {
  'features.bot': 'Виден, когда подключён бот',
  'steps.pay': 'Способы оплаты подставляются по валюте',
  'faq.devices': 'Число устройств и цена берутся из тарифа',
  'faq.renewal': 'Текст зависит от валюты',
};

function desktop(ctx: LandingContext): LandingContext {
  return ctx.phone ? { ...ctx, phone: false } : ctx;
}

export function materialize(kind: ItemKind, item: LandingItem, ctx: LandingContext): LandingItem {
  if (!item.std) {
    return item;
  }
  const std = stdItem(kind, item.std, desktop(ctx));
  const out: LandingItem = { title: std?.title ?? '', text: std?.text ?? '' };
  if (kind === 'features' && std?.link) {
    out.link = std.link;
  }
  return out;
}

export function sameItems(kind: ItemKind, items: LandingItem[]): boolean {
  const defaults = defaultItems(kind);
  return items.length === defaults.length && items.every((item, i) => item.std !== undefined && item.std === defaults[i]?.std);
}

export function itemProblem(kind: ItemKind, item: LandingItem, limits: Limits): string | null {
  if (item.std) {
    return null;
  }
  const title = textProblem(item.title ?? '', limits.title, true);
  if (title) {
    return `${kind === 'faq' ? 'вопрос' : 'заголовок'}: ${title.toLowerCase()}`;
  }
  const body = textProblem(item.text ?? '', kind === 'faq' ? limits.answer : limits.text, kind === 'faq');
  if (body) {
    return `${kind === 'faq' ? 'ответ' : 'текст'}: ${body.toLowerCase()}`;
  }
  if (kind === 'features' && item.link) {
    const link = linkProblem(item.link, limits);
    if (link) {
      return `ссылка: ${link.toLowerCase()}`;
    }
  }
  return null;
}

type ItemsProps = {
  kind: ItemKind;
  items: LandingItem[] | null | undefined;
  onChange: (next: LandingItem[] | null) => void;
  ctx: LandingContext;
  sections: Set<SectionKind>;
  limits: Limits;
  rules: ImageRules;
};

const ITEM_WORDS: Record<ItemKind, { one: string; add: string }> = {
  features: { one: 'Пункт', add: 'Добавить пункт' },
  steps: { one: 'Шаг', add: 'Добавить шаг' },
  faq: { one: 'Вопрос', add: 'Добавить вопрос' },
};

export function ItemsEditor({ kind, items, onChange, ctx, sections, limits, rules }: ItemsProps) {
  const list = items ?? defaultItems(kind);
  const [open, setOpen] = useState<number | null>(null);
  const max = kind === 'features' ? limits.features : kind === 'steps' ? limits.steps : limits.faq;
  const words = ITEM_WORDS[kind];

  const commit = (next: LandingItem[]) => onChange(sameItems(kind, next) ? null : next);
  const edit = (index: number, patch: Partial<LandingItem>) => {
    commit(
      list.map((item, i) => {
        if (i !== index) {
          return item;
        }
        const base = materialize(kind, item, ctx);
        const next: LandingItem = { ...base, ...patch };
        if (next.link === null) {
          delete next.link;
        }
        if (next.image === null) {
          delete next.image;
        }
        return next;
      }),
    );
  };
  const move = (index: number, delta: number) => {
    const to = index + delta;
    if (to < 0 || to >= list.length) {
      return;
    }
    const next = [...list];
    const [moved] = next.splice(index, 1);
    if (moved) {
      next.splice(to, 0, moved);
    }
    commit(next);
    setOpen((current) => (current === index ? to : current === to ? index : current));
  };
  const remove = (index: number) => {
    commit(list.filter((_, i) => i !== index));
    setOpen(null);
  };
  const add = () => {
    commit([...list, { title: '', text: '' }]);
    setOpen(list.length);
  };

  return (
    <div className="lde-items">
      {list.map((item, index) => {
        const content = itemContent(kind, item, desktop(ctx));
        const problem = itemProblem(kind, item, limits);
        const target = content.link ? linkTarget(content.link.to, ctx, sections) : null;
        const hidden = target && !target.ok ? `Не видно на сайте: ${target.reason}` : null;
        const note = hidden ?? (item.std ? STD_NOTES[`${kind}.${item.std}`] : null) ?? null;
        const expanded = open === index;
        const name = content.title.trim() || `${words.one} ${index + 1}`;
        return (
          <div key={index} className={expanded ? 'lde-item open' : 'lde-item'}>
            <div className="lde-item-head">
              <button type="button" className="lde-item-name" aria-expanded={expanded} onClick={() => setOpen(expanded ? null : index)}>
                <span className="lde-item-title">{name}</span>
                {problem ? <span className="err">{problem}</span> : note ? <span className="cap">{note}</span> : null}
              </button>
              <span className="lde-item-tools">
                <button type="button" className="lde-icon hov" aria-label={`${name}: выше`} disabled={index === 0} onClick={() => move(index, -1)}>
                  <Icon name="arrow-up" size={16} />
                </button>
                <button type="button" className="lde-icon hov" aria-label={`${name}: ниже`} disabled={index === list.length - 1} onClick={() => move(index, 1)}>
                  <Icon name="arrow-down" size={16} />
                </button>
                <button type="button" className="lde-icon hov" aria-label={`${name}: удалить`} onClick={() => remove(index)}>
                  <Icon name="close" size={16} />
                </button>
              </span>
            </div>
            {expanded ? (
              <div className="lde-item-body">
                <TextField
                  label={kind === 'faq' ? 'Вопрос' : 'Заголовок'}
                  value={content.title}
                  fallback=""
                  max={limits.title}
                  required
                  onChange={(next) => edit(index, { title: next ?? '' })}
                />
                <TextField
                  label={kind === 'faq' ? 'Ответ' : 'Текст'}
                  value={content.text}
                  fallback=""
                  max={kind === 'faq' ? limits.answer : limits.text}
                  multiline
                  rows={kind === 'faq' ? 4 : 3}
                  required={kind === 'faq'}
                  onChange={(next) => edit(index, { text: next ?? '' })}
                />
                {kind === 'features' ? (
                  <>
                    <div className="lde-field">
                      <span className="lde-label">Ссылка</span>
                      <LinkEditor
                        link={content.link}
                        allowNone
                        what="Ссылка пункта"
                        ctx={ctx}
                        sections={sections}
                        limits={limits}
                        onChange={(next) => edit(index, { link: next })}
                      />
                    </div>
                    <div className="lde-field">
                      <span className="lde-label">Картинка</span>
                      <ImageField
                        image={content.image}
                        rules={rules}
                        what={name}
                        hint="Необязательно. Лучше горизонтальная 16:10, от 800 точек в ширину"
                        onChange={(next) => edit(index, { image: next })}
                      />
                    </div>
                  </>
                ) : null}
              </div>
            ) : null}
          </div>
        );
      })}
      {list.length < max ? (
        <button type="button" className="lde-add" onClick={add}>
          {words.add}
        </button>
      ) : (
        <span className="cap">Больше {max} не поместится</span>
      )}
    </div>
  );
}
