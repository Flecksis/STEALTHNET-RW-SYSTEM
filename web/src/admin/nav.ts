import type { IconName } from '../lib/icons';
import { isIconName } from '../lib/icons';
import type { Me, Permission } from '../lib/types';

export type NavItem = {
  key: string;
  label: string;
  icon: IconName;
  to: string;
  group: 'core' | 'module' | 'system';
  badge?: number;
  dot?: boolean;
};

const CORE: Array<{ key: string; label: string; icon: IconName; to: string; need?: Permission }> = [
  { key: 'home', label: 'Главная', icon: 'home', to: '/admin' },
  { key: 'clients', label: 'Клиенты', icon: 'users', to: '/admin/clients', need: 'clients_read' },
  { key: 'support', label: 'Поддержка', icon: 'chat', to: '/admin/support', need: 'support' },
  { key: 'money', label: 'Деньги', icon: 'card', to: '/admin/money', need: 'money' },
  { key: 'tariffs', label: 'Тарифы', icon: 'tag', to: '/admin/tariffs', need: 'tariffs' },
  { key: 'infra', label: 'Инфраструктура', icon: 'server', to: '/admin/infra', need: 'infrastructure' },
];

export function navItems(me: Me, can: (p: Permission) => boolean, supportBadge?: number, updateDot?: boolean): NavItem[] {
  const items: NavItem[] = CORE.filter((item) => !item.need || can(item.need)).map((item) => ({
    key: item.key,
    label: item.label,
    icon: item.icon,
    to: item.to,
    group: 'core',
    badge: item.key === 'support' && supportBadge ? supportBadge : undefined,
  }));
  for (const entry of [...me.menu].sort((a, b) => a.order - b.order)) {
    items.push({
      key: `m-${entry.module}`,
      label: entry.label,
      icon: isIconName(entry.icon) ? entry.icon : 'blocks',
      to: `/admin/m/${entry.module}`,
      group: 'module',
    });
  }
  if (can('modules')) {
    items.push({ key: 'modules', label: 'Модули', icon: 'blocks', to: '/admin/modules', group: 'system' });
  }
  if (can('settings')) {
    items.push({ key: 'settings', label: 'Настройки', icon: 'sliders', to: '/admin/settings', group: 'system', dot: updateDot || undefined });
  }
  return items;
}

export function isCurrent(item: NavItem, path: string): boolean {
  if (item.to === '/admin') {
    return path === '/admin' || path === '/admin/';
  }
  return path === item.to || path.startsWith(`${item.to}/`);
}
