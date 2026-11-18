import { useEffect, useState } from 'react';
import { money } from '../../lib/format';
import { Icon } from '../../lib/icons';
import type { Currency } from '../../lib/types';
import type { FieldValues, SettingField } from '../SchemaFields';
import { Switch } from '../ui';
import './payouts.css';

export const PAYOUT_KEY = 'payout_methods';

const MAX_METHODS = 10;

type DetailsCheck = 'phone' | 'card' | 'trc20' | 'text';

const CHECKS: Array<{ key: DetailsCheck; label: string; note: string }> = [
  { key: 'phone', label: 'Телефон', note: 'От 10 до 15 цифр, можно дописать банк' },
  { key: 'card', label: 'Номер карты', note: 'От 13 до 19 цифр, с проверкой контрольной цифры' },
  { key: 'trc20', label: 'Кошелёк TRC20', note: 'Адрес из 34 символов, начинается с T' },
  { key: 'text', label: 'Любой текст', note: 'Без проверки, до 200 символов' },
];

export type PayoutRow = { key: string; id: string; title: string; hint: string; check: DetailsCheck; min: string; enabled: boolean };

type PayoutItem = { id: string; title: string; hint: string; check: DetailsCheck; min: string | null; enabled: boolean };

const MIN_RE = /^\d{1,9}([.,]\d{1,2})?$/;
const ID_ALPHABET = 'abcdefghijklmnopqrstuvwxyz0123456789';

function text(value: unknown): string {
  if (typeof value === 'string') {
    return value;
  }
  return typeof value === 'number' ? String(value) : '';
}

function isCheck(value: unknown): value is DetailsCheck {
  return CHECKS.some((c) => c.key === value);
}

export function payoutRows(stored: unknown, fallback: string | null): PayoutRow[] {
  let list: unknown = stored;
  if (!Array.isArray(list)) {
    try {
      list = fallback ? JSON.parse(fallback) : [];
    } catch {
      list = [];
    }
  }
  return (Array.isArray(list) ? list : []).map((item, i) => {
    const m = (item && typeof item === 'object' ? item : {}) as Record<string, unknown>;
    return {
      key: `${i}-${text(m.id)}`,
      id: text(m.id),
      title: text(m.title),
      hint: text(m.hint),
      check: isCheck(m.check) ? m.check : 'text',
      min: text(m.min),
      enabled: m.enabled !== false,
    };
  });
}

export function payoutItems(rows: PayoutRow[]): PayoutItem[] {
  return rows.map((r) => ({
    id: r.id,
    title: r.title.trim(),
    hint: r.hint.trim(),
    check: r.check,
    min: r.min.trim() ? r.min.trim().replace(',', '.') : null,
    enabled: r.enabled,
  }));
}

export function payoutProblem(rows: PayoutRow[], values: FieldValues): { key: string | null; text: string } | null {
  for (const r of rows) {
    if (!r.title.trim()) {
      return { key: r.key, text: 'У каждого способа вывода должно быть название' };
    }
    if (r.min.trim() && !MIN_RE.test(r.min.trim())) {
      return { key: r.key, text: `Минимум у «${r.title.trim()}» — число, например 500` };
    }
  }
  if (values.withdrawals !== false && !rows.some((r) => r.enabled)) {
    return { key: null, text: 'Включите хотя бы один способ вывода или выключите приём заявок' };
  }
  return null;
}

function newId(rows: PayoutRow[]): string {
  const taken = new Set(rows.map((r) => r.id));
  for (;;) {
    const bytes = crypto.getRandomValues(new Uint8Array(6));
    const id = `m_${Array.from(bytes, (b) => ID_ALPHABET[b % ID_ALPHABET.length]).join('')}`;
    if (!taken.has(id)) {
      return id;
    }
  }
}

function amount(raw: string, currency: Currency): string | null {
  const clean = raw.trim();
  if (!clean || !MIN_RE.test(clean)) {
    return null;
  }
  return money({ minor: Math.round(Number(clean.replace(',', '.')) * 100), currency });
}

type Props = {
  field: SettingField;
  rows: PayoutRow[];
  onChange: (rows: PayoutRow[]) => void;
  values: FieldValues;
  currency: Currency;
  busy: boolean;
  invalid: string | null;
};

export function PayoutMethods({ field, rows, onChange, values, currency, busy, invalid }: Props) {
  const [open, setOpen] = useState<string[]>([]);

  useEffect(() => {
    if (invalid) {
      setOpen((list) => (list.includes(invalid) ? list : [...list, invalid]));
    }
  }, [invalid]);

  const general = amount(text(values.withdraw_min), currency);
  const unit = currency === 'RUB' ? '₽' : '$';

  const patch = (key: string, change: Partial<PayoutRow>) => onChange(rows.map((r) => (r.key === key ? { ...r, ...change } : r)));

  const raise = (index: number) => {
    const row = rows[index];
    const above = rows[index - 1];
    if (!row || !above) {
      return;
    }
    const next = [...rows];
    next[index - 1] = row;
    next[index] = above;
    onChange(next);
  };

  const remove = (key: string) => {
    onChange(rows.filter((r) => r.key !== key));
    setOpen((list) => list.filter((k) => k !== key));
  };

  const add = () => {
    const id = newId(rows);
    const row: PayoutRow = { key: `new-${id}`, id, title: '', hint: '', check: 'text', min: '', enabled: true };
    onChange([...rows, row]);
    setOpen((list) => [...list, row.key]);
  };

  const toggle = (key: string) => setOpen((list) => (list.includes(key) ? list.filter((k) => k !== key) : [...list, key]));

  const summary = (r: PayoutRow): string => {
    const check = CHECKS.find((c) => c.key === r.check)?.label ?? '';
    const own = amount(r.min, currency);
    const min = own ? `от ${own}` : general ? `общий минимум ${general}` : 'без минимума';
    return `${check} · ${min}`;
  };

  return (
    <div className="po" role="group" aria-labelledby="po-h">
      <div className="po-head">
        <span id="po-h" className="sf-label">
          {field.label}
        </span>
        {field.hint ? <span className="cap">{field.hint}.</span> : null}
      </div>
      {rows.length ? (
        <ul className="po-list">
          {rows.map((r, i) => {
            const isOpen = open.includes(r.key);
            const title = r.title.trim() || 'Новый способ';
            const check = CHECKS.find((c) => c.key === r.check);
            const bad = invalid === r.key;
            const base = `po-${r.key}`;
            return (
              <li key={r.key} className={`po-item${isOpen ? ' open' : ''}${r.enabled ? '' : ' po-off'}`}>
                <div className="po-row">
                  <button type="button" className="po-main" aria-expanded={isOpen} aria-controls={base} onClick={() => toggle(r.key)}>
                    <span className="po-text">
                      <span className="po-name">{title}</span>
                      <span className="po-sum">{summary(r)}</span>
                    </span>
                    <Icon name="chevron-down" size={18} />
                  </button>
                  <Switch on={r.enabled} onToggle={() => patch(r.key, { enabled: !r.enabled })} label={`Показывать клиентам: ${title}`} disabled={busy} />
                </div>
                {isOpen ? (
                  <div className="po-body" id={base}>
                    <div className="po-grid">
                      <div className="po-field">
                        <label htmlFor={`${base}-title`} className="po-label">
                          Название
                        </label>
                        <input
                          id={`${base}-title`}
                          className="field"
                          value={r.title}
                          maxLength={40}
                          placeholder="Например: PayPal"
                          aria-invalid={bad && !r.title.trim() ? true : undefined}
                          onChange={(e) => patch(r.key, { title: e.target.value })}
                          disabled={busy}
                          autoFocus={r.key.startsWith('new-') && !r.title}
                        />
                      </div>
                      <div className="po-field">
                        <label htmlFor={`${base}-check`} className="po-label">
                          Проверка реквизитов
                        </label>
                        <select
                          id={`${base}-check`}
                          className="field"
                          value={r.check}
                          aria-describedby={`${base}-check-note`}
                          onChange={(e) => patch(r.key, { check: e.target.value as DetailsCheck })}
                          disabled={busy}
                        >
                          {CHECKS.map((c) => (
                            <option key={c.key} value={c.key}>
                              {c.label}
                            </option>
                          ))}
                        </select>
                        <span id={`${base}-check-note`} className="cap">
                          {check?.note}
                        </span>
                      </div>
                      <div className="po-field po-wide">
                        <label htmlFor={`${base}-hint`} className="po-label">
                          Подсказка клиенту
                        </label>
                        <input
                          id={`${base}-hint`}
                          className="field"
                          value={r.hint}
                          maxLength={120}
                          placeholder="Что написать в реквизитах, например: почта PayPal"
                          aria-describedby={`${base}-hint-note`}
                          onChange={(e) => patch(r.key, { hint: e.target.value })}
                          disabled={busy}
                        />
                        <span id={`${base}-hint-note`} className="cap">
                          Клиент видит её в поле для реквизитов
                        </span>
                      </div>
                      <div className="po-field">
                        <label htmlFor={`${base}-min`} className="po-label">
                          Минимум
                        </label>
                        <span className="po-unit">
                          {currency === 'USD' ? <span className="muted">{unit}</span> : null}
                          <input
                            id={`${base}-min`}
                            className="field num"
                            inputMode={currency === 'USD' ? 'decimal' : 'numeric'}
                            value={r.min}
                            maxLength={12}
                            aria-describedby={`${base}-min-note`}
                            aria-invalid={bad && r.min.trim() !== '' ? true : undefined}
                            onChange={(e) => patch(r.key, { min: e.target.value })}
                            disabled={busy}
                          />
                          {currency === 'RUB' ? <span className="muted">{unit}</span> : null}
                        </span>
                        <span id={`${base}-min-note`} className="cap">
                          {general ? `Пусто — общий минимум, ${general}` : 'Пусто — без минимума'}
                        </span>
                      </div>
                    </div>
                    <div className="po-foot">
                      <button type="button" className="b36 hov" onClick={() => raise(i)} disabled={busy || i === 0}>
                        <Icon name="arrow-up" size={16} />
                        Выше
                      </button>
                      <button type="button" className="po-remove" onClick={() => remove(r.key)} disabled={busy}>
                        Удалить способ
                      </button>
                    </div>
                  </div>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="muted po-empty">Способов нет: клиенты не смогут отправить заявку на вывод.</p>
      )}
      {rows.length < MAX_METHODS ? (
        <button type="button" className="b40 hov po-add" onClick={add} disabled={busy}>
          Добавить способ
        </button>
      ) : (
        <span className="cap">Больше {MAX_METHODS} способов добавить нельзя.</span>
      )}
    </div>
  );
}
