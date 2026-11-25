import type { SubscriptionView } from './model';

type Held = Pick<SubscriptionView, 'status'> & { subscription: { id: number; mirror: { expire_at: string | null } } };

const RANK: Partial<Record<SubscriptionView['status'], number>> = { limited: 1, blocked: 2, expired: 3 };

function rank(view: Held): number {
  return RANK[view.status] ?? 0;
}

function endsAt(view: Held): number {
  const at = view.subscription.mirror.expire_at;
  return at ? Date.parse(at) : Number.POSITIVE_INFINITY;
}

export function byTimeLeft<T extends Held>(list: T[]): T[] {
  return [...list].sort((a, b) => {
    if (rank(a) !== rank(b)) {
      return rank(a) - rank(b);
    }
    const [left, right] = [endsAt(a), endsAt(b)];
    return left === right ? a.subscription.id - b.subscription.id : left > right ? -1 : 1;
  });
}

export function numberOf(id: number, list: Held[]): number {
  return list.filter((v) => v.subscription.id <= id).length;
}

export function betterStart(list: Held[], kept: number | string | null): number | null {
  const own = list.find((v) => v.subscription.id === kept);
  const best = list[0];
  return own && best && own.status === 'expired' && best.status !== 'expired' ? best.subscription.id : null;
}
