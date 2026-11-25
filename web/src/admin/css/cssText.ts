export type CssTexts<P extends string> = Partial<Record<P, string>>;
export type CssProblem = { line: number | null; text: string };
export type CssProblems<P extends string> = Partial<Record<P, CssProblem>>;
export type CssReport<P extends string> = Partial<Record<P, { style?: string; line?: number; problem?: string }>>;
export type Edit = { from: number; to: number; text: string; caret: number; anchor?: number };
export type Snippet = { name: string; css: string };

const INDENT = '  ';

export function snippet(name: string, selector: string): Snippet {
  return { name, css: `${selector} {\n  \n}` };
}

export function without<P extends string, T>(current: Partial<Record<P, T>>, places: P[]): Partial<Record<P, T>> {
  const next = { ...current };
  for (const place of places) {
    delete next[place];
  }
  return next;
}

export function readTexts<P extends string>(places: P[], raw: unknown): CssTexts<P> {
  const out: CssTexts<P> = {};
  if (raw && typeof raw === 'object') {
    for (const place of places) {
      const text = (raw as Record<string, unknown>)[place];
      if (typeof text === 'string' && text) {
        out[place] = text;
      }
    }
  }
  return out;
}

export function cssOut<P extends string>(places: P[], texts: CssTexts<P>): CssTexts<P> {
  const out: CssTexts<P> = {};
  for (const place of places) {
    const text = (texts[place] ?? '').trimEnd();
    if (text) {
      out[place] = text;
    }
  }
  return out;
}

export function sameCss<P extends string>(places: P[], a: CssTexts<P>, b: CssTexts<P>): boolean {
  const [x, y] = [cssOut(places, a), cssOut(places, b)];
  return places.every((place) => (x[place] ?? '') === (y[place] ?? ''));
}

export function joinStyles<P extends string>(places: P[], styles: CssTexts<P>): string {
  return places.map((place) => styles[place] ?? '').join('');
}

export function linesOf(text: string): number {
  return text.trim().split('\n').length;
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

export function insertImage(text: string, caret: number, url: string, selector: string | null): Edit {
  const value = `url("${url}")`;
  const { depth, since } = spotAt(text, caret);
  if (since.includes(':') && (depth > 0 || selector === null)) {
    return { from: caret, to: caret, text: value, caret: caret + value.length };
  }
  const declaration = `background: ${value} center / cover no-repeat;`;
  if (depth > 0 || selector === null) {
    const atLineStart = !text.slice(lineStart(text, caret), caret).trim();
    const lead = atLineStart ? '' : `\n${indentOf(text, caret)}`;
    return { from: caret, to: caret, text: `${lead}${declaration}`, caret: caret + lead.length + declaration.length };
  }
  const rule = `${selector} {\n${INDENT}${declaration}\n}`;
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

export function placeProblems<P extends string>(places: P[], texts: CssTexts<P>, problems: CssProblems<P>, max: (place: P) => number, name: (place: P) => string): Array<{ place: P; text: string }> {
  const out: Array<{ place: P; text: string }> = [];
  for (const place of places) {
    const problem = lengthProblem(texts[place] ?? '', max(place)) ?? problems[place];
    if (problem) {
      out.push({ place, text: problem.line ? `${name(place)}, строка ${problem.line}: ${problem.text}` : `${name(place)}: ${problem.text}` });
    }
  }
  return out;
}
