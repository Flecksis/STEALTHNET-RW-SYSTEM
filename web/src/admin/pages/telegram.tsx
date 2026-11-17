import { Fragment, type ReactNode } from 'react';

type Node = { tag: string; attrs: Record<string, string>; children: Array<Node | string> };

const KNOWN = new Set(['b', 'strong', 'i', 'em', 'u', 'ins', 's', 'strike', 'del', 'span', 'tg-spoiler', 'a', 'code', 'pre', 'blockquote', 'tg-emoji']);

function decode(text: string): string {
  return text
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#(\d+);/g, (_, code: string) => String.fromCodePoint(Number(code)))
    .replace(/&#x([0-9a-f]+);/gi, (_, code: string) => String.fromCodePoint(parseInt(code, 16)))
    .replace(/&amp;/g, '&');
}

function readAttrs(raw: string): Record<string, string> {
  const found: Record<string, string> = {};
  const pattern = /([a-zA-Z-]+)\s*=\s*("([^"]*)"|'([^']*)')/g;
  let match = pattern.exec(raw);
  while (match) {
    found[(match[1] ?? '').toLowerCase()] = decode(match[3] ?? match[4] ?? '');
    match = pattern.exec(raw);
  }
  return found;
}

export function parseTelegram(html: string): Array<Node | string> {
  const root: Node = { tag: 'root', attrs: {}, children: [] };
  const stack: Node[] = [root];
  const pattern = /<(\/?)([a-zA-Z][a-zA-Z0-9-]*)([^<>]*)>/g;
  let last = 0;
  let match = pattern.exec(html);
  while (match) {
    const top = stack[stack.length - 1] ?? root;
    if (match.index > last) {
      top.children.push(decode(html.slice(last, match.index)));
    }
    const tag = (match[2] ?? '').toLowerCase();
    if (!KNOWN.has(tag)) {
      top.children.push(match[0]);
    } else if (match[1]) {
      let at = -1;
      for (let i = stack.length - 1; i > 0; i -= 1) {
        if (stack[i]?.tag === tag) {
          at = i;
          break;
        }
      }
      if (at > 0) {
        stack.length = at;
      } else {
        top.children.push(match[0]);
      }
    } else {
      const node: Node = { tag, attrs: readAttrs(match[3] ?? ''), children: [] };
      top.children.push(node);
      stack.push(node);
    }
    last = pattern.lastIndex;
    match = pattern.exec(html);
  }
  if (last < html.length) {
    (stack[stack.length - 1] ?? root).children.push(decode(html.slice(last)));
  }
  return root.children;
}

function safeHref(href: string | undefined): string | undefined {
  return href && /^(https?:|tg:)/i.test(href) ? href : undefined;
}

function renderNodes(nodes: Array<Node | string>, prefix: string): ReactNode[] {
  return nodes.map((node, index) => {
    const key = `${prefix}.${index}`;
    if (typeof node === 'string') {
      return <Fragment key={key}>{node}</Fragment>;
    }
    const inner = renderNodes(node.children, key);
    switch (node.tag) {
      case 'b':
      case 'strong':
        return <strong key={key}>{inner}</strong>;
      case 'i':
      case 'em':
        return <em key={key}>{inner}</em>;
      case 'u':
      case 'ins':
        return <u key={key}>{inner}</u>;
      case 's':
      case 'strike':
      case 'del':
        return <s key={key}>{inner}</s>;
      case 'code':
        return <code key={key}>{inner}</code>;
      case 'pre':
        return <pre key={key}>{inner}</pre>;
      case 'blockquote':
        return <blockquote key={key}>{inner}</blockquote>;
      case 'a':
        return (
          <a key={key} href={safeHref(node.attrs.href)} target="_blank" rel="noreferrer noopener">
            {inner}
          </a>
        );
      case 'tg-emoji':
        return (
          <span key={key} className="bt-emoji" title={`Премиум-эмодзи ${node.attrs['emoji-id'] ?? ''}`}>
            {inner}
          </span>
        );
      case 'tg-spoiler':
      case 'span':
        return (
          <span key={key} className="bt-spoiler">
            {inner}
          </span>
        );
      default:
        return <Fragment key={key}>{inner}</Fragment>;
    }
  });
}

export function TelegramText({ html }: { html: string }) {
  return <>{renderNodes(parseTelegram(html), 'n')}</>;
}

export function unclosedTag(html: string): string | null {
  const stack: string[] = [];
  const pattern = /<(\/?)([a-zA-Z][a-zA-Z0-9-]*)[^<>]*>/g;
  let match = pattern.exec(html);
  while (match) {
    const tag = (match[2] ?? '').toLowerCase();
    if (KNOWN.has(tag)) {
      if (match[1]) {
        if (stack[stack.length - 1] !== tag) {
          return `лишний закрывающий тег </${tag}>`;
        }
        stack.pop();
      } else {
        stack.push(tag);
      }
    }
    match = pattern.exec(html);
  }
  return stack.length ? `тег <${stack[stack.length - 1]}> не закрыт` : null;
}

export function visibleLength(html: string): number {
  const text = html.replace(/<[^>]*>/g, '');
  return decode(text).replace(/[\uD800-\uDBFF][\uDC00-\uDFFF]/g, '__').length;
}
