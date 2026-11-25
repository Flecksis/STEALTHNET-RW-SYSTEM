export type Sections = { antibot: boolean; limits: boolean };

export function protectSections(modules: readonly string[] | undefined, canRead: boolean): Sections {
  const enabled = canRead ? (modules ?? []) : [];
  return { antibot: enabled.includes('antibot'), limits: enabled.includes('limits') };
}

export function clientFrom(raw: string | null): number | null {
  return raw && /^[1-9]\d{0,15}$/.test(raw) ? Number(raw) : null;
}
