import type { PayoutMethod, WithdrawalStatus } from './model';

export const PAYOUT_METHODS: Array<{ key: PayoutMethod; label: string; hint: string }> = [
  { key: 'sbp', label: 'СБП', hint: 'Номер телефона и банк, например: +7 900 123-45-67, Т-Банк' },
  { key: 'card', label: 'Карта', hint: 'Номер карты, 16 цифр' },
  { key: 'usdt_trc20', label: 'USDT (TRC20)', hint: 'Адрес кошелька TRC20, начинается с T' },
];

export const WITHDRAWAL_STATUS: Record<WithdrawalStatus, { text: string; tone: string }> = {
  holding: { text: 'На рассмотрении', tone: 'muted' },
  pending: { text: 'На рассмотрении', tone: 'muted' },
  paid: { text: 'Выплачено', tone: 'ok' },
  rejected: { text: 'Отклонено', tone: 'bad' },
  canceled: { text: 'Отменено', tone: 'muted' },
};

export function payoutDetails(details: string): string {
  const text = details.trim();
  return text.length > 24 && !/\s/.test(text) ? `${text.slice(0, 6)}…${text.slice(-4)}` : text;
}

export function payoutLabel(method: PayoutMethod): string {
  return PAYOUT_METHODS.find((m) => m.key === method)?.label ?? method;
}
