import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { cleanHtml, safeHref } from '../../lib/cleanHtml';

export type Block = 'p' | 'h2' | 'h3' | 'blockquote';

export type Body = { html: string; raw: boolean };

const BLOCKS: Array<{ value: Block; label: string }> = [
  { value: 'p', label: 'Обычный текст' },
  { value: 'h2', label: 'Заголовок' },
  { value: 'h3', label: 'Подзаголовок' },
  { value: 'blockquote', label: 'Цитата' },
];

const EMPTY = '<p><br></p>';

type Props = {
  value: Body;
  onChange: (next: Body) => void;
  clean: (raw: string) => Promise<string>;
  placeholder: string;
  label: string;
  disabled?: boolean;
};

function pretty(html: string): string {
  return html
    .replace(/(<\/(?:p|h[1-6]|li|blockquote|tr|thead|tbody|table|ul|ol|pre|dl|dt|dd|div)>)/g, '$1\n')
    .replace(/(<(?:ul|ol|table|thead|tbody|tr|dl)(?:\s[^>]*)?>)/g, '$1\n')
    .replace(/(<(?:hr|br)\s*\/?>)(?!\n)/g, '$1\n')
    .replace(/\n{2,}/g, '\n')
    .trim();
}

function exec(command: string, value?: string): void {
  document.execCommand(command, false, value);
}

function blockOf(node: Node | null, root: HTMLElement): Block {
  let current: Node | null = node;
  while (current && current !== root) {
    if (current.nodeType === Node.ELEMENT_NODE) {
      const name = (current as Element).localName;
      if (name === 'h1' || name === 'h2') {
        return 'h2';
      }
      if (name === 'h3' || name === 'h4' || name === 'h5' || name === 'h6') {
        return 'h3';
      }
      if (name === 'blockquote') {
        return 'blockquote';
      }
    }
    current = current.parentNode;
  }
  return 'p';
}

function linkOf(node: Node | null, root: HTMLElement): HTMLAnchorElement | null {
  let current: Node | null = node;
  while (current && current !== root) {
    if (current.nodeType === Node.ELEMENT_NODE && (current as Element).localName === 'a') {
      return current as HTMLAnchorElement;
    }
    current = current.parentNode;
  }
  return null;
}

export function RichText({ value, onChange, clean, placeholder, label, disabled }: Props) {
  const box = useRef<HTMLDivElement | null>(null);
  const emitted = useRef<string | null>(null);
  const range = useRef<Range | null>(null);
  const [mode, setMode] = useState<'visual' | 'code'>(value.raw ? 'code' : 'visual');
  const [code, setCode] = useState(value.raw ? value.html : '');
  const [block, setBlock] = useState<Block>('p');
  const [marks, setMarks] = useState({ bold: false, italic: false, ul: false, ol: false });
  const [link, setLink] = useState<{ href: string; editing: boolean } | null>(null);
  const [linkError, setLinkError] = useState<string | null>(null);
  const [empty, setEmpty] = useState(true);
  const [working, setWorking] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el || mode !== 'visual' || value.raw || value.html === emitted.current) {
      return;
    }
    el.innerHTML = value.html.trim() ? value.html : EMPTY;
    emitted.current = value.html;
    setEmpty(!(el.textContent ?? '').trim());
  }, [value, mode]);

  const save = () => {
    const el = box.current;
    const selection = document.getSelection();
    if (!el || !selection || selection.rangeCount === 0) {
      return;
    }
    const current = selection.getRangeAt(0);
    if (!el.contains(current.commonAncestorContainer)) {
      return;
    }
    range.current = current.cloneRange();
    setBlock(blockOf(selection.anchorNode, el));
    setMarks({
      bold: document.queryCommandState('bold'),
      italic: document.queryCommandState('italic'),
      ul: document.queryCommandState('insertUnorderedList'),
      ol: document.queryCommandState('insertOrderedList'),
    });
  };

  useEffect(() => {
    document.addEventListener('selectionchange', save);
    return () => document.removeEventListener('selectionchange', save);
  }, []);

  const emit = () => {
    const el = box.current;
    if (!el) {
      return;
    }
    const next = el.innerHTML;
    emitted.current = next;
    setEmpty(!(el.textContent ?? '').trim() && !el.querySelector('hr, li, table'));
    onChange({ html: next, raw: false });
  };

  const restore = () => {
    const el = box.current;
    if (!el) {
      return;
    }
    const selection = document.getSelection();
    const inside = selection !== null && selection.rangeCount > 0 && el.contains(selection.getRangeAt(0).commonAncestorContainer);
    el.focus();
    exec('defaultParagraphSeparator', 'p');
    const saved = range.current;
    if (!inside && saved && selection && el.contains(saved.commonAncestorContainer)) {
      selection.removeAllRanges();
      selection.addRange(saved);
    }
  };

  const run = (command: string, arg?: string) => {
    restore();
    exec(command, arg);
    emit();
    save();
  };

  const setBlockKind = (next: Block) => {
    run('formatBlock', next === 'p' ? 'P' : next.toUpperCase());
    setBlock(next);
  };

  const openLink = () => {
    const el = box.current;
    const selection = document.getSelection();
    save();
    const anchor = el && selection ? linkOf(selection.anchorNode, el) : null;
    setLinkError(null);
    setLink({ href: anchor?.getAttribute('href') ?? 'https://', editing: anchor !== null });
  };

  const applyLink = () => {
    if (!link) {
      return;
    }
    const href = safeHref(link.href);
    if (!href || href === 'https://') {
      setLinkError('Нужна ссылка вида https://…, mailto: или адрес на этом сайте');
      return;
    }
    restore();
    const selection = document.getSelection();
    const el = box.current;
    const anchor = el && selection ? linkOf(selection.anchorNode, el) : null;
    if (anchor) {
      anchor.setAttribute('href', href);
    } else if (selection && selection.isCollapsed) {
      const text = href.replace(/^mailto:/, '');
      exec('insertHTML', `<a href="${href.replace(/&/g, '&amp;').replace(/"/g, '&quot;')}">${text.replace(/&/g, '&amp;').replace(/</g, '&lt;')}</a>`);
    } else {
      exec('createLink', href);
    }
    setLink(null);
    emit();
  };

  const removeLink = () => {
    restore();
    const selection = document.getSelection();
    const el = box.current;
    const anchor = el && selection ? linkOf(selection.anchorNode, el) : null;
    if (anchor && selection) {
      const whole = document.createRange();
      whole.selectNodeContents(anchor);
      selection.removeAllRanges();
      selection.addRange(whole);
    }
    exec('unlink');
    setLink(null);
    emit();
  };

  const toCode = () => {
    const el = box.current;
    setCode(pretty(cleanHtml(el ? el.innerHTML : value.html)));
    setLink(null);
    setFailed(null);
    setMode('code');
  };

  const toVisual = async () => {
    if (!value.raw) {
      emitted.current = null;
      setMode('visual');
      return;
    }
    setWorking(true);
    setFailed(null);
    try {
      const html = await clean(code);
      emitted.current = null;
      onChange({ html, raw: false });
      setMode('visual');
    } catch {
      setFailed('Не получилось очистить код, попробуйте ещё раз');
    } finally {
      setWorking(false);
    }
  };

  const paste = (rich: string, plain: string) => {
    const el = box.current;
    const selection = document.getSelection();
    const anchor = el && selection && selection.rangeCount > 0 && el.contains(selection.getRangeAt(0).commonAncestorContainer) ? selection.getRangeAt(0).cloneRange() : null;
    setWorking(true);
    clean(rich)
      .then((html) => html)
      .catch(() => null)
      .then((html) => {
        setWorking(false);
        if (anchor) {
          range.current = anchor;
        }
        const target = box.current;
        if (target) {
          target.focus();
          const current = document.getSelection();
          if (anchor && current) {
            current.removeAllRanges();
            current.addRange(anchor);
          }
        }
        if (html === null) {
          exec('insertText', plain);
        } else if (html) {
          exec('insertHTML', html);
        }
        emit();
      });
  };

  const onKey = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      openLink();
    }
  };

  const keep = (event: { preventDefault: () => void }) => event.preventDefault();
  const locked = disabled || working;

  return (
    <div className="dce-editor" aria-busy={working}>
      <div className="dce-tools" role="toolbar" aria-label="Оформление текста">
        {mode === 'visual' ? (
          <>
            <select className="dce-sel" aria-label="Вид абзаца" value={block} onChange={(e) => setBlockKind(e.target.value as Block)} disabled={locked}>
              {BLOCKS.map((b) => (
                <option key={b.value} value={b.value}>
                  {b.label}
                </option>
              ))}
            </select>
            <span className="dce-sep" aria-hidden="true" />
            <button type="button" className="dce-tool b" title="Жирный" aria-label="Жирный" aria-pressed={marks.bold} onMouseDown={keep} onClick={() => run('bold')} disabled={locked}>
              Ж
            </button>
            <button type="button" className="dce-tool i" title="Курсив" aria-label="Курсив" aria-pressed={marks.italic} onMouseDown={keep} onClick={() => run('italic')} disabled={locked}>
              К
            </button>
            <span className="dce-sep" aria-hidden="true" />
            <button type="button" className="dce-tool" aria-pressed={marks.ul} onMouseDown={keep} onClick={() => run('insertUnorderedList')} disabled={locked}>
              Список
            </button>
            <button type="button" className="dce-tool" aria-pressed={marks.ol} onMouseDown={keep} onClick={() => run('insertOrderedList')} disabled={locked}>
              Нумерация
            </button>
            <button type="button" className="dce-tool" title="Ссылка (Ctrl+K)" aria-expanded={link !== null} onMouseDown={keep} onClick={openLink} disabled={locked}>
              Ссылка
            </button>
            <button type="button" className="dce-tool" onMouseDown={keep} onClick={() => run('insertHorizontalRule')} disabled={locked}>
              Линия
            </button>
          </>
        ) : (
          <span className="dce-code-note">{failed ?? 'Код HTML. Скрипты, стили, фреймы и картинки уберутся при сохранении'}</span>
        )}
        <span className="grow" />
        <div role="group" aria-label="Режим редактора" className="segs dce-mode">
          <button type="button" aria-pressed={mode === 'visual'} onClick={() => (mode === 'code' ? void toVisual() : undefined)} disabled={locked}>
            Текст
          </button>
          <button type="button" aria-pressed={mode === 'code'} onClick={() => (mode === 'visual' ? toCode() : undefined)} disabled={locked}>
            HTML
          </button>
        </div>
      </div>
      {link ? (
        <div className="dce-link" role="group" aria-label="Ссылка">
          <input
            className="field mono"
            value={link.href}
            inputMode="url"
            aria-label="Адрес ссылки"
            aria-invalid={linkError !== null}
            autoFocus
            onChange={(e) => {
              setLink({ ...link, href: e.target.value });
              setLinkError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                applyLink();
              } else if (e.key === 'Escape') {
                setLink(null);
              }
            }}
          />
          <button type="button" className="b36 hov" onClick={applyLink}>
            {link.editing ? 'Изменить' : 'Добавить'}
          </button>
          {link.editing ? (
            <button type="button" className="b36 hov" onClick={removeLink}>
              Убрать ссылку
            </button>
          ) : null}
          <button type="button" className="dce-link-close hov" onClick={() => setLink(null)}>
            Отмена
          </button>
          {linkError ? <span className="err">{linkError}</span> : null}
        </div>
      ) : null}
      {mode === 'visual' ? (
        <div
          ref={box}
          className={empty ? 'dce-text is-empty' : 'dce-text'}
          contentEditable={!disabled}
          suppressContentEditableWarning
          role="textbox"
          aria-multiline="true"
          aria-label={label}
          data-placeholder={placeholder}
          spellCheck
          onInput={() => {
            emit();
            save();
          }}
          onKeyDown={onKey}
          onKeyUp={save}
          onMouseUp={save}
          onFocus={() => exec('defaultParagraphSeparator', 'p')}
          onPaste={(event) => {
            const data = event.clipboardData;
            const rich = data.getData('text/html');
            const plain = data.getData('text/plain');
            event.preventDefault();
            if (rich) {
              paste(rich, plain);
            } else {
              exec('insertText', plain);
              emit();
            }
          }}
          onDrop={(event) => event.preventDefault()}
        />
      ) : (
        <textarea
          className="dce-code"
          value={code}
          aria-label={`${label}, код HTML`}
          spellCheck={false}
          onChange={(e) => {
            setCode(e.target.value);
            onChange({ html: e.target.value, raw: true });
          }}
          disabled={locked}
        />
      )}
    </div>
  );
}
