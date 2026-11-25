import { ApiError } from '../../lib/api';
import { ago, count, dateTime, plural, traffic } from '../../lib/format';
import type { Person } from '../home/model';
import { captchaTrouble, type CaptchaTrouble } from './captchaText';
import type { Paces, SiteLimits, SiteState } from './siteLimits';

export const ANTIBOT = '/api/admin/m/antibot';
export const LIMITS = '/api/admin/m/limits';

export type Page<T> = { items: T[]; next: number | null };

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function isOff(err: unknown): boolean {
  return err instanceof ApiError && err.code === 'module.disabled';
}

export type Mode = 'mark' | 'confirm' | 'block';

export type Reason = { code: string; title: string };

export type JournalItem = {
  id: number;
  at: string;
  client_id: number;
  client: Person | null;
  action:
    | 'marked'
    | 'asked'
    | 'confirmed'
    | 'trial_denied'
    | 'promo_denied'
    | 'no_bonus'
    | 'cleared'
    | 'roulette_asked'
    | 'roulette_denied'
    | 'roulette_noted'
    | 'contest_asked'
    | 'contest_denied'
    | 'contest_noted'
    | 'contest_excluded'
    | 'promo_link_asked'
    | 'promo_link_denied';
  title: string;
  reasons: Reason[];
  details: Record<string, unknown>;
  admin: string | null;
};

export type MarkItem = {
  client_id: number;
  client: Person | null;
  reasons: Reason[];
  marked_at: string;
  confirmed_at: string | null;
  trial_denied: boolean;
};

export type AntibotSettings = Partial<SiteLimits> & Partial<Paces> & {
  mode: Mode;
  young_days: number;
  check_username: boolean;
  burst_count: number;
  ip_limit: number;
  extra_domains: string[];
  referral_guard: boolean;
  mass_invites: number;
  roulette_guard: boolean;
  contest_guard: boolean;
  captcha: CaptchaProvider;
  captcha_site_key: string;
  captcha_trial: 'all' | 'suspicious';
  captcha_signup: boolean;
  captcha_domains?: string[];
};

export type CaptchaProvider = 'none' | 'turnstile' | 'yandex';

export type CaptchaProblem = CaptchaTrouble & { at: string; provider: CaptchaProvider };

export type CaptchaState = {
  secret_set: boolean;
  ready: boolean;
  problem: CaptchaProblem | null;
};

export type AntibotSummary = {
  settings: AntibotSettings;
  captcha?: CaptchaState;
  site?: SiteState;
  marked_open: number;
  marked_week: number;
  asked_week: number;
  confirmed_week: number;
  denied_week: number;
  no_bonus_week: number;
};

export type ClientMark = {
  marked: boolean;
  reasons: Reason[];
  marked_at: string | null;
  confirmed_at: string | null;
  cleared_at: string | null;
};

export const MODES: Array<{ id: Mode; title: string; text: string }> = [
  {
    id: 'mark',
    title: 'Только отмечать',
    text: 'Подозрительные аккаунты попадают в журнал, пробный период, промо-ссылки, бонусы, рулетка и конкурсы работают как обычно.',
  },
  {
    id: 'confirm',
    title: 'Просить подтвердить',
    text: 'Перед пробным периодом, подпиской по промо-ссылке, бесплатным вращением рулетки и участием в конкурсе подозрительный клиент нажимает кнопку «Я не робот».',
  },
  {
    id: 'block',
    title: 'Не давать пробник и бонусы',
    text: 'Подозрительным не выдаются пробный период, дни по промокодам, подписки по промо-ссылкам и бесплатные вращения рулетки, в конкурсы их не принимают, пригласившим не начисляется процент с их оплат.',
  },
];

export const CAPTCHA_PROVIDERS: Array<{ id: CaptchaProvider; title: string; text: string }> = [
  {
    id: 'none',
    title: 'Без внешней капчи',
    text: 'Кнопка «Я не робот» с проверкой в браузере. Простой скрипт её не пройдёт, а написанный под вашу панель — пройдёт.',
  },
  {
    id: 'turnstile',
    title: 'Cloudflare Turnstile',
    text: 'Бесплатно. Ключи: панель Cloudflare → Turnstile → Add widget, в списке доменов — домен панели.',
  },
  {
    id: 'yandex',
    title: 'Яндекс SmartCaptcha',
    text: 'Ключи: Яндекс Облако → SmartCaptcha → Создать капчу, в списке доменов — домен панели. Подходит, если у клиентов плохо открывается Cloudflare.',
  },
];

export function captchaTitle(provider: CaptchaProvider): string {
  return CAPTCHA_PROVIDERS.find((p) => p.id === provider)?.title ?? provider;
}

export function captchaLine(settings: AntibotSettings): string | null {
  if (settings.captcha === 'none') {
    return null;
  }
  const where = [settings.captcha_trial === 'all' ? 'всем перед пробным периодом' : 'подозрительным перед пробным периодом'];
  if (settings.captcha_signup) {
    where.push('при регистрации по почте');
  }
  return `Капча ${captchaTitle(settings.captcha)}: ${where.join(' и ')}.`;
}

export function captchaAlert(summary: AntibotSummary): string | null {
  const state = summary.captcha;
  if (summary.settings.captcha === 'none' || !state) {
    return null;
  }
  if (!state.ready) {
    return 'Капча выбрана, но секретный ключ не читается: пробный период и регистрация идут без неё. Откройте «Настроить» и вставьте ключ заново.';
  }
  if (state.problem) {
    return captchaProblem(state.problem);
  }
  return null;
}

export function captchaProblem(problem: CaptchaProblem): string {
  return captchaTrouble(problem, dateTime(problem.at).toLowerCase());
}

export const JOURNAL_FILTERS: Array<[string, string]> = [
  ['all', 'Все'],
  ['signup', 'Отметки'],
  ['trial', 'Пробный период'],
  ['promo', 'Промокоды и ссылки'],
  ['referral', 'Рефералы'],
  ['roulette', 'Рулетка'],
  ['contest', 'Конкурсы'],
  ['cleared', 'Снятые'],
];

export function modeTitle(mode: Mode): string {
  return MODES.find((m) => m.id === mode)?.title ?? mode;
}

export function antibotLead(summary: AntibotSummary): string {
  const parts: string[] = [];
  parts.push(`Режим: ${modeTitle(summary.settings.mode).toLowerCase()}.`);
  const captcha = captchaLine(summary.settings);
  if (captcha) {
    parts.push(captcha);
  }
  if (summary.marked_open > 0) {
    parts.push(`Сейчас отмечено ${count(summary.marked_open, 'клиент', 'клиента', 'клиентов')}.`);
  } else {
    parts.push('Отмеченных клиентов нет.');
  }
  const week: string[] = [];
  if (summary.marked_week > 0) {
    week.push(`${summary.marked_week} ${plural(summary.marked_week, 'новая отметка', 'новые отметки', 'новых отметок')}`);
  }
  if (summary.confirmed_week > 0) {
    week.push(`${summary.confirmed_week} ${plural(summary.confirmed_week, 'прошёл', 'прошли', 'прошли')} проверку «Я не робот»`);
  }
  if (summary.denied_week > 0) {
    week.push(`${summary.denied_week} ${plural(summary.denied_week, 'отказ', 'отказа', 'отказов')} в пробнике, промокодах, промо-ссылках, рулетке или конкурсах`);
  }
  if (summary.no_bonus_week > 0) {
    week.push(`${summary.no_bonus_week} без бонуса пригласившему`);
  }
  if (week.length) {
    parts.push(`За неделю: ${week.join(', ')}.`);
  }
  return parts.join(' ');
}

export function reasonsText(reasons: Reason[]): string {
  return reasons.map((r) => r.title).join(', ');
}

export function journalNote(item: JournalItem): string | null {
  const d = item.details;
  if (item.action === 'no_bonus') {
    const levels = Array.isArray(d.levels) ? (d.levels as number[]) : [];
    const payment = typeof d.payment_id === 'number' ? `оплата №${d.payment_id}` : 'оплата';
    return levels.length > 1 ? `${payment}, уровни ${levels.join(', ')}` : payment;
  }
  if (item.action === 'cleared' && item.admin) {
    return item.admin;
  }
  if (item.action === 'confirmed' && d.via === 'captcha') {
    return d.target === 'trial' ? 'капчей перед пробным периодом' : 'капчей';
  }
  if (d.target === 'promo_link') {
    return typeof d.name === 'string' ? `промо-ссылка «${d.name}»` : 'промо-ссылка';
  }
  if (d.target === 'promo') {
    return 'промокод на дни';
  }
  if (d.target === 'contest') {
    return typeof d.title === 'string' ? `конкурс «${d.title}»` : 'конкурс';
  }
  if (d.target === 'roulette') {
    return typeof d.prize === 'string' ? `рулетка, приз: ${d.prize}` : 'рулетка';
  }
  if (typeof d.network === 'string') {
    return `адрес ${d.network}`;
  }
  if (typeof d.domain === 'string') {
    return `домен ${d.domain}`;
  }
  return null;
}

export function markState(m: MarkItem): { text: string; tone: string } {
  if (m.trial_denied) {
    return { text: 'Пробный период не выдан', tone: 'warn' };
  }
  if (m.confirmed_at) {
    return { text: `Прошёл проверку ${dateTime(m.confirmed_at).toLowerCase()}`, tone: 'ink' };
  }
  return { text: 'Не подтверждал', tone: 'muted' };
}

export type Audience = 'all' | 'trial' | 'paid';
export type TrafficScope = 'any' | 'unlimited' | 'limited';
export type Period = 'day' | 'week';
export type Restriction = 'none' | 'disable' | 'squad';

export type Rule = {
  id: number;
  enabled: boolean;
  audience: Audience;
  traffic: TrafficScope;
  devices_min: number | null;
  period: Period;
  limit_bytes: number;
  per_device: boolean;
  warn: boolean;
  alert: boolean;
  restriction: Restriction;
  squad: string | null;
  note: string | null;
  group: string;
  description: string;
};

export type RuleInput = {
  enabled: boolean;
  audience: Audience;
  traffic: TrafficScope;
  devices_min: number | null;
  period: Period;
  limit_gb: number;
  per_device: boolean;
  warn: boolean;
  alert: boolean;
  restriction: Restriction;
  squad: string | null;
  note: string | null;
};

export type Exceed = {
  id: number;
  rule_id: number;
  subscription_id: number;
  client_id: number;
  period: Period;
  window_start: string;
  window_end: string;
  limit_bytes: number;
  used_bytes: number;
  warn: boolean;
  alert: boolean;
  restriction: Restriction;
  squad: string | null;
  restrict_tries: number;
  exceeded_at: string;
  warned_at: string | null;
  restricted_at: string | null;
  restrict_note: string | null;
  lifted_at: string | null;
  lift_reason: string | null;
  lift_due_at: string | null;
  client: Person | null;
  tariff: string | null;
  rule: string | null;
};

export type LimitsSummary = {
  rules: number;
  current: number;
  restricted: number;
  week: number;
  offset_minutes: number;
  last_scan: { at: string; subscriptions: number; exceeded: number } | null;
};

export type Squad = { uuid: string; name: string };

export const GB = 1024 ** 3;

export const AUDIENCES: Array<[Audience, string]> = [
  ['all', 'Все подписки'],
  ['paid', 'Кроме пробных'],
  ['trial', 'Только пробные'],
];

export const SCOPES: Array<[TrafficScope, string]> = [
  ['any', 'Любой трафик'],
  ['unlimited', 'Без лимита трафика'],
  ['limited', 'С лимитом трафика'],
];

export const PERIODS: Array<[Period, string]> = [
  ['day', 'За сутки'],
  ['week', 'За неделю'],
];

export const RESTRICTIONS: Array<[Restriction, string, string]> = [
  ['none', 'Не ограничивать', 'Только предупреждение и уведомление админам'],
  ['disable', 'Отключить до конца окна', 'Подписка выключается в Remnawave и включается сама в начале следующих суток или недели'],
  ['squad', 'Перевести в другой сквад', 'Например, в сквад с медленными нодами; прежние сквады вернутся сами'],
];

export function emptyRule(): RuleInput {
  return {
    enabled: true,
    audience: 'all',
    traffic: 'unlimited',
    devices_min: null,
    period: 'day',
    limit_gb: 100,
    per_device: false,
    warn: true,
    alert: true,
    restriction: 'none',
    squad: null,
    note: null,
  };
}

export function ruleInput(rule: Rule): RuleInput {
  return {
    enabled: rule.enabled,
    audience: rule.audience,
    traffic: rule.traffic,
    devices_min: rule.devices_min,
    period: rule.period,
    limit_gb: Math.round((rule.limit_bytes / GB) * 100) / 100,
    per_device: rule.per_device,
    warn: rule.warn,
    alert: rule.alert,
    restriction: rule.restriction,
    squad: rule.squad,
    note: rule.note,
  };
}

export function actionsText(rule: Rule, squads: Squad[]): string {
  const parts: string[] = [];
  if (rule.warn) {
    parts.push('предупредить клиента');
  }
  if (rule.alert) {
    parts.push('сообщить админам');
  }
  if (rule.restriction === 'disable') {
    parts.push(rule.period === 'day' ? 'отключить до конца суток' : 'отключить до конца недели');
  }
  if (rule.restriction === 'squad') {
    const name = squads.find((s) => s.uuid === rule.squad)?.name;
    parts.push(name ? `перевести в сквад «${name}»` : 'перевести в другой сквад');
  }
  const text = parts.join(', ');
  return text ? text.charAt(0).toUpperCase() + text.slice(1) : '';
}

export function usageText(e: Exceed): string {
  const word = e.period === 'day' ? 'за сутки' : 'за неделю';
  return `${traffic(e.used_bytes)} ${word} при пороге ${traffic(e.limit_bytes)}`;
}

const LIFTED: Record<string, string> = {
  window: 'снято само по окончании окна',
  admin: 'снято вручную',
  ended: 'подписка закончилась',
  blocked: 'клиент заблокирован, доступ не включали',
  changed: 'подписку уже поменяли другим действием',
  off: 'снято при выключении модуля',
};

export function exceedState(e: Exceed): { text: string; sub: string | null; tone: string; active: boolean } {
  if (e.restricted_at && !e.lifted_at) {
    return { text: `Ограничен до ${dateTime(e.lift_due_at ?? e.window_end).toLowerCase()}`, sub: e.restriction === 'squad' ? 'в другом скваде' : 'подписка отключена', tone: 'warn', active: true };
  }
  if (e.restricted_at && e.lifted_at) {
    return { text: 'Ограничение снято', sub: LIFTED[e.lift_reason ?? ''] ?? null, tone: 'ink', active: false };
  }
  if (e.restriction !== 'none' && e.restrict_note) {
    return { text: 'Не ограничен', sub: e.restrict_note, tone: 'muted', active: false };
  }
  if (e.warned_at) {
    return { text: 'Предупреждён', sub: null, tone: 'ink', active: false };
  }
  return { text: 'Превысил порог', sub: null, tone: 'ink', active: false };
}

export function limitsLead(summary: LimitsSummary): string {
  if (summary.rules === 0) {
    return 'Правил пока нет: трафик не проверяется. Добавьте порог, и проверка начнётся в течение 10 минут.';
  }
  const parts: string[] = [];
  parts.push(`${count(summary.rules, 'правило', 'правила', 'правил')} в работе, проверка каждые 10 минут`);
  if (summary.last_scan) {
    parts.push(`последняя — ${ago(summary.last_scan.at)}, ${count(summary.last_scan.subscriptions, 'подписка', 'подписки', 'подписок')}`);
  }
  let text = `${parts.join(', ')}.`;
  if (summary.current > 0 || summary.restricted > 0) {
    text += ` Сейчас превысили: ${summary.current}, ограничены: ${summary.restricted}.`;
  } else {
    text += ' Сейчас никто не превысил порог.';
  }
  return text;
}

export function windowNote(summary: LimitsSummary | null): string {
  if (!summary) {
    return '';
  }
  const minutes = summary.offset_minutes;
  const sign = minutes < 0 ? '−' : '+';
  const abs = Math.abs(minutes);
  const zone = abs % 60 === 0 ? `${abs / 60}` : `${Math.floor(abs / 60)}:${String(abs % 60).padStart(2, '0')}`;
  return `Сутки и неделя считаются по часам панели Remnawave (UTC${sign}${zone}), неделя начинается в понедельник.`;
}
