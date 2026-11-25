export type Part = { text: string; long: boolean };

const LONG = /\S{24,}/g;

export function longParts(text: string): Part[] {
  const out: Part[] = [];
  let from = 0;
  for (const match of text.matchAll(LONG)) {
    if (match.index > from) {
      out.push({ text: text.slice(from, match.index), long: false });
    }
    out.push({ text: match[0], long: true });
    from = match.index + match[0].length;
  }
  if (from < text.length) {
    out.push({ text: text.slice(from), long: false });
  }
  return out;
}

export function wrapLongWords(html: string): string {
  if (!html.includes('<table')) {
    return html;
  }
  const doc = new DOMParser().parseFromString(`<!doctype html><html><body>${html}</body></html>`, 'text/html');
  const texts = Array.from(doc.querySelectorAll('td, th')).flatMap((cell) => {
    const found: Text[] = [];
    const walker = doc.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      if (node.parentElement?.closest('td, th') === cell) {
        found.push(node);
      }
    }
    return found;
  });
  for (const node of texts) {
    const parts = longParts(node.data);
    if (!parts.some((part) => part.long)) {
      continue;
    }
    node.replaceWith(
      ...parts.map((part) => {
        if (!part.long) {
          return part.text;
        }
        const span = doc.createElement('span');
        span.className = 'dp-long';
        span.textContent = part.text;
        return span;
      }),
    );
  }
  return doc.body.innerHTML;
}
