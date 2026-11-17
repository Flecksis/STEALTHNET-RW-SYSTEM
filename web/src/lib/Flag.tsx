const FILES = import.meta.glob<string>('/node_modules/country-flag-icons/3x2/*.svg', {
  base: '/node_modules/country-flag-icons/3x2/',
  eager: true,
  import: 'default',
  query: '?no-inline',
});

const BY_CODE = new Map<string, string>();
for (const [path, url] of Object.entries(FILES)) {
  BY_CODE.set(path.slice(path.lastIndexOf('/') + 1, -'.svg'.length).toUpperCase(), url);
}

export function flagUrl(code: string | null | undefined): string | null {
  return code ? (BY_CODE.get(code.trim().toUpperCase()) ?? null) : null;
}

type Props = { code: string | null | undefined; label?: string };

export function Flag({ code, label }: Props) {
  const src = flagUrl(code);
  if (!src) {
    return null;
  }
  return <img className="country-flag" src={src} alt={label ?? ''} aria-hidden={label ? undefined : true} width={18} height={12} decoding="async" />;
}
