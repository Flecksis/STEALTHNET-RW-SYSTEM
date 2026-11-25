const FORBIDDEN = new Set(['javascript', 'vbscript', 'data', 'file', 'about', 'blob', 'filesystem', 'http', 'ftp', 'ws', 'wss', 'chrome', 'chrome-extension', 'view-source', 'jar', 'mailto', 'tel', 'sms']);

export function forbiddenScheme(scheme: string): boolean {
  return FORBIDDEN.has(scheme.toLowerCase());
}

export function appScheme(link: string): string | null {
  const scheme = /^([a-z][a-z0-9+.-]{1,31}):./i.exec(link)?.[1]?.toLowerCase();
  return !scheme || scheme === 'https' || forbiddenScheme(scheme) ? null : scheme;
}
