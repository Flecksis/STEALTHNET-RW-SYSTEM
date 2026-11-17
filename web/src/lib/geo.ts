const REGIONS = (() => {
  try {
    return new Intl.DisplayNames(['ru'], { type: 'region' });
  } catch {
    return null;
  }
})();

export function countryName(code: string): string {
  if (!code || code.toUpperCase() === 'XX') {
    return '';
  }
  try {
    return REGIONS?.of(code.toUpperCase()) ?? code;
  } catch {
    return code;
  }
}

export function joinAnd(items: string[]): string {
  if (items.length < 2) {
    return items[0] ?? '';
  }
  return `${items.slice(0, -1).join(', ')} и ${items[items.length - 1]}`;
}
