export function backTarget(pathname: string): string | null {
  const path = pathname.replace(/\/+$/, '');
  if (path === '/app' || !path.startsWith('/app/')) {
    return null;
  }
  const parent = path.slice(0, path.lastIndexOf('/'));
  return parent.length > '/app'.length ? parent : '/app';
}
