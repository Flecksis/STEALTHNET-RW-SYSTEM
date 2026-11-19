const DOT_SEGMENT = /^(?:\.|%2e){1,2}$/i;

export function leavesPath(path: string): boolean {
  const [route = ''] = path.split(/[?#]/, 1);
  return route.split(/[\\/]/).some((part) => DOT_SEGMENT.test(part));
}

export function pathPart(value: string | number): string {
  return encodeURIComponent(String(value));
}
