import { useEffect, useState } from 'react';

export type Theme = 'dark' | 'light';
export type Surface = 'admin' | 'admin-phone' | 'cabinet' | 'cabinet-phone' | 'mini' | 'landing';

function systemTheme(): Theme {
  if (typeof window === 'undefined' || !window.matchMedia) {
    return 'dark';
  }
  return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}

export function useSystemTheme(): Theme {
  const [theme, setTheme] = useState<Theme>(systemTheme);
  useEffect(() => {
    if (!window.matchMedia) {
      return;
    }
    const query = window.matchMedia('(prefers-color-scheme: light)');
    const onChange = () => setTheme(query.matches ? 'light' : 'dark');
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
  }, []);
  return theme;
}

export function useSurface(surface: Surface, theme: Theme, design: 'a' | 'b'): void {
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.surface = surface;
    root.dataset.theme = theme;
    root.dataset.design = design;
  }, [surface, theme, design]);
}

export function readStored(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function writeStored(key: string, value: string | null): void {
  try {
    if (value === null) {
      window.localStorage.removeItem(key);
    } else {
      window.localStorage.setItem(key, value);
    }
  } catch {
    return;
  }
}
