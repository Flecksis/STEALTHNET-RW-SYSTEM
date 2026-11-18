export type MethodRef = { provider: string; method?: string | null };

export function methodKey(m: MethodRef): string {
  return m.method ? `${m.provider}:${m.method}` : m.provider;
}
