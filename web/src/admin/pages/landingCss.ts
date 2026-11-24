import type { BlockKind } from '../../site/landingModel';

export type CssPlace = 'page' | BlockKind;
export type CssTexts = Partial<Record<CssPlace, string>>;
export type CssProblem = { line: number | null; text: string };
export type CssProblems = Partial<Record<CssPlace, CssProblem>>;
export type CssReport = Partial<Record<CssPlace, { style?: string; line?: number; problem?: string }>>;
export type Edit = { from: number; to: number; text: string; caret: number; anchor?: number };

export const CSS_PLACES: CssPlace[] = ['page', 'hero', 'features', 'steps', 'plans', 'servers', 'faq', 'cta', 'footer'];

const INDENT = '  ';

export function readTexts(raw: unknown): CssTexts {
  const out: CssTexts = {};
  if (raw && typeof raw === 'object') {
    for (const place of CSS_PLACES) {
      const text = (raw as Record<string, unknown>)[place];
      if (typeof text === 'string' && text) {
        out[place] = text;
      }
    }
  }
  return out;
}

export function cssOut(texts: CssTexts): CssTexts {
  const out: CssTexts = {};
  for (const place of CSS_PLACES) {
    const text = (texts[place] ?? '').trimEnd();
    if (text) {
      out[place] = text;
    }
  }
  return out;
}

export function sameCss(a: CssTexts, b: CssTexts): boolean {
  const [x, y] = [cssOut(a), cssOut(b)];
  return CSS_PLACES.every((place) => (x[place] ?? '') === (y[place] ?? ''));
}

export function joinStyles(styles: CssTexts): string {
  return CSS_PLACES.map((place) => styles[place] ?? '').join('');
}

type Spot = { depth: number; since: string };

function walk(text: string, visit: (piece: string, at: number, next: number) => boolean): void {
  let at = 0;
  while (at < text.length) {
    const c = text[at] ?? '';
    let next = at + 1;
    let piece = c;
    if (c === '/' && text[next] === '*') {
      const end = text.indexOf('*/', next + 1);
      next = end < 0 ? text.length : end + 2;
      piece = '';
    } else if (c === '"' || c === "'") {
      while (next < text.length && text[next] !== c && text[next] !== '\n') {
        next += text[next] === '\\' ? 2 : 1;
      }
      next = Math.min(next + 1, text.length);
      piece = text.slice(at, next);
    }
    if (!visit(piece, at, next)) {
      return;
    }
    at = next;
  }
}

function spotAt(text: string, caret: number): Spot {
  let depth = 0;
  let since = '';
  walk(text, (piece, at) => {
    if (at >= caret) {
      return false;
    }
    if (piece === '{' || piece === '}' || piece === ';') {
      depth = Math.max(0, depth + (piece === '{' ? 1 : piece === '}' ? -1 : 0));
      since = '';
    } else {
      since += piece;
    }
    return true;
  });
  return { depth, since };
}

function ruleEnd(text: string, caret: number): number {
  let depth = 0;
  let end = text.length;
  walk(text, (piece, at, next) => {
    if (piece === '{') {
      depth += 1;
    } else if (piece === '}') {
      depth = Math.max(0, depth - 1);
      if (depth === 0 && at >= caret) {
        end = next;
        return false;
      }
    }
    return true;
  });
  const line = text.indexOf('\n', end);
  return line < 0 ? text.length : line;
}

function lineStart(text: string, at: number): number {
  return text.lastIndexOf('\n', at - 1) + 1;
}

function indentOf(text: string, at: number): string {
  const line = text.slice(lineStart(text, at), at);
  return /^[ \t]*/.exec(line)?.[0] ?? '';
}

export function newLine(text: string, from: number, to: number): Edit {
  const indent = indentOf(text, from);
  const opened = /\{[ \t]*$/.test(text.slice(lineStart(text, from), from));
  const closes = opened && /^[ \t]*\}/.test(text.slice(to));
  const inner = `\n${indent}${opened ? INDENT : ''}`;
  return { from, to, text: closes ? `${inner}\n${indent}` : inner, caret: from + inner.length };
}

export function closeBrace(text: string, from: number, to: number): Edit | null {
  const start = lineStart(text, from);
  const line = text.slice(start, from);
  if (!line.endsWith(INDENT) || line.trim()) {
    return null;
  }
  const at = from - INDENT.length;
  return { from: at, to, text: '}', caret: at + 1 };
}

export function shiftLines(text: string, from: number, to: number, back: boolean): Edit | null {
  if (!back && !text.slice(from, to).includes('\n')) {
    return { from, to, text: INDENT, caret: from + INDENT.length };
  }
  const start = lineStart(text, from);
  const end = to > from && text[to - 1] === '\n' ? to - 1 : to;
  const lines = text.slice(start, end).split('\n');
  const moved = lines.map((line) => (back ? line.replace(/^( {1,2}|\t)/, '') : line ? INDENT + line : line)).join('\n');
  if (moved === lines.join('\n')) {
    return null;
  }
  const edit = { from: start, to: end, text: moved };
  return lines.length > 1
    ? { ...edit, anchor: start, caret: start + moved.length }
    : { ...edit, caret: Math.max(start, from + moved.length - (end - start)) };
}

function gap(text: string, at: number): string {
  const before = text.slice(0, at);
  if (!before.trim()) {
    return '';
  }
  return before.endsWith('\n\n') ? '' : before.endsWith('\n') ? '\n' : '\n\n';
}

export function insertRule(text: string, caret: number, rule: string): Edit {
  const { depth, since } = spotAt(text, caret);
  const at = depth > 0 || since.trim() ? ruleEnd(text, caret) : caret;
  const lead = gap(text, at);
  const tail = text.slice(at).startsWith('\n') || !text.slice(at).trim() ? '' : '\n\n';
  const inside = rule.indexOf(`${INDENT}\n`);
  const caretIn = inside >= 0 ? inside + INDENT.length : rule.length;
  return { from: at, to: at, text: `${lead}${rule}${tail}`, caret: at + lead.length + caretIn };
}

export function insertImage(text: string, caret: number, url: string, page: boolean): Edit {
  const value = `url("${url}")`;
  const { depth, since } = spotAt(text, caret);
  if (since.includes(':') && (depth > 0 || !page)) {
    return { from: caret, to: caret, text: value, caret: caret + value.length };
  }
  const declaration = `background: ${value} center / cover no-repeat;`;
  if (depth > 0 || !page) {
    const atLineStart = !text.slice(lineStart(text, caret), caret).trim();
    const lead = atLineStart ? '' : `\n${indentOf(text, caret)}`;
    return { from: caret, to: caret, text: `${lead}${declaration}`, caret: caret + lead.length + declaration.length };
  }
  const rule = `.ld-hero {\n${INDENT}${declaration}\n}`;
  const lead = gap(text, caret);
  return { from: caret, to: caret, text: `${lead}${rule}`, caret: caret + lead.length + rule.length };
}

export function lineRange(text: string, line: number): { from: number; to: number } {
  let from = 0;
  for (let n = 1; n < line; n += 1) {
    const next = text.indexOf('\n', from);
    if (next < 0) {
      break;
    }
    from = next + 1;
  }
  const end = text.indexOf('\n', from);
  return { from, to: end < 0 ? text.length : end };
}

export function lengthProblem(text: string, max: number): CssProblem | null {
  return [...text].length > max ? { line: null, text: `не длиннее ${max.toLocaleString('ru-RU')} символов` } : null;
}

export function problemText(problem: CssProblem): string {
  return problem.line ? `Строка ${problem.line}: ${problem.text}` : `${problem.text.charAt(0).toUpperCase()}${problem.text.slice(1)}`;
}
