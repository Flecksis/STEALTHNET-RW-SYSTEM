import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { sameRows, type Draft, type LayoutView, type MenuMode, type Rows, type ScreenKey, type ScreenView } from './model';

type Drafts = Record<ScreenKey, Draft>;
type Status = { text: string; bad: boolean } | null;

function draftsOf(view: LayoutView): Drafts {
  const out = {} as Drafts;
  for (const screen of view.screens) {
    out[screen.key] = { rows: screen.rows, custom: screen.custom };
  }
  return out;
}

function changedScreen(screen: ScreenView, draft: Draft | undefined): boolean {
  return draft !== undefined && (draft.custom !== screen.custom || !sameRows(draft.rows, screen.rows));
}

export function useLayoutEditor() {
  const [view, setView] = useState<LayoutView | null>(null);
  const [drafts, setDrafts] = useState<Drafts | null>(null);
  const [hidden, setHidden] = useState<Record<string, boolean>>({});
  const [mode, setMode] = useState<MenuMode | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<Status>(null);

  const apply = useCallback((data: LayoutView) => {
    setView(data);
    setDrafts(draftsOf(data));
    setHidden({});
    setMode(null);
  }, []);

  useEffect(() => {
    let alive = true;
    api<LayoutView>('/api/admin/bot/layout')
      .then((data) => {
        if (alive) {
          apply(data);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (alive) {
          setError(err instanceof ApiError ? err.message : 'Порядок кнопок не загрузился');
        }
      });
    return () => {
      alive = false;
    };
  }, [apply]);

  const hiddenChanges = useMemo(() => {
    if (!view) {
      return {} as Record<string, boolean>;
    }
    const out: Record<string, boolean> = {};
    for (const screen of view.screens) {
      for (const slot of screen.slots) {
        const want = hidden[slot.key];
        if (want !== undefined && want !== slot.hidden) {
          out[slot.key] = want;
        }
      }
    }
    return out;
  }, [view, hidden]);

  const changedScreens = useMemo(
    () => (view && drafts ? view.screens.filter((screen) => changedScreen(screen, drafts[screen.key])) : []),
    [view, drafts],
  );

  const menuMode: MenuMode = mode ?? view?.menu_mode ?? 'classic';
  const modeChanged = view !== null && menuMode !== view.menu_mode;

  const unsaved = changedScreens.length + Object.keys(hiddenChanges).length + (modeChanged ? 1 : 0);

  const setRows = useCallback((key: ScreenKey, rows: Rows) => {
    setDrafts((current) => (current ? { ...current, [key]: { rows, custom: true } } : current));
    setStatus(null);
  }, []);

  const resetScreen = useCallback(
    (key: ScreenKey) => {
      const screen = view?.screens.find((s) => s.key === key);
      if (!screen) {
        return;
      }
      setDrafts((current) => (current ? { ...current, [key]: { rows: screen.standard, custom: false } } : current));
      setStatus(null);
    },
    [view],
  );

  const toggleHidden = useCallback(
    (key: string, value: boolean) => {
      setHidden((current) => ({ ...current, [key]: value }));
      setStatus(null);
    },
    [],
  );

  const setMenuMode = useCallback((next: MenuMode) => {
    setMode(next);
    setStatus(null);
  }, []);

  const isHidden = useCallback(
    (key: string, fallback: boolean) => hidden[key] ?? fallback,
    [hidden],
  );

  const cancel = useCallback(() => {
    if (view) {
      apply(view);
      setStatus(null);
    }
  }, [view, apply]);

  const save = useCallback(async () => {
    if (!view || !drafts || unsaved === 0 || busy) {
      return;
    }
    const layouts: Record<string, Rows | null> = {};
    for (const screen of changedScreens) {
      const draft = drafts[screen.key];
      layouts[screen.key] = draft.custom ? draft.rows : null;
    }
    setBusy(true);
    setStatus(null);
    try {
      const body = modeChanged ? { layouts, hidden: hiddenChanges, menu_mode: menuMode } : { layouts, hidden: hiddenChanges };
      const saved = await api<LayoutView>('/api/admin/bot/layout', { method: 'PUT', body });
      apply(saved);
      setStatus({ text: 'Сохранено: бот уже показывает кнопки по-новому', bad: false });
    } catch (err) {
      setStatus({ text: err instanceof ApiError ? err.message : 'Порядок кнопок не сохранился', bad: true });
    } finally {
      setBusy(false);
    }
  }, [view, drafts, unsaved, busy, changedScreens, hiddenChanges, modeChanged, menuMode, apply]);

  return { view, drafts, error, busy, status, unsaved, menuMode, modeChanged, setMenuMode, setRows, resetScreen, toggleHidden, isHidden, hidden, cancel, save };
}
