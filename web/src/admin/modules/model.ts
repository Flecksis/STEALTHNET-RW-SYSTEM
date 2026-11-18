import type { SettingField } from '../SchemaFields';

export type ModuleGroup = 'sales' | 'marketing' | 'support' | 'protection' | 'accounting' | 'site' | 'rare';

export type ModuleInfo = {
  id: string;
  title: string;
  description: string;
  where_used: string;
  group: ModuleGroup;
  enabled_by_default: boolean;
  menu: { label: string; icon: string; order: number } | null;
  only_currency: 'USD' | 'RUB' | null;
  needs_api_key: boolean;
};

export type ModuleView = {
  info: ModuleInfo;
  enabled: boolean;
  active: boolean;
  locked: string | null;
  warning: string | null;
  has_settings: boolean;
};

export type ModulesData = { modules: ModuleView[]; core: string[] };

export type ModuleConfig = {
  module: string;
  schema: SettingField[];
  config: Record<string, unknown>;
};

export const GROUPS: Array<[ModuleGroup, string]> = [
  ['sales', 'Продажи'],
  ['marketing', 'Маркетинг'],
  ['support', 'Поддержка и связь'],
  ['protection', 'Защита'],
  ['accounting', 'Учёт'],
  ['site', 'Сайт и вход'],
  ['rare', 'Редко нужно'],
];

const ELSEWHERE: Record<string, string> = {
  promolinks: '/admin/m/promolinks',
  options: '/admin/tariffs',
  antibot: '/admin/clients/antibot',
  limits: '/admin/clients/limits',
  landing: '/admin/settings/landing',
};

export function setupLink(m: ModuleView): string {
  const fixed = ELSEWHERE[m.info.id];
  if (fixed) {
    return fixed;
  }
  if (m.info.menu && !m.has_settings) {
    return `/admin/m/${m.info.id}`;
  }
  return `/admin/modules/${m.info.id}`;
}

export function setupText(m: ModuleView): string {
  return m.warning && m.info.needs_api_key ? 'Указать ключ' : 'Настроить';
}
