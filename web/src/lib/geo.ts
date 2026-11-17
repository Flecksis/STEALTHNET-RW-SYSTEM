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

const IN: Record<string, string> = {
  NL: 'Нидерландах', DE: 'Германии', FI: 'Финляндии', RU: 'России', KZ: 'Казахстане', US: 'США', GB: 'Великобритании',
  FR: 'Франции', PL: 'Польше', TR: 'Турции', SE: 'Швеции', LV: 'Латвии', EE: 'Эстонии', LT: 'Литве', AE: 'ОАЭ',
  JP: 'Японии', SG: 'Сингапуре', HK: 'Гонконге', CH: 'Швейцарии', AT: 'Австрии', ES: 'Испании', IT: 'Италии',
  CA: 'Канаде', UA: 'Украине', AM: 'Армении', GE: 'Грузии', RS: 'Сербии', MD: 'Молдове', BY: 'Беларуси',
  CZ: 'Чехии', RO: 'Румынии', BG: 'Болгарии', HU: 'Венгрии', NO: 'Норвегии', DK: 'Дании', IE: 'Ирландии',
  PT: 'Португалии', IL: 'Израиле', IN: 'Индии', KR: 'Южной Корее', AU: 'Австралии', BR: 'Бразилии',
};

export function countriesIn(codes: string[]): string | null {
  const names = codes.map((c) => IN[c.toUpperCase()]);
  if (!names.length || names.some((n) => !n)) {
    return null;
  }
  return joinAnd(names as string[]);
}
