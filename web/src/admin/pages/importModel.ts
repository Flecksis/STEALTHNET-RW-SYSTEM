import type { Currency } from '../../lib/types';

export type Source = 'stealthnet' | 'bedolaga' | 'telegram_shop' | 'chill' | 'remnashop' | 'minishop';

export type RunState =
  | 'uploaded'
  | 'checking'
  | 'needs_password'
  | 'ready'
  | 'failed'
  | 'running'
  | 'done'
  | 'interrupted'
  | 'discarded';

export type Counts = {
  clients: number;
  subscriptions: number;
  tariffs: number;
  promo_codes: number;
  referrals: number;
  payments: number;
  tickets: number;
  admins?: number;
  gifts?: number;
  auto_rules?: number;
  withdrawals?: number;
  settings?: number;
};

export type Skip = { code: string; count: number; text: string };

export type MailPlan =
  | { action: 'transfer'; incoming: string }
  | { action: 'partial'; incoming: string; reason: string }
  | { action: 'same'; incoming: string }
  | { action: 'differs'; current: string; incoming: string }
  | { action: 'skipped'; reason: string };

export type MultiPlan =
  | { action: 'enable'; setting: boolean; clients: number }
  | { action: 'same' }
  | { action: 'differs'; setting: boolean; clients: number; recommended: boolean };

export type Preview = {
  detected: Source;
  dumped_at: string | null;
  counts: Counts;
  balances: {
    clients: number;
    total_minor: number;
    currency: Currency;
    large_clients?: number;
    large_total_minor?: number;
    large_from_minor?: number | null;
    large_by_tariff?: boolean;
  };
  currency: { source: Currency; install: Currency; switch: boolean; blocked: string | null };
  remna: { connected: boolean; error: string | null; users: number; found: number; missing: number; ambiguous: number; shared?: number };
  existing: { clients: number; matched: number; again: number };
  tariff_mode: 'tariffs' | 'classic';
  skipped: Skip[];
  mail?: MailPlan | null;
  multi?: MultiPlan | null;
};

export type Stage = 'read' | 'tariffs' | 'clients' | 'subscriptions' | 'payments' | 'promo' | 'referrals' | 'tickets' | 'extras' | 'finish';

export type Progress = { stage: Stage; done: number; total: number };

export type Outcome = {
  counts: Counts;
  created: number;
  merged: number;
  updated: number;
  issues: number;
  currency_switched: boolean;
  balance_total_minor: number;
  providers?: string[];
  mail?: string | null;
  multi?: boolean;
};

export type Run = {
  id: string;
  source: Source;
  state: RunState;
  file: { name: string; size: number };
  preview: Preview | null;
  progress: Progress | null;
  outcome: Outcome | null;
  error: string | null;
  issues: number;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  by: string | null;
};

export type BotLink = 'ok' | 'conflict' | 'webhook' | 'no_token' | 'unauthorized' | 'offline';

export type ImportView = {
  remna: boolean;
  currency: Currency;
  clients: number;
  bot: BotLink;
  run: Run | null;
  history: Run[];
};

export const SOURCES: Array<{ id: Source; name: string; what: string; of: string }> = [
  { id: 'stealthnet', name: 'STEALTHNET', what: 'Панель и бот STEALTHNET версии 7', of: 'STEALTHNET' },
  { id: 'bedolaga', name: 'Бот Бедолаги', what: 'remnawave-bedolaga-telegram-bot', of: 'бота Бедолаги' },
  { id: 'telegram_shop', name: 'Remnawave Telegram Shop', what: 'remnawave-telegram-shop', of: 'Remnawave Telegram Shop' },
  { id: 'chill', name: 'Chill Remna bot', what: 'chill-remna-bot', of: 'Chill Remna bot' },
  { id: 'remnashop', name: 'Remnashop', what: 'Бот remnashop от snoups', of: 'Remnashop' },
  { id: 'minishop', name: 'Minishop', what: 'remnawave-minishop', of: 'Minishop' },
];

export const STAGES: Array<{ key: Stage; label: string }> = [
  { key: 'read', label: 'Чтение файла' },
  { key: 'tariffs', label: 'Тарифы' },
  { key: 'clients', label: 'Клиенты и балансы' },
  { key: 'subscriptions', label: 'Подписки' },
  { key: 'payments', label: 'Платежи' },
  { key: 'promo', label: 'Промокоды' },
  { key: 'referrals', label: 'Рефералы' },
  { key: 'tickets', label: 'Обращения' },
  { key: 'extras', label: 'Настройки, админы, подарки' },
  { key: 'finish', label: 'Сверка' },
];

export const COUNT_LABELS: Array<{ key: keyof Counts; label: string; optional?: boolean }> = [
  { key: 'clients', label: 'Клиенты' },
  { key: 'subscriptions', label: 'Подписки' },
  { key: 'tariffs', label: 'Тарифы' },
  { key: 'promo_codes', label: 'Промокоды' },
  { key: 'referrals', label: 'Рефералы' },
  { key: 'payments', label: 'Платежи' },
  { key: 'tickets', label: 'Обращения' },
  { key: 'admins', label: 'Админы', optional: true },
  { key: 'gifts', label: 'Подарочные коды', optional: true },
  { key: 'withdrawals', label: 'Заявки на вывод', optional: true },
  { key: 'auto_rules', label: 'Авторассылки', optional: true },
];

export function countLabels(counts: Counts) {
  return COUNT_LABELS.filter((c) => !c.optional || (counts[c.key] ?? 0) > 0);
}

export const MAX_UPLOAD = 20 * 1024 * 1024 * 1024;

export const ACCEPT: Record<Source, string> = {
  stealthnet: '.sql,.gz',
  bedolaga: '.gz,.tgz,.tar,.zip,.json',
  telegram_shop: '.sql,.gz,.tgz,.tar,.zip',
  chill: '.bak,.db,.sqlite,.sql,.gz,.tgz,.tar,.zip',
  remnashop: '.sql,.gz,.dump',
  minishop: '.zip,.dump,.sql,.gz',
};

export const FILES: Record<Source, string> = {
  stealthnet: 'Файл .sql или .sql.gz',
  bedolaga: 'Файл .tar.gz, .tar или .zip',
  telegram_shop: 'Файл .sql или .sql.gz',
  chill: 'Файл .bak, файл базы .db или .sql',
  remnashop: 'Файл .sql',
  minishop: 'Файл .zip',
};

export const ACTIVE: RunState[] = ['checking', 'running'];
