import { useCallback, useEffect, useState } from 'react';
import { api } from '../../lib/api';
import { without, type CssProblems, type CssReport, type CssTexts } from './cssText';

const CHECK_DELAY_MS = 350;

export type CssCheck<P extends string> = { styles: CssTexts<P>; problems: CssProblems<P>; reset: (texts: CssTexts<P>, styles: CssTexts<P>) => void };

export function useCssCheck<P extends string>(path: string, all: P[], texts: CssTexts<P>): CssCheck<P> {
  const [styles, setStyles] = useState<CssTexts<P>>({});
  const [problems, setProblems] = useState<CssProblems<P>>({});
  const [checked, setChecked] = useState<CssTexts<P>>({});

  useEffect(() => {
    const places = all.filter((place) => (texts[place] ?? '') !== (checked[place] ?? ''));
    if (!places.length) {
      return;
    }
    let alive = true;
    const filled = places.filter((place) => texts[place]?.trim());
    const settle = (report: CssReport<P>) => {
      if (!alive) {
        return;
      }
      setStyles((current) => {
        const next = without(current, places);
        for (const place of filled) {
          next[place] = report[place]?.style ?? current[place] ?? '';
        }
        return next;
      });
      setProblems((current) => {
        const next = without(current, places);
        for (const place of filled) {
          const entry = report[place];
          if (entry?.problem) {
            next[place] = { line: entry.line ?? null, text: entry.problem };
          }
        }
        return next;
      });
      setChecked((current) => {
        const next = without(current, places);
        for (const place of places) {
          next[place] = texts[place] ?? '';
        }
        return next;
      });
    };
    const timer = window.setTimeout(
      () => {
        if (!filled.length) {
          settle({});
          return;
        }
        const css = Object.fromEntries(filled.map((place) => [place, texts[place]]));
        api<CssReport<P>>(path, { method: 'POST', body: { css } })
          .then(settle)
          .catch(() => undefined);
      },
      filled.length ? CHECK_DELAY_MS : 0,
    );
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [path, all, texts, checked]);

  const reset = useCallback(
    (nextTexts: CssTexts<P>, nextStyles: CssTexts<P>) => {
      const refused = all.filter((place) => nextTexts[place] && !nextStyles[place]);
      setChecked(without(nextTexts, refused));
      setStyles(nextStyles);
      setProblems({});
    },
    [all],
  );

  return { styles, problems, reset };
}
