import { msg } from '../lib/i18n';
import type { Currency } from '../lib/types';
import type { PayoutOption, Withdrawal, WithdrawalStatus } from './model';

export const WITHDRAWAL_STATUS: Record<WithdrawalStatus, { text: string; tone: string }> = {
  holding: { text: msg('На рассмотрении'), tone: 'muted' },
  pending: { text: msg('На рассмотрении'), tone: 'muted' },
  paid: { text: msg('Выплачено'), tone: 'ok' },
  rejected: { text: msg('Отклонено'), tone: 'bad' },
  canceled: { text: msg('Отменено'), tone: 'muted' },
};

export function payoutDetails(details: string): string {
  const text = details.trim();
  return text.length > 24 && !/\s/.test(text) ? `${text.slice(0, 6)}…${text.slice(-4)}` : text;
}

export function payoutLabel(w: Withdrawal): string {
  return w.method_title?.trim() || w.method;
}

export function firstPayout(methods: PayoutOption[], currency: Currency): string {
  const usual = currency === 'RUB' ? 'sbp' : 'usdt_trc20';
  return (methods.find((m) => m.id === usual) ?? methods[0])?.id ?? '';
}
