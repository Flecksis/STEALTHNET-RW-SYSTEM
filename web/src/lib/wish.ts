export type Lang = 'ru' | 'en';

export type Wish = {
  query?: string | null;
  stored?: string | null;
  hint?: string | null;
  browser?: readonly (string | undefined)[];
};

const RUSSIAN_LIKE = ['ru', 'uk', 'be', 'kk'];

export function langOfTag(tag: string | undefined | null): Lang | null {
  const base = (tag ?? '').trim().split(/[-_]/)[0]?.toLowerCase() ?? '';
  if (!/^[a-z]{2,3}$/.test(base)) {
    return null;
  }
  return RUSSIAN_LIKE.includes(base) ? 'ru' : 'en';
}

export function wishedLang(wish: Wish): Lang | null {
  for (const chosen of [wish.query, wish.stored]) {
    if (chosen === 'ru' || chosen === 'en') {
      return chosen;
    }
  }
  if (wish.hint) {
    return langOfTag(wish.hint);
  }
  for (const tag of wish.browser ?? []) {
    const found = langOfTag(tag);
    if (found) {
      return found;
    }
  }
  return null;
}
