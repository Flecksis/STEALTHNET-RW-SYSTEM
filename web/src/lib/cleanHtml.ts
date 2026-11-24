const HTML_NS = 'http://www.w3.org/1999/xhtml';

const TAGS = new Set([
  'a',
  'b',
  'blockquote',
  'br',
  'caption',
  'code',
  'dd',
  'del',
  'div',
  'dl',
  'dt',
  'em',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'hr',
  'i',
  'ins',
  'li',
  'ol',
  'p',
  'pre',
  's',
  'small',
  'span',
  'strong',
  'sub',
  'sup',
  'table',
  'tbody',
  'td',
  'tfoot',
  'th',
  'thead',
  'tr',
  'u',
  'ul',
]);

const DROPPED = new Set([
  'applet',
  'audio',
  'button',
  'canvas',
  'embed',
  'frame',
  'frameset',
  'head',
  'iframe',
  'math',
  'noembed',
  'noframes',
  'noscript',
  'object',
  'script',
  'select',
  'style',
  'svg',
  'template',
  'textarea',
  'title',
  'video',
  'xmp',
]);

const ATTRIBUTES: Record<string, string[]> = {
  a: ['href', 'title'],
  ol: ['start', 'reversed'],
  td: ['colspan', 'rowspan'],
  th: ['colspan', 'rowspan', 'scope'],
};

const SCHEMES = new Set(['http', 'https', 'mailto', 'tel']);

export function safeHref(raw: string): string | null {
  const value = raw.trim();
  if (!value) {
    return null;
  }
  const compact = value.replace(/[\u0000- \u007f]/g, '');
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(compact);
  if (!scheme) {
    return value;
  }
  return SCHEMES.has((scheme[1] ?? '').toLowerCase()) ? value : null;
}

function copy(from: Node, to: Element, doc: Document): void {
  for (const node of Array.from(from.childNodes)) {
    if (node.nodeType === Node.TEXT_NODE) {
      to.appendChild(doc.createTextNode(node.textContent ?? ''));
      continue;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) {
      continue;
    }
    const element = node as Element;
    const name = element.localName.toLowerCase();
    if (DROPPED.has(name) || element.namespaceURI !== HTML_NS) {
      continue;
    }
    if (!TAGS.has(name) || (name === 'a' && safeHref(element.getAttribute('href') ?? '') === null)) {
      copy(element, to, doc);
      continue;
    }
    const clean = doc.createElement(name);
    for (const attribute of ATTRIBUTES[name] ?? []) {
      const value = element.getAttribute(attribute);
      if (value === null) {
        continue;
      }
      if (attribute === 'href') {
        const href = safeHref(value);
        if (href !== null) {
          clean.setAttribute('href', href);
        }
        continue;
      }
      clean.setAttribute(attribute, value);
    }
    if (name === 'a' && clean.hasAttribute('href')) {
      clean.setAttribute('rel', 'noopener noreferrer');
    }
    copy(element, clean, doc);
    to.appendChild(clean);
  }
}

export function cleanHtml(raw: string): string {
  const doc = new DOMParser().parseFromString(`<!doctype html><html><body>${raw}</body></html>`, 'text/html');
  const out = doc.createElement('div');
  copy(doc.body, out, doc);
  return out.innerHTML.trim();
}

export function textLength(html: string): number {
  const doc = new DOMParser().parseFromString(`<!doctype html><html><body>${html}</body></html>`, 'text/html');
  return (doc.body.textContent ?? '').replace(/\s+/g, ' ').trim().length;
}
