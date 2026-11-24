export type Side = 'top' | 'right' | 'bottom' | 'left';
export type InsetBox = Partial<Record<Side, number>>;

export type InsetSource = {
  safeAreaInset?: InsetBox | null;
  contentSafeAreaInset?: InsetBox | null;
  onEvent?: (name: string, cb: () => void) => void;
  offEvent?: (name: string, cb: () => void) => void;
};

export type StyleTarget = {
  style: {
    setProperty: (name: string, value: string) => void;
    removeProperty: (name: string) => string;
  };
};

export const INSET_EVENTS = ['safeAreaChanged', 'contentSafeAreaChanged', 'fullscreenChanged'] as const;

const SIDES: Side[] = ['top', 'right', 'bottom', 'left'];

function px(value: unknown): string {
  const n = typeof value === 'number' ? value : typeof value === 'string' ? Number.parseFloat(value) : Number.NaN;
  return `${Number.isFinite(n) && n > 0 ? Math.round(n * 100) / 100 : 0}px`;
}

export function insetVars(app: InsetSource | null | undefined): Record<string, string> {
  const vars: Record<string, string> = {};
  const groups: Array<[string, InsetBox | null | undefined]> = [
    ['safe', app?.safeAreaInset],
    ['content', app?.contentSafeAreaInset],
  ];
  for (const [group, box] of groups) {
    if (!box || typeof box !== 'object') {
      continue;
    }
    for (const side of SIDES) {
      vars[`--mn-tg-${group}-${side}`] = px(box[side]);
    }
  }
  return vars;
}

export function watchInsets(app: InsetSource, root: StyleTarget): () => void {
  let names: string[] = [];
  const apply = () => {
    const vars = insetVars(app);
    for (const name of names) {
      if (!(name in vars)) {
        root.style.removeProperty(name);
      }
    }
    for (const [name, value] of Object.entries(vars)) {
      root.style.setProperty(name, value);
    }
    names = Object.keys(vars);
  };
  apply();
  for (const name of INSET_EVENTS) {
    app.onEvent?.(name, apply);
  }
  return () => {
    for (const name of INSET_EVENTS) {
      app.offEvent?.(name, apply);
    }
    for (const name of names) {
      root.style.removeProperty(name);
    }
    names = [];
  };
}
