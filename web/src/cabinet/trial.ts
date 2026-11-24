import { api } from '../lib/api';
import type { CheckAnswer, CheckPrompt } from '../lib/humanCheck';
import { count, days } from '../lib/format';
import { countryName } from '../lib/geo';
import { t } from '../lib/i18n';
import { gbText } from './model';

export type TrialChoice = {
  id: number;
  name: string;
  days: number;
  devices: number;
  traffic: { limit_bytes: number | null; reset: string };
  countries: string[];
};

export type TrialOffer = { available: boolean; variants: TrialChoice[] };

export type TrialCheck = CheckPrompt & { again?: string };

export const NO_TRIAL: TrialOffer = { available: false, variants: [] };

export function loadTrial(): Promise<TrialOffer> {
  return api<TrialOffer>('/api/client/trial');
}

export async function requestTrial(variant: number | null, answer?: CheckAnswer): Promise<TrialCheck | null> {
  const body: Partial<CheckAnswer> & { variant?: number } = { ...(answer ?? {}) };
  if (variant !== null) {
    body.variant = variant;
  }
  const started = await api<{ confirm?: CheckPrompt; again?: string }>('/api/client/trial', { method: 'POST', body });
  return started.confirm ? { ...started.confirm, ...(started.again ? { again: started.again } : {}) } : null;
}

export function trialFacts(choice: TrialChoice, withDays = true): string {
  const traffic = choice.traffic.limit_bytes ? t('трафик {amount}', { amount: gbText(choice.traffic.limit_bytes) }) : t('трафик без лимита');
  const facts = [count(choice.devices, 'устройство', 'устройства', 'устройств'), traffic];
  return (withDays ? [days(choice.days), ...facts] : facts).join(' · ');
}

export function trialPlaces(choice: TrialChoice): string | null {
  const names = choice.countries.map(countryName).filter(Boolean);
  return names.length ? t('Серверы: {list}', { list: names.join(', ') }) : null;
}

export function trialTitle(choice: TrialChoice, index: number): string {
  return choice.name.trim() || t('Вариант {n}', { n: index + 1 });
}
