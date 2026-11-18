import { count, money, type Money } from '../../lib/format';
import { amountText } from '../client/history';
import type { Amount } from '../client/types';
import { ATTENTION_LINK, attentionCount, attentionNote, attentionTitle, type Attention } from '../money/journal';
import { minutesSince, personName, spanText, type Person, type TicketItem } from './model';

export type DecisionBrief = {
  client_id: number | null;
  client: Person | null;
  amount: Amount | null;
  reason: string | null;
  action: string | null;
};

export type DecisionGroup = { count: number; since: string | null; total: Amount | null; oldest: DecisionBrief | null };

export type Decisions = {
  orders: DecisionGroup | null;
  refund_actions: DecisionGroup | null;
  refund_requests: DecisionGroup | null;
  charges: DecisionGroup | null;
  stars: DecisionGroup | null;
  credits?: DecisionGroup | null;
};

export type TodoTone = 'bad' | 'warn' | 'muted';

export type Todo = {
  key: string;
  title: string;
  note: string;
  tone: TodoTone;
  action: string;
  label: string;
  to?: string;
  href?: string;
  run?: () => void;
  busy?: boolean;
};

export type Withdrawal = { id: number; amount: Money; status: string; created_at: string };

export type Waiting = { count: number; oldest: TicketItem | null };

export const TODO_LINK = {
  refunds: '/admin/money#todo-refunds',
  orders: '/admin/money#todo-orders',
  stars: '/admin/money#todo-stars',
  charges: '/admin/money#todo-charges',
  credits: '/admin/money#todo-credits',
  requests: '/admin/money#todo-requests',
};

function open(group: DecisionGroup | null | undefined): group is DecisionGroup {
  return !!group && group.count > 0;
}

function who(brief: DecisionBrief | null): string | null {
  return brief?.client_id ? personName(brief.client, brief.client_id) : null;
}

function sum(amount: Amount | null | undefined): string | null {
  return amount ? amountText(amount) : null;
}

function span(since: string | null): string | null {
  const minutes = minutesSince(since);
  if (minutes === null) {
    return null;
  }
  return minutes < 1 ? 'меньше минуты' : spanText(minutes);
}

function line(...parts: Array<string | null | undefined>): string {
  return parts.filter(Boolean).join(' · ');
}

export function refundTodo(group: DecisionGroup | null | undefined): Todo | null {
  if (!open(group)) {
    return null;
  }
  const brief = group.oldest;
  const what = brief?.action === 'cut' ? 'срок не укоротили' : 'подписку не отключили';
  const one = group.count === 1;
  return {
    key: 'refunds',
    title: one ? 'Возврат не завершён' : `${count(group.count, 'возврат', 'возврата', 'возвратов')} не завершены`,
    note: one ? line(who(brief), sum(brief?.amount), what) : `${line(who(brief), sum(brief?.amount))} и ещё ${group.count - 1}`,
    tone: 'bad',
    action: 'Открыть',
    label: 'Открыть незавершённые возвраты',
    to: TODO_LINK.refunds,
  };
}

export function webhooksTodo(attention: Attention | null | undefined): Todo | null {
  if (!attention || attentionCount(attention) === 0) {
    return null;
  }
  return {
    key: 'webhooks',
    title: attentionTitle(attention),
    note: attentionNote(attention),
    tone: 'bad',
    action: 'Открыть',
    label: 'Открыть журнал уведомлений платёжек',
    to: ATTENTION_LINK,
  };
}

export function moneyTodos(decisions: Decisions | null | undefined): Todo[] {
  const out: Todo[] = [];
  const orders = decisions?.orders;
  if (open(orders)) {
    const wait = span(orders.since);
    const name = who(orders.oldest);
    out.push({
      key: 'orders',
      title: `${count(orders.count, 'заказ', 'заказа', 'заказов')} на ручной проверке`,
      note: !wait ? '' : orders.count === 1 ? `${name ?? 'Клиент'} ждёт ${wait}` : `Самый старый ждёт ${wait}`,
      tone: 'warn',
      action: 'Разобрать',
      label: 'Разобрать заказы на ручной проверке',
      to: TODO_LINK.orders,
    });
  }
  const stars = decisions?.stars;
  if (open(stars)) {
    const one = stars.count === 1;
    out.push({
      key: 'stars',
      title: one ? 'Оплата звёздами на разборе' : `${count(stars.count, 'оплата', 'оплаты', 'оплат')} звёздами на разборе`,
      note: one ? line(sum(stars.oldest?.amount), stars.oldest?.reason) : stars.total ? `Всего ${amountText(stars.total)}` : '',
      tone: 'warn',
      action: 'Открыть',
      label: 'Открыть оплаты звёздами на разборе',
      to: TODO_LINK.stars,
    });
  }
  const charges = decisions?.charges;
  if (open(charges)) {
    const one = charges.count === 1;
    out.push({
      key: 'charges',
      title: `${count(charges.count, 'списание', 'списания', 'списаний')} с карты на проверке`,
      note: one
        ? line('Автопродление', who(charges.oldest), sum(charges.oldest?.amount))
        : line('Автопродление', charges.total ? `всего ${amountText(charges.total)}` : null),
      tone: 'warn',
      action: 'Открыть',
      label: 'Открыть списания с карты на проверке',
      to: TODO_LINK.charges,
    });
  }
  const credits = decisions?.credits;
  if (open(credits)) {
    const one = credits.count === 1;
    out.push({
      key: 'credits',
      title: one ? 'Деньги без решения' : `${count(credits.count, 'платёж', 'платежа', 'платежей')} без решения`,
      note: one ? line(who(credits.oldest), sum(credits.oldest?.amount)) : credits.total ? `Всего ${amountText(credits.total)}` : '',
      tone: 'warn',
      action: 'Открыть',
      label: 'Открыть деньги без решения',
      to: TODO_LINK.credits,
    });
  }
  const requests = decisions?.refund_requests;
  if (open(requests)) {
    const wait = span(requests.since);
    out.push({
      key: 'requests',
      title: `${count(requests.count, 'заявка', 'заявки', 'заявок')} на возврат без ответа`,
      note:
        requests.count === 1
          ? line(who(requests.oldest), sum(requests.oldest?.amount), wait ? `ждёт ${wait}` : null)
          : wait
            ? `Самая старая ждёт ${wait}`
            : '',
      tone: 'warn',
      action: 'Открыть',
      label: 'Открыть заявки на возврат без ответа',
      to: TODO_LINK.requests,
    });
  }
  return out;
}

export function supportTodo(waiting: Waiting | null): Todo | null {
  if (!waiting || waiting.count === 0) {
    return null;
  }
  const oldest = waiting.oldest;
  const wait = oldest ? span(oldest.waiting_since) : null;
  const name = oldest ? (oldest.client_name ?? (oldest.telegram_username ? `@${oldest.telegram_username}` : (oldest.email ?? `Клиент ${oldest.ticket.client_id}`))) : null;
  return {
    key: 'support',
    title: `${count(waiting.count, 'обращение', 'обращения', 'обращений')} без ответа`,
    note: name && wait ? `${name} ждёт ${wait}` : '',
    tone: 'muted',
    action: 'Ответить',
    label: 'Ответить на обращения',
    to: '/admin/support',
  };
}

export function withdrawalsTodo(withdrawals: Withdrawal[]): Todo | null {
  const first = withdrawals[0];
  if (!first) {
    return null;
  }
  const total = { minor: withdrawals.reduce((acc, w) => acc + w.amount.minor, 0), currency: first.amount.currency };
  return {
    key: 'withdrawals',
    title: `${count(withdrawals.length, 'заявка', 'заявки', 'заявок')} на вывод`,
    note: `Всего ${money(total)}`,
    tone: 'muted',
    action: 'Рассмотреть',
    label: 'Рассмотреть заявки на вывод',
    to: '/admin/m/referral',
  };
}

export function desktopTodos(decisions: Decisions | null | undefined, attention: Attention | null | undefined, waiting: Waiting | null, withdrawals: Withdrawal[]): Todo[] {
  return [refundTodo(decisions?.refund_actions), webhooksTodo(attention), ...moneyTodos(decisions), supportTodo(waiting), withdrawalsTodo(withdrawals)].filter(
    (todo): todo is Todo => todo !== null,
  );
}
