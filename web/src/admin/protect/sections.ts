export type Sections = { antibot: boolean; limits: boolean };

export function protectSections(modules: readonly string[] | undefined, canRead: boolean): Sections {
  const enabled = canRead ? (modules ?? []) : [];
  return { antibot: enabled.includes('antibot'), limits: enabled.includes('limits') };
}
