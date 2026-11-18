import type { Permission } from '../../lib/types';
import type { SettingField } from '../SchemaFields';

export type Change = 'same' | 'added' | 'removed';

export type ExtEvent = { kind: string; title: string; about: string; change: Change };

export type ExtPermission = { key: Permission; title: string; about: string; sensitive: boolean; change: Change };

export type ExtPage = { url: string; title: string; permission: Permission; permission_title: string };

export type ExtSummary = {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string | null;
  enabled: boolean;
  has_settings: boolean;
  page: ExtPage | null;
  warning: string | null;
};

export type KeyState = 'active' | 'missing' | 'revoked' | 'expired' | 'owner_disabled';

export type ExtCard = {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string | null;
  site: string | null;
  webhook: string | null;
  enabled: boolean;
  events: ExtEvent[];
  permissions: ExtPermission[];
  page: ExtPage | null;
  has_settings: boolean;
  key: { prefix: string | null; state: KeyState; owner: string | null; last_used_at: string | null };
  installed_by: string;
  installed_at: string;
  updated_at: string;
  stats: { pending: number; failed: number; last_ok: boolean | null; last_at: string | null };
  warning: string | null;
};

export type Problem = { path: string; message: string };

export type Moved = { address: 'webhook' | 'page'; before: string | null; after: string | null };

export type Preview = {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string | null;
  site: string | null;
  webhook: string | null;
  page: ExtPage | null;
  settings: string[];
  events: ExtEvent[];
  permissions: ExtPermission[];
  sensitive: boolean;
  installed: { version: string; enabled: boolean } | null;
  moved: Moved[];
};

export type PreviewResult = { errors: Problem[]; preview: Preview | null };

export type Credentials = { api_key: string; signing_secret: string };

export type InstallResult = { module: ExtCard; created: boolean; credentials: Credentials | null };

export type ExtConfig = { module: string; schema: SettingField[]; config: Record<string, unknown> };

export type Attempt = {
  id: number;
  delivery_id: number;
  kind: string;
  title: string;
  event_id: number | null;
  attempt: number;
  at: string;
  http_status: number | null;
  duration_ms: number;
  error: string | null;
  response: string | null;
  status: 'pending' | 'done' | 'failed' | 'skipped';
  next_attempt_at: string | null;
  can_retry: boolean;
};

export type Journal = { attempts: Attempt[]; stats: ExtCard['stats'] };

export type PingResult = { ok: boolean; http_status: number | null; duration_ms: number; error: string | null; response: string | null };

export type PageToken = { url: string; origin: string; token: string; expires_at: string };

export const KEY_TEXT: Record<KeyState, string> = {
  active: 'работает',
  missing: 'нет ключа',
  revoked: 'отозван',
  expired: 'срок истёк',
  owner_disabled: 'не работает: администратор отключён',
};

export const MANIFEST_LIMIT = 64 * 1024;

export function extLink(id: string): string {
  return `/admin/modules/ext/${encodeURIComponent(id)}`;
}
