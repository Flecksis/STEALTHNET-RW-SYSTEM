import type { Permission } from '../lib/types';

const MODULE_NEED: Record<string, Permission> = {
  analytics: 'money',
  referral: 'money',
  broadcast: 'marketing',
  promo: 'marketing',
  promolinks: 'marketing',
  gifts: 'marketing',
  contests: 'marketing',
  roulette: 'marketing',
};

export function menuAllowed(module: string, can: (permission: Permission) => boolean): boolean {
  const need = MODULE_NEED[module];
  return !need || can(need);
}
