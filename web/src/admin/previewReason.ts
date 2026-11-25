export type PreviewAnswer = { status: number; redirected: boolean; frame: string | null; policy: string | null };

export const FINE = 'Браузер не открыл превью, обновите страницу.';

const KNOWN = ['deny', 'allowall', 'sameorigin'];

function tokens(raw: string | null): string[] {
  return (raw ?? '')
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean);
}

function frameBan(raw: string | null): string | null {
  const values = [...new Set(tokens(raw).map((value) => value.toLowerCase()))];
  if (values.includes('deny')) {
    return 'DENY';
  }
  if (values.length > 1 && values.some((value) => KNOWN.includes(value))) {
    return tokens(raw).join(', ');
  }
  return null;
}

function allowsSelf(sources: string[], origin: string): boolean {
  const { protocol, host, hostname } = new URL(origin);
  return sources.some((source) => {
    const value = source.toLowerCase();
    if (value === "'self'" || value === '*' || value === protocol || value === origin || value === host || value === hostname) {
      return true;
    }
    const wildcard = /^(?:https?:\/\/)?\*(\.[^:/]+)(?::(?:\d+|\*))?$/.exec(value);
    return !!wildcard?.[1] && hostname.endsWith(wildcard[1]);
  });
}

function ancestorsBan(raw: string | null, origin: string): string | null {
  for (const policy of tokens(raw)) {
    const directive = policy
      .split(';')
      .map((part) => part.trim())
      .find((part) => /^frame-ancestors(\s|$)/i.test(part));
    if (!directive) {
      continue;
    }
    const sources = directive.split(/\s+/).slice(1);
    if (!allowsSelf(sources, origin)) {
      return `frame-ancestors ${sources.join(' ') || "'none'"}`;
    }
  }
  return null;
}

export function previewReason(answer: PreviewAnswer, path: string, origin: string): string {
  if (answer.redirected) {
    return `Перед панелью стоит прокси, который перенаправляет ${path}, уберите перенаправление для этого адреса.`;
  }
  const refused = answer.status < 200 || answer.status > 299;
  if (refused && answer.status >= 500) {
    return `Страница превью ответила ошибкой ${answer.status}. Проверьте, что панель запущена, и повторите.`;
  }
  if (refused) {
    return `Страница превью ответила ошибкой ${answer.status}. Проверьте в прокси перед панелью правила для ${path}.`;
  }
  const frame = frameBan(answer.frame);
  if (frame) {
    return `Перед панелью стоит прокси, который добавляет X-Frame-Options: ${frame}, уберите его для ${path}.`;
  }
  const ancestors = ancestorsBan(answer.policy, origin);
  if (ancestors) {
    return `Перед панелью стоит прокси, который добавляет Content-Security-Policy: ${ancestors}, уберите его для ${path}.`;
  }
  return FINE;
}
