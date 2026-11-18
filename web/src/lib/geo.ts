import { lang, t, type Lang } from './i18n';

const REGIONS = new Map<Lang, Intl.DisplayNames | null>();

function regions(): Intl.DisplayNames | null {
  const current = lang();
  if (!REGIONS.has(current)) {
    try {
      REGIONS.set(current, new Intl.DisplayNames([current], { type: 'region' }));
    } catch {
      REGIONS.set(current, null);
    }
  }
  return REGIONS.get(current) ?? null;
}

export function countryName(code: string): string {
  if (!code || code.toUpperCase() === 'XX') {
    return '';
  }
  try {
    return regions()?.of(code.toUpperCase()) ?? code;
  } catch {
    return code;
  }
}

export function joinAnd(items: string[]): string {
  if (items.length < 2) {
    return items[0] ?? '';
  }
  return `${items.slice(0, -1).join(', ')} ${t('и')} ${items[items.length - 1]}`;
}

const IN: Record<string, string> = {
  NL: 'Нидерландах', DE: 'Германии', FI: 'Финляндии', RU: 'России', KZ: 'Казахстане', US: 'США', GB: 'Великобритании',
  FR: 'Франции', PL: 'Польше', TR: 'Турции', SE: 'Швеции', LV: 'Латвии', EE: 'Эстонии', LT: 'Литве', AE: 'ОАЭ',
  JP: 'Японии', SG: 'Сингапуре', HK: 'Гонконге', CH: 'Швейцарии', AT: 'Австрии', ES: 'Испании', IT: 'Италии',
  CA: 'Канаде', UA: 'Украине', AM: 'Армении', GE: 'Грузии', RS: 'Сербии', MD: 'Молдове', BY: 'Беларуси',
  CZ: 'Чехии', RO: 'Румынии', BG: 'Болгарии', HU: 'Венгрии', NO: 'Норвегии', DK: 'Дании', IE: 'Ирландии',
  PT: 'Португалии', IL: 'Израиле', IN: 'Индии', KR: 'Южной Корее', AU: 'Австралии', BR: 'Бразилии',
};

const WITH_THE = new Set(['NL', 'US', 'GB', 'AE', 'PH', 'CZ']);

export function countriesIn(codes: string[]): string | null {
  if (lang() === 'en') {
    const names = codes.map((code) => (WITH_THE.has(code.toUpperCase()) ? `the ${countryName(code)}` : countryName(code))).filter((name) => name && name !== 'the ');
    return names.length === codes.length && names.length ? joinAnd(names) : null;
  }
  const names = codes.map((c) => IN[c.toUpperCase()]);
  if (!names.length || names.some((n) => !n)) {
    return null;
  }
  return joinAnd(names as string[]);
}
