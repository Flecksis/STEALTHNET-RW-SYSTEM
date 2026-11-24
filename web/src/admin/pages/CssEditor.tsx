import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { api, ApiError } from '../../lib/api';
import { closeBrace, CSS_PLACES, insertImage, insertRule, lengthProblem, lineRange, newLine, problemText, shiftLines, type CssPlace, type CssProblem, type CssProblems, type CssReport, type CssTexts, type Edit } from './landingCss';
import { chars, sendImage, type ImageRules } from './landingFields';
import type { Snippet } from './landingSnippets';
import './css-editor.css';

const CHECK_PATH = '/api/admin/settings/landing/css';
const CHECK_DELAY_MS = 350;

type CssCheck = { styles: CssTexts; problems: CssProblems; reset: (texts: CssTexts, styles: CssTexts) => void };

function without<T>(current: Partial<Record<CssPlace, T>>, places: CssPlace[]): Partial<Record<CssPlace, T>> {
  const next = { ...current };
  for (const place of places) {
    delete next[place];
  }
  return next;
}

export function useCssCheck(texts: CssTexts): CssCheck {
  const [styles, setStyles] = useState<CssTexts>({});
  const [problems, setProblems] = useState<CssProblems>({});
  const [checked, setChecked] = useState<CssTexts>({});

  useEffect(() => {
    const places = CSS_PLACES.filter((place) => (texts[place] ?? '') !== (checked[place] ?? ''));
    if (!places.length) {
      return;
    }
    let alive = true;
    const filled = places.filter((place) => texts[place]?.trim());
    const settle = (report: CssReport) => {
      if (!alive) {
        return;
      }
      setStyles((current) => {
        const next = without(current, places);
        for (const place of filled) {
          next[place] = report[place]?.style ?? current[place] ?? '';
        }
        return next;
      });
      setProblems((current) => {
        const next = without(current, places);
        for (const place of filled) {
          const entry = report[place];
          if (entry?.problem) {
            next[place] = { line: entry.line ?? null, text: entry.problem };
          }
        }
        return next;
      });
      setChecked((current) => {
        const next = without(current, places);
        for (const place of places) {
          next[place] = texts[place] ?? '';
        }
        return next;
      });
    };
    const timer = window.setTimeout(
      () => {
        if (!filled.length) {
          settle({});
          return;
        }
        const css = Object.fromEntries(filled.map((place) => [place, texts[place]]));
        api<CssReport>(CHECK_PATH, { method: 'POST', body: { css } })
          .then(settle)
          .catch(() => undefined);
      },
      filled.length ? CHECK_DELAY_MS : 0,
    );
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [texts, checked]);

  const reset = useCallback((nextTexts: CssTexts, nextStyles: CssTexts) => {
    const refused = CSS_PLACES.filter((place) => nextTexts[place] && !nextStyles[place]);
    setChecked(without(nextTexts, refused));
    setStyles(nextStyles);
    setProblems({});
  }, []);

  return { styles, problems, reset };
}

type Props = {
  label: string;
  value: string;
  max: number;
  problem: CssProblem | null;
  page: boolean;
  rows: [number, number];
  hint?: ReactNode;
  placeholder: string;
  snippets: Snippet[];
  rules: ImageRules;
  onChange: (next: string) => void;
};

function applyEdit(area: HTMLTextAreaElement, edit: Edit): void {
  area.focus();
  area.setSelectionRange(edit.from, edit.to);
  if (!document.execCommand('insertText', false, edit.text)) {
    area.setRangeText(edit.text, edit.from, edit.to, 'end');
    area.dispatchEvent(new Event('input', { bubbles: true }));
  }
  area.setSelectionRange(edit.anchor ?? edit.caret, edit.caret);
}

export function CssEditor({ label, value, max, problem, page, rows, hint, placeholder, snippets, rules, onChange }: Props) {
  const id = useId();
  const area = useRef<HTMLTextAreaElement>(null);
  const numbers = useRef<HTMLDivElement>(null);
  const file = useRef<HTMLInputElement>(null);
  const released = useRef(false);
  const entered = useRef(false);
  const [busy, setBusy] = useState(false);
  const [imageError, setImageError] = useState<string | null>(null);

  const length = chars(value);
  const shown = lengthProblem(value, max) ?? problem;
  const lines = value.split('\n').length;
  const bad = shown?.line ?? 0;
  const numbered = Array.from({ length: lines }, (_, i) => i + 1);
  const height = Math.min(Math.max(lines, rows[0]), rows[1]);

  const reset = () => {
    onChange('');
    area.current?.focus();
  };

  const syncScroll = () => {
    if (area.current && numbers.current) {
      numbers.current.scrollTop = area.current.scrollTop;
    }
  };

  const caret = () => (entered.current && area.current ? area.current.selectionStart : value.length);

  const run = (edit: Edit | null): boolean => {
    if (!area.current || !edit) {
      return false;
    }
    applyEdit(area.current, edit);
    return true;
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing || event.ctrlKey || event.metaKey || event.altKey) {
      return;
    }
    const { selectionStart: from, selectionEnd: to } = event.currentTarget;
    if (event.key === 'Escape') {
      released.current = true;
      return;
    }
    if (event.key === 'Tab' && !released.current) {
      event.preventDefault();
      run(shiftLines(value, from, to, event.shiftKey));
    } else if (event.key === 'Enter') {
      event.preventDefault();
      run(newLine(value, from, to));
    } else if (event.key === '}' && from === to && run(closeBrace(value, from, to))) {
      event.preventDefault();
    }
    released.current = false;
  };

  const jump = () => {
    if (!area.current || !shown?.line) {
      return;
    }
    const range = lineRange(value, shown.line);
    area.current.focus();
    area.current.setSelectionRange(range.from, range.to);
    const lineHeight = Number.parseFloat(getComputedStyle(area.current).lineHeight) || 20;
    area.current.scrollTop = Math.max(0, (shown.line - 3) * lineHeight);
    area.current.scrollIntoView({ block: 'nearest' });
  };

  const send = async (picked: File | undefined) => {
    if (!picked) {
      return;
    }
    setBusy(true);
    setImageError(null);
    try {
      const saved = await sendImage(picked, rules);
      if (area.current) {
        run(insertImage(area.current.value, entered.current ? area.current.selectionStart : area.current.value.length, saved.url, page));
      }
    } catch (err) {
      setImageError(err instanceof ApiError ? err.message : 'Картинка не загрузилась');
    } finally {
      setBusy(false);
      if (file.current) {
        file.current.value = '';
      }
    }
  };

  return (
    <div className="lcs">
      <div className="lcs-head">
        <label htmlFor={id} className="lde-label">
          {label}
        </label>
        <span className="lcs-side">
          <span className="cap num">
            {length.toLocaleString('ru-RU')} из {max.toLocaleString('ru-RU')}
          </span>
          {value ? (
            <button type="button" className="lcs-reset" aria-label={`Сбросить: ${label}`} onClick={reset}>
              Сбросить
            </button>
          ) : null}
        </span>
      </div>
      <div className={shown ? 'lcs-box bad' : 'lcs-box'}>
        <div className="lcs-nums" style={{ width: `calc(${String(lines).length}ch + 20px)` }} aria-hidden="true">
          <div ref={numbers}>
            {numbered.map((n) => (
              <span key={n} className={n === bad ? 'bad' : undefined}>
                {n}
              </span>
            ))}
          </div>
        </div>
        <textarea
          ref={area}
          id={id}
          className="lcs-area"
          rows={height}
          wrap="off"
          value={value}
          placeholder={placeholder}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          autoComplete="off"
          aria-invalid={shown ? true : undefined}
          aria-describedby={`${id}-n ${id}-k`}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          onScroll={syncScroll}
          onFocus={() => {
            entered.current = true;
          }}
          onBlur={() => {
            released.current = false;
          }}
        />
      </div>
      <span id={`${id}-k`} className="sr-only">
        Tab добавляет отступ. Чтобы выйти из поля, нажмите Escape, затем Tab.
      </span>
      <span id={`${id}-n`} role="status" className={shown ? 'err' : 'cap'}>
        {shown ? (
          shown.line ? (
            <button type="button" className="lcs-jump" onClick={jump}>
              {problemText(shown)}
            </button>
          ) : (
            problemText(shown)
          )
        ) : (
          hint
        )}
      </span>
      <div className="lcs-foot">
        <select
          className="lcs-pick"
          value=""
          aria-label="Вставить правило для части страницы"
          onChange={(e) => {
            const snippet = snippets[Number(e.target.value)];
            if (snippet) {
              run(insertRule(value, caret(), snippet.css));
            }
          }}
        >
          <option value="" disabled>
            Вставить правило для…
          </option>
          {snippets.map((snippet, i) => (
            <option key={snippet.name} value={i}>
              {snippet.name}
            </option>
          ))}
        </select>
        <button type="button" className="b36" onClick={() => file.current?.click()} disabled={busy}>
          {busy ? 'Загружаем…' : 'Загрузить картинку'}
        </button>
        <input ref={file} type="file" accept={rules.types.join(',')} hidden onChange={(e) => void send(e.target.files?.[0])} />
      </div>
      {imageError ? (
        <span className="err" role="alert">
          {imageError}
        </span>
      ) : null}
    </div>
  );
}
