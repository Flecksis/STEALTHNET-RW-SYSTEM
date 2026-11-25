import { api, ApiError } from '../lib/api';
import { appScheme } from '../lib/appScheme';
import { t } from '../lib/i18n';
import { webApp } from './telegram';

export type Spot = { platform: string; app: string; step: number; button?: number };

export const JUMP_WAIT_MS = 8000;

export function needsJump(link: string): boolean {
  return Boolean(webApp()?.openLink) && appScheme(link) !== null;
}

export async function jump(subscription: number, spot: Spot): Promise<void> {
  const controller = new AbortController();
  const timer = window.setTimeout(() => controller.abort(), JUMP_WAIT_MS);
  let token: string;
  try {
    ({ token } = await api<{ token: string }>(`/api/client/subscriptions/${subscription}/app-link`, { method: 'POST', body: spot, signal: controller.signal }));
  } catch (err) {
    throw err instanceof ApiError ? err : new ApiError(0, 'timeout', t('Сервер долго не отвечает. Попробуйте ещё раз'));
  } finally {
    window.clearTimeout(timer);
  }
  try {
    webApp()?.openLink?.(`${window.location.origin}/open-app#${token}`);
  } catch {
    throw new ApiError(0, 'openLink', t('Telegram не открыл браузер. Попробуйте ещё раз'));
  }
}
