export const REPORT_PATH = '/admin/bug-report';
export const ISSUES_REPO = 'STEALTHNET-APP/stealthnet-issues';
export const ISSUE_FORM: string | null = 'bug.yml';
export const MAX_URL = 8000;
export const HIDDEN = '[скрыто]';
export const CUT_NOTE = '> Текст обрезан: в ссылку на GitHub больше не поместилось.';
export const NO_TITLE = 'Ошибка в админке';
export const ERRORS_TITLE = 'Ошибки интерфейса';

export const SECTIONS = {
  what: 'Что случилось',
  steps: 'Как повторить',
  expected: 'Что ожидали',
  version: 'Версия панели',
  remnawave: 'Версия Remnawave',
  page: 'Страница',
  browser: 'Браузер и система',
} as const;

export type SectionId = keyof typeof SECTIONS;
export type Target = { repo: string; form: string | null };
export type Report = Record<SectionId, string> & { errors: string[] | null };

const IDS = Object.keys(SECTIONS) as SectionId[];
const SINGLE_LINE: SectionId[] = ['version', 'remnawave', 'page', 'browser'];

const URL_RE = /\b[a-z][a-z0-9+.-]{1,20}:\/\/[^\s<>"'`]+/gi;
const EMAIL_RE = /(?<![\p{L}\p{N}._%+-])[\p{L}\p{N}._%+-]+@[\p{L}\p{N}-]+(?:\.[\p{L}\p{N}-]+)*\.\p{L}{2,}/gu;
const JWT_RE = /\beyJ[\w-]{5,}\.[\w-]{5,}(?:\.[\w-]+)?/g;
const BOT_TOKEN_RE = /\b\d{6,12}:[\w-]{30,}/g;
const UUID_RE = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi;
const BEARER_RE = /\b(Bearer|Basic)\s+[\w.~+/=-]{8,}/gi;
const SECRET_PAIR_RE =
  /((?<![\p{L}\p{N}_-])[\p{L}\p{N}_-]*(?:token|secret|password|passwd|pwd|pass|key|cookie|auth|authorization|sid|signature|sign|пароль|ключ|токен|секрет)["']?\s*[:=]\s*["']?)([^\s"'<>,;&}]+)/giu;
const QUERY_RE = /\?[^\s?#=]*=[^\s#]*/g;
const FRAGMENT_RE = /#[\w-]+=[^\s]*/g;
const HOST_RE = /(?<![\p{L}\p{N}@.-])(?:[\p{L}\p{N}-]+\.)+\p{L}{2,}(?::\d{2,5})?(?=\/)/gu;
const BARE_HOST_RE = /(?<![\p{L}\p{N}@._/-])(?:[\p{L}\p{N}-]+\.)+(\p{L}{2,})(?::\d{2,5})?(?![\p{L}\p{N}_/-]|\.[\p{L}\p{N}])/gu;
const HOST_ZONES = new Set([
  'com', 'net', 'org', 'ru', 'su', 'рф', 'io', 'app', 'dev', 'xyz', 'pro', 'online', 'site', 'me', 'tech', 'cc', 'top', 'info',
  'store', 'shop', 'link', 'live', 'space', 'website', 'host', 'cloud', 'vip', 'club', 'biz', 'us', 'uk', 'de', 'nl', 'fi', 'kz',
  'by', 'ua', 'eu', 'world', 'network', 'one', 'tv', 'to', 'ws',
]);

function bareHost(found: string, zone: string): string {
  return found.split('.').length >= 3 || HOST_ZONES.has(zone.toLowerCase()) ? HIDDEN : found;
}
const IPV6_FULL_RE = /(?<![\w:])(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}(?![\w:])/gi;
const IPV6_SHORT_RE = /(?<![\w:])(?:[0-9a-f]{1,4}:){1,7}:(?:[0-9a-f]{1,4}(?::[0-9a-f]{1,4}){0,6})?(?![\w:])/gi;
const IPV4_RE = /(?<![\w./])(?:\d{1,3}\.){3}\d{1,3}(?::\d{2,5})?(?![\w]|\.\d)/g;
const PHONE_RE = /(?<![\w+])(?:\+\d{1,3}|8)[\s(-]*\d{3}[\s)-]*\d{3}[\s-]*\d{2}[\s-]*\d{2}(?!\d)/g;
const DIGITS_RE = /\d{9,}/g;
const USERNAME_RE = /(?<![\w@.])@[A-Za-z][\w]{3,31}\b/g;
const RUN_RE = /[\w-]{16,}/g;

function hasDigitAndLetter(text: string): boolean {
  return /\d/.test(text) && /[A-Za-z]/.test(text);
}

function secretRun(run: string): boolean {
  if (run.split(/[-_]/).some((part) => part.length >= 16 && hasDigitAndLetter(part))) {
    return true;
  }
  return run.length >= 20 && /\d/.test(run) && /[a-z]/.test(run) && /[A-Z]/.test(run);
}

function cleanSegment(segment: string): string {
  if (!segment) {
    return segment;
  }
  if (/^[A-Za-z0-9]{8,}$/.test(segment) && hasDigitAndLetter(segment)) {
    return HIDDEN;
  }
  return cleanPlain(segment);
}

function cleanUrl(url: string): string {
  const parts = /^([a-z][a-z0-9+.-]*):\/\/([^/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/i.exec(url);
  if (!parts) {
    return HIDDEN;
  }
  const scheme = parts[1] ?? '';
  if (!/^https?$/i.test(scheme)) {
    return `${scheme}://${HIDDEN}`;
  }
  const path = (parts[3] ?? '').split('/').map(cleanSegment).join('/');
  return `${scheme}://${HIDDEN}${path}${parts[4] ? `?${HIDDEN}` : ''}${parts[5] ? `#${HIDDEN}` : ''}`;
}

function cleanPlain(text: string): string {
  return text
    .replace(EMAIL_RE, HIDDEN)
    .replace(JWT_RE, HIDDEN)
    .replace(BOT_TOKEN_RE, HIDDEN)
    .replace(UUID_RE, HIDDEN)
    .replace(BEARER_RE, (_, kind: string) => `${kind} ${HIDDEN}`)
    .replace(SECRET_PAIR_RE, (_, head: string) => `${head}${HIDDEN}`)
    .replace(QUERY_RE, `?${HIDDEN}`)
    .replace(FRAGMENT_RE, `#${HIDDEN}`)
    .replace(HOST_RE, HIDDEN)
    .replace(BARE_HOST_RE, (found: string, zone: string) => bareHost(found, zone))
    .replace(IPV6_FULL_RE, HIDDEN)
    .replace(IPV6_SHORT_RE, HIDDEN)
    .replace(IPV4_RE, HIDDEN)
    .replace(PHONE_RE, HIDDEN)
    .replace(DIGITS_RE, HIDDEN)
    .replace(USERNAME_RE, `@${HIDDEN}`)
    .replace(RUN_RE, (run) => (secretRun(run) ? HIDDEN : run));
}

export function clean(text: string): string {
  let out = '';
  let last = 0;
  for (const match of text.matchAll(URL_RE)) {
    const start = match.index ?? 0;
    out += cleanPlain(text.slice(last, start)) + cleanUrl(match[0]);
    last = start + match[0].length;
  }
  return out + cleanPlain(text.slice(last));
}

export function pagePath(raw: unknown): string | null {
  if (typeof raw !== 'string' || !raw.startsWith('/')) {
    return null;
  }
  const path = raw.split(/[?#]/, 1)[0] ?? '';
  return path
    .slice(0, 200)
    .split('/')
    .map((part) => {
      if (!part) {
        return part;
      }
      const sensitive = /^\d+$/.test(part) || (/\d/.test(part) && part.length >= 8) || clean(part) !== part;
      return sensitive ? ':id' : part;
    })
    .join('/');
}

const BROWSERS: Array<[RegExp, string]> = [
  [/YaBrowser\/(\d+)/, 'Яндекс Браузер'],
  [/Edg(?:e|A|iOS)?\/(\d+)/, 'Edge'],
  [/(?:OPR|Opera)\/(\d+)/, 'Opera'],
  [/SamsungBrowser\/(\d+)/, 'Samsung Internet'],
  [/(?:Firefox|FxiOS)\/(\d+)/, 'Firefox'],
  [/(?:Chrome|CriOS)\/(\d+)/, 'Chrome'],
  [/Version\/(\d+(?:\.\d+)?).*Safari\//, 'Safari'],
];

export function browserName(ua: string): string {
  for (const [pattern, name] of BROWSERS) {
    const found = pattern.exec(ua);
    if (found) {
      return `${name} ${found[1] ?? ''}`.trim();
    }
  }
  return 'неизвестный браузер';
}

export function systemName(ua: string): string {
  const ios = /(?:iPhone|iPad|iPod).*? OS (\d+)[_.](\d+)/.exec(ua);
  if (ios) {
    return `iOS ${ios[1]}.${ios[2]}`;
  }
  const android = /Android (\d+(?:\.\d+)?)/.exec(ua);
  if (android) {
    return `Android ${android[1]}`;
  }
  if (/Windows/.test(ua)) {
    return 'Windows';
  }
  if (/CrOS/.test(ua)) {
    return 'ChromeOS';
  }
  if (/Mac OS X|Macintosh/.test(ua)) {
    return 'macOS';
  }
  if (/Linux/.test(ua)) {
    return 'Linux';
  }
  return 'неизвестная система';
}

function two(n: number): string {
  return String(n).padStart(2, '0');
}

export function clock(date: Date): string {
  return `${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}`;
}

export function stamp(date: Date): string {
  const offset = -date.getTimezoneOffset();
  const hours = Math.floor(Math.abs(offset) / 60);
  const minutes = Math.abs(offset) % 60;
  const zone = `UTC${offset < 0 ? '-' : '+'}${hours}${minutes ? `:${two(minutes)}` : ''}`;
  const day = `${two(date.getDate())}.${two(date.getMonth() + 1)}.${date.getFullYear()}`;
  return `${day} ${two(date.getHours())}:${two(date.getMinutes())} (${zone})`;
}

export function titleFrom(what: string, max = 80): string {
  const line = clean(what)
    .split('\n')
    .map((part) => part.trim())
    .find(Boolean);
  if (!line) {
    return NO_TITLE;
  }
  const chars = Array.from(line);
  if (chars.length <= max) {
    return line;
  }
  const cut = chars.slice(0, max).join('');
  const space = cut.lastIndexOf(' ');
  return `${(space > max * 0.6 ? cut.slice(0, space) : cut).trimEnd()}…`;
}

function section(id: SectionId, text: string): string {
  return `### ${SECTIONS[id]}\n\n${text}`;
}

function oneLine(text: string): string {
  return text
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .join('; ');
}

function errorsBlock(errors: string[]): string {
  const log = errors.map((entry) => clean(entry).replace(/`{3,}/g, "'''")).join('\n\n');
  return `${ERRORS_TITLE}:\n\n\`\`\`text\n${log}\n\`\`\``;
}

export function composeBody(report: Report): string {
  const what = clean(report.what.trim());
  const errors = report.errors && report.errors.length > 0 ? errorsBlock(report.errors) : '';
  const parts = [section('what', [what, errors].filter(Boolean).join('\n\n'))];
  for (const id of IDS.slice(1)) {
    const text = SINGLE_LINE.includes(id) ? oneLine(clean(report[id])) : clean(report[id].trim());
    if (text) {
      parts.push(section(id, text));
    }
  }
  return parts.join('\n\n');
}

export function formFields(body: string): Array<[SectionId, string]> {
  const found = new Map<SectionId, string[]>();
  let current: SectionId = 'what';
  for (const line of body.split('\n')) {
    const heading = /^#{1,6}\s+(.+?)\s*$/.exec(line);
    const id = heading ? IDS.find((key) => SECTIONS[key] === heading[1]) : undefined;
    if (id) {
      current = id;
      continue;
    }
    const lines = found.get(current) ?? [];
    lines.push(line);
    found.set(current, lines);
  }
  return IDS.map((id): [SectionId, string] => [id, (found.get(id) ?? []).join('\n').trim()]).filter(([, value]) => value !== '');
}

function bodyOf(parts: Map<SectionId, string>): string {
  return IDS.filter((id) => parts.get(id))
    .map((id) => section(id, parts.get(id) ?? ''))
    .join('\n\n');
}

export function issueUrl(target: Target, title: string, body: string): string {
  const params = new URLSearchParams();
  if (target.form) {
    params.set('template', target.form);
    params.set('title', title);
    for (const [id, value] of formFields(body)) {
      params.set(id, SINGLE_LINE.includes(id) ? oneLine(value) : value);
    }
  } else {
    params.set('title', title);
    params.set('body', body);
  }
  return `https://github.com/${target.repo}/issues/new?${params.toString()}`;
}

function boundary(head: string): string {
  const lineEnd = head.lastIndexOf('\n');
  if (lineEnd > 0 && head.length - lineEnd < 400) {
    return head.slice(0, lineEnd);
  }
  const wordEnd = head.lastIndexOf(' ');
  if (wordEnd > 0 && head.length - wordEnd < 100) {
    return head.slice(0, wordEnd);
  }
  return head;
}

function shortened(head: string, single: boolean): string {
  const text = head.trimEnd();
  if (single) {
    return `${text}…`;
  }
  const open = (text.match(/^```/gm) ?? []).length % 2 === 1;
  return `${text}${open ? '\n```' : ''}\n\n${CUT_NOTE}`.trimStart();
}

export function fitIssueUrl(target: Target, title: string, body: string, max = MAX_URL): { url: string; cut: boolean } {
  const full = issueUrl(target, title, body);
  if (full.length <= max) {
    return { url: full, cut: false };
  }
  const parts = new Map(formFields(body));
  const build = () => issueUrl(target, title, bodyOf(parts));
  const cost = (id: SectionId) => encodeURIComponent(parts.get(id) ?? '').length;
  const order = [...parts.keys()].sort((a, b) => cost(b) - cost(a));
  for (const id of order) {
    const chars = Array.from(parts.get(id) ?? '');
    const single = SINGLE_LINE.includes(id);
    const fits = (count: number) => {
      parts.set(id, shortened(chars.slice(0, count).join(''), single));
      return build().length <= max;
    };
    if (!fits(0)) {
      continue;
    }
    let low = 0;
    let high = chars.length;
    while (low < high) {
      const middle = Math.ceil((low + high) / 2);
      if (fits(middle)) {
        low = middle;
      } else {
        high = middle - 1;
      }
    }
    parts.set(id, shortened(boundary(chars.slice(0, low).join('')), single));
    break;
  }
  return { url: build(), cut: true };
}
