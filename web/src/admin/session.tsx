import { createContext, useContext } from 'react';
import type { Theme } from '../lib/theme';
import type { Appearance, Me, Permission } from '../lib/types';

export type Session = {
  me: Me;
  can: (permission: Permission) => boolean;
  reload: () => Promise<void>;
  logout: () => Promise<void>;
  setAppearance: (appearance: Appearance) => void;
  theme: Theme;
  flipTheme: () => void;
};

export const SessionContext = createContext<Session | null>(null);

export function useSession(): Session {
  const session = useContext(SessionContext);
  if (!session) {
    throw new Error('нет сессии');
  }
  return session;
}

export const ROLE_LABEL: Record<Me['admin']['role'], string> = {
  owner: 'Владелец',
  admin: 'Администратор',
  support: 'Поддержка',
  read_only: 'Наблюдатель',
};
