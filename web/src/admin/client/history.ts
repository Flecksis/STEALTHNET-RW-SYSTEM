import { count, dateLong, days, money, periodLabel, timeHM, type Money } from '../../lib/format';
import type { Amount, HistoryItem, OrderBrief, Person } from './types';

export type HistoryLine = { key: string; when: string; what: string; note: string; sum: string };

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

export function historyWhen(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  const diff = Math.round((startOfDay(new Date()) - startOfDay(date)) / 86_400_000);
  if (diff === 0) {
    return `Сегодня, ${timeHM(iso)}`;
  }
  if (diff === 1) {
    return `Вчера, ${timeHM(iso)}`;
  }
  return dateLong(iso);
}

export function amountText(amount: Amount): string {
  if (amount.currency === 'XTR') {
    return count(amount.minor, 'звезда', 'звезды', 'звёзд');
  }
  return money({ minor: amount.minor, currency: amount.currency });
}

const CRYPTO = new Set(['cryptopay', 'heleket']);
const CARD = new Set(['bank_card', 'card']);

export function methodText(provider: string, method: string | null): string {
  const m = (method ?? '').toLowerCase();
  if (provider === 'balance') {
    return 'С баланса';
  }
  if (provider === 'stars') {
    return 'Звёзды Telegram';
  }
  if (CRYPTO.has(provider)) {
    return method ? `Криптовалюта, ${method.toUpperCase()}` : 'Криптовалюта';
  }
  if (m === 'sbp') {
    return 'СБП';
  }
  if (CARD.has(m)) {
    return 'Карта';
  }
  if (m === 'yoo_money' || provider === 'yoomoney') {
    return 'ЮMoney';
  }
  if (m === 'sberbank' || m === 'sber_pay') {
    return 'SberPay';
  }
  if (provider === 'cloudpayments' || provider === 'lava' || provider === 'overpay') {
    return 'Карта';
  }
  return method ?? provider;
}

function period(order: OrderBrief): string {
  return periodLabel(order.days) ?? '';
}

function bought(order: OrderBrief | null): string {
  if (!order) {
    return 'Оплатил заказ';
  }
  const name = order.tariff_name ? `«${order.tariff_name}»` : 'подписку';
  const span = period(order);
  switch (order.kind) {
    case 'top_up':
      return 'Пополнил баланс';
    case 'change':
      return order.tariff_name ? `Перешёл на «${order.tariff_name}»` : 'Сменил тариф';
    case 'module':
      return order.label ? `Оплатил: ${order.label}` : 'Оплатил дополнение';
    case 'trial':
      return 'Пробный период';
    default:
      return span ? `Оплатил ${name} на ${span}` : `Оплатил ${name}`;
  }
}

function actorNote(item: Extract<HistoryItem, { type: 'audit' }>): string {
  if (item.actor_kind === 'admin') {
    return item.actor_email ?? 'Администратор';
  }
  if (item.actor_kind === 'client') {
    return 'Сам клиент';
  }
  return 'Автоматически';
}

function num(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function str(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function cut(text: string, max = 90): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

const AUDIT_TEXT: Record<string, string> = {
  'client.note': 'Изменена заметка',
  'client.unblock': 'Разблокирован',
  'client.telegram_login_confirmed': 'Вошёл в кабинет через Telegram',
  'subscription.reset_traffic': 'Сброшен трафик',
  'subscription.reissue_link': 'Выпущена новая ссылка подписки',
  'subscription.disconnect_all_devices': 'Отключены все устройства',
  'subscription.disconnect_device': 'Отключено устройство',
  'subscription.change_limits': 'Изменены лимиты',
  'subscription.block_access': 'Доступ к подписке закрыт',
  'subscription.restore_access': 'Доступ к подписке открыт',
  'subscription.release_devices': 'Освобождены места для устройств',
  'subscription.blocked_delivery': 'Выдача остановлена: клиент заблокирован',
  'payment.mark_paid': 'Платёж отмечен оплаченным вручную',
  'payment.refund': 'Оформлен возврат',
  'payment.refund_requested': 'Запрошен возврат',
  'payment.refund_confirmed': 'Возврат подтверждён',
  'payment.refund_confirmed_manually': 'Возврат подтверждён вручную',
  'payment.refund_declined': 'Возврат отклонён',
  'payment.refund_canceled': 'Возврат отменён',
  'order.redeliver': 'Заказ выдан повторно',
  'order.mark_fulfilled': 'Заказ отмечен выданным',
  'order.recalculate': 'Заказ пересчитан',
  'order.review': 'Заказ отправлен на проверку',
};

const SOURCE_TEXT: Record<string, string> = {
  bot: 'Пришёл в бот',
  miniapp: 'Открыл мини-приложение',
  web: 'Зарегистрировался в кабинете',
  admin: 'Добавлен администратором',
  import: 'Перенесён из прежней панели',
  migration: 'Перенесён из прежней панели',
};

function personText(person: Person): string {
  if (person.telegram_username) {
    return `@${person.telegram_username}`;
  }
  return person.name ?? `клиент ${person.client_id}`;
}

export function historyLine(item: HistoryItem, index: number, referrer: Person | null): HistoryLine {
  const base = { key: `${item.type}-${item.at}-${index}`, when: historyWhen(item.at) };
  switch (item.type) {
    case 'payment': {
      const { payment, order } = item;
      const what = bought(order);
      const via = methodText(payment.provider, payment.method);
      const sum = amountText(payment.amount);
      if (payment.status === 'failed') {
        return { ...base, what: 'Оплата не прошла', note: `${what}, ${via}`, sum: '' };
      }
      if (payment.status === 'refunded') {
        return { ...base, what, note: `${via}, деньги вернули`, sum };
      }
      if (payment.status === 'partially_refunded') {
        return { ...base, what, note: `${via}, часть денег вернули`, sum };
      }
      return { ...base, what, note: via, sum };
    }
    case 'order': {
      const { order } = item;
      if (order.kind === 'trial') {
        return { ...base, what: order.days ? `Взял пробный период на ${days(order.days)}` : 'Взял пробный период', note: '', sum: '' };
      }
      const name = order.tariff_name ? `«${order.tariff_name}»` : 'подписка';
      const span = period(order);
      if (order.kind === 'admin_grant') {
        return { ...base, what: span ? `Выдана ${name} на ${span}` : `Выдана ${name}`, note: 'Без оплаты, администратор', sum: '' };
      }
      return { ...base, what: span ? `Получил ${name} на ${span}` : `Получил ${name}`, note: 'Без оплаты', sum: '' };
    }
    case 'event': {
      const event = item.event;
      switch (item.kind) {
        case 'client_registered': {
          const source = SOURCE_TEXT[String(event.source)] ?? 'Зарегистрировался';
          if (referrer) {
            return { ...base, what: `${source} по приглашению`, note: `Пригласил ${personText(referrer)}`, sum: '' };
          }
          const mark = str(event.start_param);
          return { ...base, what: source, note: mark ? `Метка «${cut(mark, 40)}»` : '', sum: '' };
        }
        case 'subscription_status_changed':
          return { ...base, what: event.to === 'limited' ? 'Закончился трафик' : 'Подписка истекла', note: '', sum: '' };
        case 'ticket_opened':
          return { ...base, what: 'Написал в поддержку', note: item.ticket?.subject ? `«${cut(item.ticket.subject)}»` : '', sum: '' };
        case 'ticket_answered':
          return { ...base, what: 'Поддержка ответила', note: item.ticket?.subject ? `«${cut(item.ticket.subject)}»` : '', sum: '' };
        case 'balance_changed': {
          const delta = event.delta as Money | undefined;
          const plus = (delta?.minor ?? 0) >= 0;
          const sum = delta ? `${plus ? '+' : ''}${money(delta)}` : '';
          const kind = String(event.kind);
          const what =
            kind === 'admin_adjust'
              ? plus
                ? 'Баланс пополнен администратором'
                : 'Списание с баланса администратором'
              : kind === 'referral_credit'
                ? 'Начислено за приглашённого друга'
                : kind === 'refund'
                  ? 'Возврат на баланс'
                  : 'Изменение баланса';
          return { ...base, what, note: event.wallet === 'referral' ? 'Реферальный баланс' : '', sum };
        }
        default:
          return { ...base, what: item.kind, note: '', sum: '' };
      }
    }
    case 'audit': {
      const details = item.details ?? {};
      const actor = actorNote(item);
      switch (item.action) {
        case 'subscription.add_days': {
          const n = num(details.days);
          return { ...base, what: n ? `Добавлено ${days(n)}` : 'Добавлены дни', note: actor, sum: '' };
        }
        case 'subscription.add_days_undone': {
          const n = num(details.days);
          return { ...base, what: n ? `Отменено добавление ${days(n)}` : 'Отменено добавление дней', note: actor, sum: '' };
        }
        case 'client.block': {
          const reason = str(details.reason);
          return { ...base, what: 'Заблокирован', note: reason ? `${actor}: ${cut(reason, 70)}` : actor, sum: '' };
        }
        case 'client.message': {
          const text = str(details.text);
          return { ...base, what: 'Отправлено сообщение в бот', note: text ? `${actor}: «${cut(text, 70)}»` : actor, sum: '' };
        }
        default:
          return { ...base, what: AUDIT_TEXT[item.action] ?? `Действие: ${item.action}`, note: actor, sum: '' };
      }
    }
  }
}

export function eventsCount(total: number): string {
  return count(total, 'событие', 'события', 'событий');
}
