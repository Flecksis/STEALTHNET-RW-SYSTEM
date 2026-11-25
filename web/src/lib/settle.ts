export const SETTLE_MS = 2100;

export function settleLeft(shownAt: number | undefined, now: number): number {
  if (shownAt === undefined) {
    return 0;
  }
  return Math.min(SETTLE_MS, Math.max(0, SETTLE_MS - (now - shownAt)));
}
