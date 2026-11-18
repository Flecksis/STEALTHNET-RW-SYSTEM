import type { Stat } from './model';

export const EARLY_DAYS = 30;

export type StepKey = 'arrived' | 'trials' | 'connected' | 'paid';

export type Step = { key: StepKey; title: string; value: number; share: number | null };

type Counts = Pick<Stat, 'registrations' | 'trials' | 'connected' | 'paid'>;

export function funnel(s: Counts, trials: boolean): Step[] {
  const base = s.registrations;
  const share = (v: number) => (base > 0 ? (v / base) * 100 : null);
  const steps: Step[] = [{ key: 'arrived', title: 'Пришли', value: base, share: null }];
  if (trials) {
    steps.push({ key: 'trials', title: 'Попробовали', value: s.trials, share: share(s.trials) });
  }
  steps.push({ key: 'connected', title: 'Подключились', value: s.connected, share: share(s.connected) });
  steps.push({ key: 'paid', title: 'Оплатили', value: s.paid, share: share(s.paid) });
  return steps;
}

export function ageDays(since: string | null | undefined, now: number): number | null {
  if (!since) {
    return null;
  }
  const at = Date.parse(since);
  if (Number.isNaN(at)) {
    return null;
  }
  return Math.max(0, Math.floor((now - at) / 86_400_000));
}

export function early(days: number | null): boolean {
  return days !== null && days < EARLY_DAYS;
}

function plural(n: number, one: string, few: string, many: string): string {
  const n10 = n % 10;
  const n100 = n % 100;
  if (n10 === 1 && n100 !== 11) {
    return one;
  }
  if (n10 >= 2 && n10 <= 4 && (n100 < 12 || n100 > 14)) {
    return few;
  }
  return many;
}

export function ageText(days: number): string {
  return `${days} ${plural(days, 'день', 'дня', 'дней')}`;
}
