import { useId, useState, type ReactNode } from 'react';
import { api } from '../../lib/api';
import type { Currency } from '../../lib/types';
import { buildBody, EMPTY_DRAFT, failText, inventCode, KINDS, type Draft, type DraftErrors, type PromoCode, type TariffBrief } from './model';

type Props = {
  phone?: boolean;
  currency: Currency;
  tariffs: TariffBrief[] | null;
  onCancel: () => void;
  onCreated: (code: PromoCode) => void;
};

type FieldProps = {
  id: string;
  label: string;
  hint?: string;
  error?: string;
  className?: string;
  children: ReactNode;
};

function Field({ id, label, hint, error, className, children }: FieldProps) {
  return (
    <div className={className ? `pf-field ${className}` : 'pf-field'}>
      <label htmlFor={id} className="pf-label">
        {label}
      </label>
      {children}
      {error ? (
        <span id={`${id}-err`} className="err">
          {error}
        </span>
      ) : hint ? (
        <span id={`${id}-hint`} className="cap">
          {hint}
        </span>
      ) : null}
    </div>
  );
}

function described(id: string, error?: string): string {
  return error ? `${id}-err` : `${id}-hint`;
}

export function PromoForm({ phone = false, currency, tariffs, onCancel, onCreated }: Props) {
  const base = useId();
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  const [errors, setErrors] = useState<DraftErrors>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => {
    setDraft((d) => ({ ...d, [key]: value }));
    if (key in errors) {
      setErrors((e) => ({ ...e, [key]: undefined }));
    }
  };

  const submit = async () => {
    const built = buildBody(draft, currency);
    const found = Object.values(built.errors).some(Boolean);
    setErrors(built.errors);
    setFailure(found ? 'Исправьте отмеченные поля' : null);
    if (found) {
      return;
    }
    setBusy(true);
    try {
      const created = await api<PromoCode>('/api/admin/m/promo/codes', { method: 'POST', body: built.body });
      setDraft(EMPTY_DRAFT);
      onCreated({ ...created, used: 0, reserved: 0 });
    } catch (err) {
      setFailure(failText(err, 'Промокод не создался'));
    } finally {
      setBusy(false);
    }
  };

  const toggleTariff = (id: number) => set('tariffs', draft.tariffs.includes(id) ? draft.tariffs.filter((t) => t !== id) : [...draft.tariffs, id]);

  const ids = {
    code: `${base}-code`,
    value: `${base}-value`,
    maxUses: `${base}-max`,
    perClient: `${base}-per`,
    from: `${base}-from`,
    until: `${base}-until`,
    note: `${base}-note`,
  };

  const valueLabel = draft.kind === 'percent' ? 'Размер скидки' : draft.kind === 'fixed' ? 'Сумма скидки' : 'Сколько дней добавить';
  const unitBefore = draft.kind === 'fixed' && currency === 'USD' ? '$' : null;
  const unitAfter = draft.kind === 'percent' ? '%' : draft.kind === 'fixed' ? (currency === 'RUB' ? '₽' : null) : 'дней';
  const valueHint = draft.kind === 'days' ? 'Клиент вводит код в боте, кабинете или мини-приложении, дни сразу добавляются к его подписке' : 'Скидка применяется к оплате тарифа';
  const showTariffs = draft.kind !== 'days' && tariffs !== null && tariffs.length > 0;

  return (
    <form
      className={phone ? 'pf pf-phone' : 'pf'}
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div className="pf-grid">
        <Field id={ids.code} label="Код" error={errors.code} hint="Латинские буквы и цифры, регистр не важен" className="pf-code">
          <span className="pf-inline">
            <input
              id={ids.code}
              className="field mono pf-upper"
              value={draft.code}
              onChange={(e) => set('code', e.target.value.toUpperCase())}
              maxLength={32}
              autoComplete="off"
              spellCheck={false}
              placeholder="Например, SPRING25"
              aria-invalid={errors.code ? true : undefined}
              aria-describedby={described(ids.code, errors.code)}
              disabled={busy}
            />
            <button type="button" className={phone ? 'pf-quiet press' : 'b40 hov'} onClick={() => set('code', inventCode())} disabled={busy}>
              Придумать
            </button>
          </span>
        </Field>
        <div className="pf-field pf-kind">
          <span className="pf-label" id={`${base}-kind`}>
            Что даёт
          </span>
          <div role="group" aria-labelledby={`${base}-kind`} className={phone ? 'pf-psegs' : 'segs'}>
            {KINDS.map(([key, label]) => (
              <button
                key={key}
                type="button"
                aria-pressed={draft.kind === key}
                onClick={() => {
                  set('kind', key);
                  set('value', '');
                }}
                disabled={busy}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="pf-grid pf-three">
        <Field id={ids.value} label={valueLabel} error={errors.value} hint={valueHint}>
          <span className="pf-unit">
            {unitBefore ? <span className="muted">{unitBefore}</span> : null}
            <input
              id={ids.value}
              className="field num"
              inputMode={draft.kind === 'fixed' && currency === 'USD' ? 'decimal' : 'numeric'}
              value={draft.value}
              onChange={(e) => set('value', e.target.value)}
              maxLength={12}
              autoComplete="off"
              aria-invalid={errors.value ? true : undefined}
              aria-describedby={described(ids.value, errors.value)}
              disabled={busy}
            />
            {unitAfter ? <span className="muted">{unitAfter}</span> : null}
          </span>
        </Field>
        <Field id={ids.maxUses} label="Всего использований" error={errors.maxUses} hint="Пусто — без ограничения">
          <input
            id={ids.maxUses}
            className="field num"
            inputMode="numeric"
            value={draft.maxUses}
            onChange={(e) => set('maxUses', e.target.value)}
            placeholder="Без ограничения"
            maxLength={10}
            autoComplete="off"
            aria-invalid={errors.maxUses ? true : undefined}
            aria-describedby={described(ids.maxUses, errors.maxUses)}
            disabled={busy}
          />
        </Field>
        <Field id={ids.perClient} label="На одного клиента" error={errors.perClient} hint="Сколько раз один клиент может применить код">
          <input
            id={ids.perClient}
            className="field num"
            inputMode="numeric"
            value={draft.perClient}
            onChange={(e) => set('perClient', e.target.value)}
            maxLength={4}
            autoComplete="off"
            aria-invalid={errors.perClient ? true : undefined}
            aria-describedby={described(ids.perClient, errors.perClient)}
            disabled={busy}
          />
        </Field>
      </div>

      <div className="pf-grid pf-three">
        <Field id={ids.from} label="Действует с" error={errors.from} hint="Пусто — сразу после создания">
          <input
            id={ids.from}
            type="date"
            className="field"
            value={draft.from}
            onChange={(e) => set('from', e.target.value)}
            aria-invalid={errors.from ? true : undefined}
            aria-describedby={described(ids.from, errors.from)}
            disabled={busy}
          />
        </Field>
        <Field id={ids.until} label="Действует по" error={errors.until} hint="Включительно. Пусто — без срока">
          <input
            id={ids.until}
            type="date"
            className="field"
            value={draft.until}
            min={draft.from || undefined}
            onChange={(e) => set('until', e.target.value)}
            aria-invalid={errors.until ? true : undefined}
            aria-describedby={described(ids.until, errors.until)}
            disabled={busy}
          />
        </Field>
        <Field id={ids.note} label="Заметка" error={errors.note} hint="Видна только в админке">
          <input
            id={ids.note}
            className="field"
            value={draft.note}
            onChange={(e) => set('note', e.target.value)}
            maxLength={500}
            placeholder="Для кого и зачем"
            aria-invalid={errors.note ? true : undefined}
            aria-describedby={described(ids.note, errors.note)}
            disabled={busy}
          />
        </Field>
      </div>

      <label className="pf-check">
        <input type="checkbox" checked={draft.firstOnly} onChange={(e) => set('firstOnly', e.target.checked)} disabled={busy} />
        <span>
          Только для первой покупки
          <span className="cap">Не примем у клиентов, которые уже платили за подписку</span>
        </span>
      </label>

      {showTariffs ? (
        <fieldset className="pf-tariffs" disabled={busy}>
          <legend className="pf-label">Тарифы</legend>
          <span className="cap">Ничего не отмечено — код подходит к любому тарифу</span>
          <span className="pf-chips">
            {tariffs.map((t) => (
              <label key={t.id} className="pf-chip">
                <input type="checkbox" checked={draft.tariffs.includes(t.id)} onChange={() => toggleTariff(t.id)} />
                <span>{t.name}</span>
              </label>
            ))}
          </span>
        </fieldset>
      ) : null}

      <div className="pf-actions">
        <button type="submit" className={phone ? 'np-ok press' : 'btn-main btn'} disabled={busy}>
          {busy ? 'Создаём…' : 'Создать промокод'}
        </button>
        <button type="button" className={phone ? 'np-cancel press' : 'btn-line hov'} onClick={onCancel} disabled={busy}>
          Отменить
        </button>
        {failure ? (
          <span className="err pf-fail" role="alert">
            {failure}
          </span>
        ) : null}
      </div>
    </form>
  );
}
