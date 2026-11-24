import { useEffect, useId, useRef, useState, type DragEvent, type ReactNode } from 'react';
import { ApiError, upload } from '../../lib/api';
import type { Lang } from '../../lib/i18n';
import { Icon } from '../../lib/icons';
import { AUDIENCES, GLYPHS, imageUrl, isSection, TARGET_KEYS, type Audience, type Glyph, type ImageKind, type MiniImage, type Phrase, type SectionKey } from '../../mini/config';
import { AUDIENCE_NAMES, chars, dropIndex, normalizeUrl, SECTION_TITLES, setPhrase, urlProblem, type ImageRules, type SectionInfo } from './model';

type Uploaded = { tag: string; kind: ImageKind; width: number; height: number; size: number; url: string };

const GLYPH_NAMES: Record<Glyph, string> = {
  send: 'Самолётик',
  globe: 'Глобус',
  chat: 'Сообщение',
  mail: 'Письмо',
  book: 'Книга',
  file: 'Документ',
  gift: 'Подарок',
  percent: 'Процент',
  trophy: 'Кубок',
  zap: 'Молния',
  flag: 'Флаг',
  phone: 'Телефон',
};

export function weight(size: number): string {
  if (size >= 1024 * 1024) {
    return `${(size / 1024 / 1024).toFixed(1).replace('.', ',').replace(',0', '')} МБ`;
  }
  return `${Math.max(1, Math.round(size / 1024))} КБ`;
}

export function Field({ label, hint, problem, id, children }: { label: string; hint?: ReactNode; problem?: string | null; id?: string; children: ReactNode }) {
  return (
    <div className="mce-field">
      {id ? (
        <label htmlFor={id} className="mce-label">
          {label}
        </label>
      ) : (
        <span className="mce-label">{label}</span>
      )}
      {children}
      {problem ? (
        <span id={id ? `${id}-n` : undefined} className="err">
          {problem}
        </span>
      ) : hint ? (
        <span id={id ? `${id}-n` : undefined} className="cap">
          {hint}
        </span>
      ) : null}
    </div>
  );
}

type PhraseProps = {
  label: string;
  value: Phrase | undefined;
  lang: Lang;
  max: number;
  onChange: (next: Phrase | undefined) => void;
  standard?: string | null;
  multiline?: boolean;
  rows?: number;
  hint?: ReactNode;
  placeholder?: string;
};

export function PhraseField({ label, value, lang, max, onChange, standard = null, multiline = false, rows = 3, hint, placeholder }: PhraseProps) {
  const id = useId();
  const own = value?.[lang];
  const shown = own ?? standard ?? '';
  const length = chars(shown);
  const problem = length > max ? `Не длиннее ${max} символов, сейчас ${length}` : null;
  const fallback = standard ?? (lang === 'en' ? (value?.ru ?? '') : '');
  const set = (next: string) => onChange(setPhrase(value, lang, next === standard ? null : next));
  const note =
    hint ??
    (lang === 'en' && standard === null
      ? value?.ru
        ? 'Пусто — англоязычный клиент увидит русский текст'
        : undefined
      : undefined);
  const common = {
    id,
    className: 'field',
    value: shown,
    placeholder: fallback || placeholder,
    'aria-invalid': problem ? true : undefined,
    'aria-describedby': problem || note ? `${id}-n` : undefined,
  } as const;
  return (
    <Field label={label} id={id} hint={note} problem={problem}>
      {multiline ? <textarea {...common} rows={rows} onChange={(e) => set(e.target.value)} /> : <input {...common} onChange={(e) => set(e.target.value)} />}
    </Field>
  );
}

type TargetProps = {
  label: string;
  to: string | null | undefined;
  onChange: (next: string | null) => void;
  sections: SectionInfo[];
  max: number;
  none?: string;
};

export function TargetField({ label, to, onChange, sections, max, none }: TargetProps) {
  const id = useId();
  const outer = typeof to === 'string' && !isSection(to) ? to : null;
  const [raw, setRaw] = useState(outer ?? '');
  useEffect(() => {
    if (outer !== null) {
      setRaw((current) => (normalizeUrl(current) === outer ? current : outer));
    }
  }, [outer]);
  const choice = isSection(to) ? to : outer !== null ? 'url' : 'none';
  const problem = outer !== null ? urlProblem(raw, max) : choice === 'none' && !none ? 'укажите, куда ведёт' : null;
  const off = isSection(to) ? (sections.find((section) => section.key === to)?.off ?? null) : null;
  const pick = (value: string) => {
    if (value === 'url') {
      onChange(normalizeUrl(raw));
    } else {
      onChange(value === 'none' ? null : value);
    }
  };
  return (
    <Field label={label} id={id} problem={problem ? problem.charAt(0).toUpperCase() + problem.slice(1) : null} hint={off ? `Сейчас не показывается: ${off.charAt(0).toLowerCase()}${off.slice(1)}` : undefined}>
      <select id={id} className="field" value={choice} aria-describedby={problem || off ? `${id}-n` : undefined} onChange={(e) => pick(e.target.value)}>
        {none || choice === 'none' ? <option value="none">{none ?? 'Выберите'}</option> : null}
        {TARGET_KEYS.map((key) => (
          <option key={key} value={key}>
            Раздел «{SECTION_TITLES[key]}»
          </option>
        ))}
        <option value="url">Своя ссылка</option>
      </select>
      {outer !== null ? (
        <input
          className="field mono"
          value={raw}
          placeholder="https://… или t.me/…"
          aria-label={`${label}: адрес`}
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          aria-invalid={problem ? true : undefined}
          onChange={(e) => {
            setRaw(e.target.value);
            onChange(normalizeUrl(e.target.value));
          }}
        />
      ) : null}
    </Field>
  );
}

export function AudienceField({ value, onChange }: { value: Audience[] | undefined; onChange: (next: Audience[] | undefined) => void }) {
  const kept = value && value.length ? AUDIENCES.filter((key) => value.includes(key)) : AUDIENCES;
  const toggle = (key: Audience) => {
    const next = kept.includes(key) ? kept.filter((item) => item !== key) : AUDIENCES.filter((item) => item === key || kept.includes(item));
    if (next.length) {
      onChange(next.length === AUDIENCES.length ? undefined : next);
    }
  };
  return (
    <div className="mce-field">
      <span className="mce-label">Кому показывать</span>
      <span className="mce-chips" role="group" aria-label="Кому показывать">
        {AUDIENCES.map((key) => (
          <button key={key} type="button" className="mce-chip" aria-pressed={kept.includes(key)} onClick={() => toggle(key)}>
            {AUDIENCE_NAMES[key]}
          </button>
        ))}
      </span>
    </div>
  );
}

export function GlyphField({ value, shown, onChange }: { value: Glyph | undefined; shown: string; onChange: (next: Glyph) => void }) {
  return (
    <div className="mce-field">
      <span className="mce-label">Значок</span>
      <span className="mce-chips" role="radiogroup" aria-label="Значок">
        {GLYPHS.map((name) => (
          <button key={name} type="button" role="radio" className="mce-chip icon" aria-checked={(value ?? shown) === name} aria-label={GLYPH_NAMES[name]} title={GLYPH_NAMES[name]} onClick={() => onChange(name)}>
            <Icon name={name} size={18} />
          </button>
        ))}
      </span>
    </div>
  );
}

type PictureProps = { image: MiniImage | null | undefined; onChange: (next: MiniImage | null) => void; rules: ImageRules; label: string; hint: string; what: string };

export function PictureField({ image, onChange, rules, label, hint, what }: PictureProps) {
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
      const saved = await upload<Uploaded>('/api/admin/settings/miniapp/images', new Blob([file], { type: file.type }));
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
    <div className="mce-field">
      <span className="mce-label">{label}</span>
      <div className="mce-image">
        {src ? <img className="mce-thumb" src={src} alt="" draggable={false} /> : null}
        <div className="mce-image-side">
          <span className="mce-btns">
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
    </div>
  );
}

export type DragOrder = {
  grip: (index: number) => { draggable: true; onDragStart: (event: DragEvent<HTMLElement>) => void; onDragEnd: () => void };
  row: (index: number) => { onDragOver: (event: DragEvent<HTMLElement>) => void; onDrop: (event: DragEvent<HTMLElement>) => void };
  mark: (index: number) => string;
};

export function useDragOrder(kind: string, onMove: (from: number, to: number) => void): DragOrder {
  const [drag, setDrag] = useState<number | null>(null);
  const [drop, setDrop] = useState<{ index: number; after: boolean } | null>(null);
  const end = () => {
    setDrag(null);
    setDrop(null);
  };
  return {
    grip: (index) => ({
      draggable: true,
      onDragStart: (event) => {
        event.stopPropagation();
        event.dataTransfer.effectAllowed = 'move';
        event.dataTransfer.setData('text/plain', `${kind}:${index}`);
        const row = event.currentTarget.closest('[data-drag]');
        if (row instanceof HTMLElement) {
          event.dataTransfer.setDragImage(row, 24, 24);
        }
        setDrag(index);
      },
      onDragEnd: end,
    }),
    row: (index) => ({
      onDragOver: (event) => {
        if (drag === null || drag === index) {
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        event.dataTransfer.dropEffect = 'move';
        const box = event.currentTarget.getBoundingClientRect();
        const after = event.clientY > box.top + Math.min(box.height, 58) / 2;
        setDrop((current) => (current && current.index === index && current.after === after ? current : { index, after }));
      },
      onDrop: (event) => {
        if (drag === null) {
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        if (drop && drag !== drop.index) {
          const to = dropIndex(drag, drop.index, drop.after);
          if (to !== drag) {
            onMove(drag, to);
          }
        }
        end();
      },
    }),
    mark: (index) => `${drag === index ? ' dragging' : ''}${drop?.index === index ? (drop.after ? ' drop-after' : ' drop-before') : ''}`,
  };
}

export function Grip({ order, index, phone, name }: { order: DragOrder; index: number; phone: boolean; name: string }) {
  if (phone) {
    return null;
  }
  return (
    <span className="mce-grip" {...order.grip(index)} title={`Перетащите «${name}», чтобы поменять порядок`} aria-hidden="true">
      <Icon name="menu" size={18} />
    </span>
  );
}

export function MoveButtons({ name, first, last, onUp, onDown }: { name: string; first: boolean; last: boolean; onUp: () => void; onDown: () => void }) {
  return (
    <>
      <button type="button" className="b36" disabled={first} onClick={onUp} aria-label={`${name}: выше`}>
        <Icon name="arrow-up" size={16} />
        Выше
      </button>
      <button type="button" className="b36" disabled={last} onClick={onDown} aria-label={`${name}: ниже`}>
        <Icon name="arrow-down" size={16} />
        Ниже
      </button>
    </>
  );
}

export function AddSelect({ label, options, onPick }: { label: string; options: Array<{ key: string; name: string }>; onPick: (key: string) => void }) {
  if (options.length === 0) {
    return null;
  }
  return (
    <select className="field mce-add-select" value="" aria-label={label} onChange={(e) => e.target.value && onPick(e.target.value)}>
      <option value="">{label}</option>
      {options.map((option) => (
        <option key={option.key} value={option.key}>
          {option.name}
        </option>
      ))}
    </select>
  );
}

export function sectionOff(sections: SectionInfo[], key: SectionKey): string | null {
  return sections.find((section) => section.key === key)?.off ?? null;
}
