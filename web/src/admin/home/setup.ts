import type { Permission } from '../../lib/types';
import type { Setup } from './model';
import type { Todo } from './todo';

export function setupTodos(setup: Setup | null | undefined, can: (permission: Permission) => boolean): Todo[] {
  if (!setup) {
    return [];
  }
  const out: Todo[] = [];
  if (!setup.remnawave && can('settings')) {
    out.push({
      key: 'setup-remnawave',
      title: 'Подключите Remnawave',
      note: 'Адрес панели и API-токен',
      tone: 'warn',
      action: 'Подключить',
      label: 'Подключить Remnawave',
      to: '/admin/settings/remnawave',
    });
  }
  if (!setup.bot && can('settings')) {
    out.push({
      key: 'setup-bot',
      title: 'Подключите бота',
      note: 'Токен бота из @BotFather',
      tone: 'warn',
      action: 'Подключить',
      label: 'Подключить бота',
      to: '/admin/settings/bot',
    });
  }
  if (!setup.tariffs && can('tariffs')) {
    out.push({
      key: 'setup-tariffs',
      title: 'Создайте тариф',
      note: 'В продаже нет ни одного тарифа',
      tone: 'warn',
      action: 'Создать',
      label: 'Создать тариф',
      to: '/admin/tariffs',
    });
  }
  return out;
}
