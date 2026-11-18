import { Switch } from './ui';
import './form.css';

export type FieldKind = { type: 'text' | 'secret' | 'number' | 'flag' | 'url' | 'list' } | { type: 'select'; options: string[] };

export type SettingField = {
  key: string;
  label: string;
  hint: string;
  kind: FieldKind;
  required: boolean;
  default: string | null;
  large?: boolean;
};

export type FieldValues = Record<string, string | boolean>;

export type SecretState = { clear: string[]; replacing: string[] };

export function fieldInitial(field: SettingField, config: Record<string, unknown>): string | boolean {
  const stored = config[field.key];
  if (field.kind.type === 'secret') {
    return '';
  }
  if (field.kind.type === 'flag') {
    return typeof stored === 'boolean' ? stored : field.default === 'true';
  }
  if (stored === null || stored === undefined) {
    return field.default ?? '';
  }
  return String(stored);
}

export function initialValues(schema: SettingField[], config: Record<string, unknown>): FieldValues {
  return Object.fromEntries(schema.filter((f) => f.kind.type !== 'list').map((f) => [f.key, fieldInitial(f, config)]));
}

export function secretFilled(config: Record<string, unknown>, key: string): boolean {
  const value = config[key];
  return typeof value === 'object' && value !== null && (value as { filled?: boolean }).filled === true;
}

export function changedConfig(schema: SettingField[], config: Record<string, unknown>, values: FieldValues): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const field of schema) {
    const value = values[field.key];
    if (field.kind.type === 'list') {
      continue;
    }
    if (field.kind.type === 'secret') {
      if (typeof value === 'string' && value.trim()) {
        out[field.key] = value.trim();
      }
      continue;
    }
    if (value === fieldInitial(field, config)) {
      continue;
    }
    out[field.key] = value === '' ? null : value;
  }
  return out;
}

export function missingRequired(schema: SettingField[], config: Record<string, unknown>, values: FieldValues, secrets: SecretState): SettingField[] {
  return schema.filter((field) => {
    if (!field.required || field.kind.type === 'list') {
      return false;
    }
    const value = values[field.key];
    if (field.kind.type === 'secret') {
      const kept = secretFilled(config, field.key) && !secrets.clear.includes(field.key);
      return !kept && !(typeof value === 'string' && value.trim());
    }
    if (field.kind.type === 'flag') {
      return false;
    }
    return typeof value !== 'string' || value.trim() === '';
  });
}

type Props = {
  schema: SettingField[];
  config: Record<string, unknown>;
  values: FieldValues;
  onChange: (values: FieldValues) => void;
  secrets: SecretState;
  onSecrets: (secrets: SecretState) => void;
  busy: boolean;
  idPrefix: string;
};

export function SchemaFields({ schema, config, values, onChange, secrets, onSecrets, busy, idPrefix }: Props) {
  const set = (key: string, value: string | boolean) => onChange({ ...values, [key]: value });
  return (
    <>
      {schema.map((field) => {
        if (field.kind.type === 'list') {
          return null;
        }
        const value = values[field.key];
        const id = `${idPrefix}-${field.key}`;
        const hintId = field.hint ? `${id}-hint` : undefined;
        const hint = field.hint ? (
          <span id={hintId} className="cap">
            {field.hint}
          </span>
        ) : null;
        if (field.kind.type === 'flag') {
          return (
            <div key={field.key} className="sf-flag">
              <span className="sf-text">
                <span className="sf-label">{field.label}</span>
                {hint}
              </span>
              <Switch on={value === true} onToggle={() => set(field.key, value !== true)} label={field.label} describedBy={hintId} disabled={busy} />
            </div>
          );
        }
        if (field.kind.type === 'secret') {
          const filled = secretFilled(config, field.key) && !secrets.clear.includes(field.key);
          const editing = !filled || secrets.replacing.includes(field.key);
          return (
            <div key={field.key} className="sf-field">
              <label htmlFor={id} className="sf-label">
                {field.label}
              </label>
              {editing ? (
                <input
                  id={id}
                  className="field mono"
                  type="password"
                  autoComplete="off"
                  spellCheck={false}
                  value={typeof value === 'string' ? value : ''}
                  onChange={(e) => set(field.key, e.target.value)}
                  aria-describedby={hintId}
                  placeholder={filled ? 'Новое значение' : 'Вставьте значение'}
                  disabled={busy}
                />
              ) : (
                <span className="sf-secret">
                  <span className="muted">Сохранено</span>
                  <button type="button" className="b34 hov" onClick={() => onSecrets({ ...secrets, replacing: [...secrets.replacing, field.key] })} disabled={busy}>
                    Заменить
                  </button>
                  {!field.required ? (
                    <button type="button" className="b34 hov" onClick={() => onSecrets({ ...secrets, clear: [...secrets.clear, field.key] })} disabled={busy}>
                      Удалить
                    </button>
                  ) : null}
                </span>
              )}
              {hint}
            </div>
          );
        }
        return (
          <div key={field.key} className="sf-field">
            <label htmlFor={id} className="sf-label">
              {field.label}
            </label>
            {field.kind.type === 'text' && field.large ? (
              <textarea
                id={id}
                className="field"
                rows={8}
                value={String(value ?? '')}
                onChange={(e) => set(field.key, e.target.value)}
                aria-describedby={hintId}
                disabled={busy}
              />
            ) : field.kind.type === 'select' ? (
              <select id={id} className="field" value={String(value ?? '')} onChange={(e) => set(field.key, e.target.value)} aria-describedby={hintId} disabled={busy}>
                {field.required ? null : <option value="">Не выбрано</option>}
                {field.kind.options.map((option) => (
                  <option key={option} value={option}>
                    {option}
                  </option>
                ))}
              </select>
            ) : (
              <input
                id={id}
                className="field"
                type={field.kind.type === 'url' ? 'url' : 'text'}
                inputMode={field.kind.type === 'number' ? 'decimal' : undefined}
                value={String(value ?? '')}
                onChange={(e) => set(field.key, e.target.value)}
                aria-describedby={hintId}
                disabled={busy}
              />
            )}
            {hint}
          </div>
        );
      })}
    </>
  );
}
