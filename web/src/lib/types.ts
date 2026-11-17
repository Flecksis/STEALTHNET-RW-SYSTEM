export type AdminDesign = 'side' | 'top';
export type AdminTheme = 'dark' | 'light' | 'system';
export type ClientDesign = 'accent' | 'mono';
export type ClientTheme = 'dark' | 'light' | 'client';

export type Appearance = {
  admin: AdminDesign;
  admin_theme: AdminTheme;
  client: ClientDesign;
  client_theme: ClientTheme;
};

export type Currency = 'USD' | 'RUB';

export type Site = {
  brand_name: string;
  currency: Currency;
  support_url: string | null;
  bot_username: string | null;
  appearance: Appearance;
};

export type Permission =
  | 'clients_read'
  | 'clients_write'
  | 'money'
  | 'refunds'
  | 'tariffs'
  | 'infrastructure'
  | 'support'
  | 'marketing'
  | 'modules'
  | 'settings'
  | 'admins';

export type AdminRole = 'owner' | 'admin' | 'support' | 'read_only';

export type ModuleMenuItem = {
  module: string;
  label: string;
  icon: string;
  order: number;
};

export type Me = {
  admin: { admin_id: number; role: AdminRole; permissions: Permission[]; session_id: number };
  email: string | null;
  permissions: Permission[];
  menu: ModuleMenuItem[];
  currency: Currency;
  brand_name: string;
  appearance: Appearance;
};
