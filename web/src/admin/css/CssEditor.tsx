import { useId, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { ApiError } from '../../lib/api';
import { closeBrace, insertImage, insertRule, lengthProblem, lineRange, newLine, problemText, shiftLines, type CssProblem, type Edit, type Snippet } from './cssText';
import './css-editor.css';

type Props = {
  label: string;
  value: string;
  max: number;
  problem: CssProblem | null;
  imageRule: string | null;
  rows: [number, number];
  hint?: ReactNode;
  placeholder: string;
  snippets: Snippet[];
  types: string[];
  sendImage: (file: File) => Promise<{ url: string }>;
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

export function CssEditor({ label, value, max, problem, imageRule, rows, hint, placeholder, snippets, types, sendImage, onChange }: Props) {
  const id = useId();
  const area = useRef<HTMLTextAreaElement>(null);
  const numbers = useRef<HTMLDivElement>(null);
  const file = useRef<HTMLInputElement>(null);
  const released = useRef(false);
  const entered = useRef(false);
  const [busy, setBusy] = useState(false);
  const [imageError, setImageError] = useState<string | null>(null);

  const length = [...value].length;
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
      const saved = await sendImage(picked);
      if (area.current) {
        run(insertImage(area.current.value, entered.current ? area.current.selectionStart : area.current.value.length, saved.url, imageRule));
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
        <label htmlFor={id} className="lcs-label">
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
          aria-label="Вставить правило для части"
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
        <input ref={file} type="file" accept={types.join(',')} hidden onChange={(e) => void send(e.target.files?.[0])} />
      </div>
      {imageError ? (
        <span className="err" role="alert">
          {imageError}
        </span>
      ) : null}
    </div>
  );
}
