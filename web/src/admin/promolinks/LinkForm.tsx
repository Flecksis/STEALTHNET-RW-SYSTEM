import { useId, useState, type ReactNode } from 'react';
import { api } from '../../lib/api';
import { RESETS } from '../tariffs/model';
import { buildBody, EMPTY_DRAFT, failText, inventCode, ROOT, type Draft, type DraftErrors, type PromoLink, type SquadBrief } from './model';

type Props = {
  phone?: boolean;
  squads: SquadBrief[] | null;
  onCancel: () => void;
  onCreated: (link: PromoLink) => void;
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

export function LinkForm({ phone = false, squads, onCancel, onCreated }: Props) {
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
    const built = buildBody(draft);
    const found = Object.values(built.errors).some(Boolean);
    setErrors(built.errors);
    setFailure(found ? 'Исправьте отмеченные поля' : null);
    if (found) {
      return;
    }
    setBusy(true);
    try {
      const created = await api<PromoLink>(`${ROOT}/links`, { method: 'POST', body: built.body });
      setDraft(EMPTY_DRAFT);
      onCreated(created);
    } catch (err) {
      setFailure(failText(err, 'Ссылка не создалась'));
    } finally {
      setBusy(false);
    }
  };

  const ids = {
    name: `${base}-name`,
    code: `${base}-code`,
    days: `${base}-days`,
    traffic: `${base}-traffic`,
    reset: `${base}-reset`,
    devices: `${base}-devices`,
    max: `${base}-max`,
    until: `${base}-until`,
  };
  const limited = draft.traffic.trim() !== '' && draft.traffic.trim() !== '0';
  const toggleSquad = (uuid: string) => set('squads', draft.squads.includes(uuid) ? draft.squads.filter((s) => s !== uuid) : [...draft.squads, uuid]);

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
        <Field id={ids.name} label="Название" error={errors.name} hint="Видите только вы, например «Блогер Иван, ноябрь»">
          <input
            id={ids.name}
            className="field"
            value={draft.name}
            onChange={(e) => set('name', e.target.value)}
            maxLength={100}
            aria-invalid={errors.name ? true : undefined}
            aria-describedby={described(ids.name, errors.name)}
            disabled={busy}
          />
        </Field>
        <Field id={ids.code} label="Код в ссылке" error={errors.code} hint="Пусто — придумаем сами. Будет в конце ссылки после pl_">
          <span className="pf-inline">
            <input
              id={ids.code}
              className="field mono"
              value={draft.code}
              onChange={(e) => set('code', e.target.value)}
              maxLength={32}
              autoComplete="off"
              spellCheck={false}
              placeholder="Например, ivan-nov"
              aria-invalid={errors.code ? true : undefined}
              aria-describedby={described(ids.code, errors.code)}
              disabled={busy}
            />
            <button type="button" className={phone ? 'pf-quiet press' : 'b40 hov'} onClick={() => set('code', inventCode())} disabled={busy}>
              Придумать
            </button>
          </span>
        </Field>
      </div>

      <fieldset className="pf-tariffs">
        <legend className="pf-label">Сквады</legend>
        {squads === null ? <span className="cap">Загружаем сквады…</span> : null}
        {squads !== null && squads.length === 0 ? <span className="cap">Сквады не загрузились: проверьте подключение к Remnawave.</span> : null}
        <div className="pf-chips">
          {(squads ?? []).map((s) => (
            <label key={s.uuid} className="pf-chip">
              <input type="checkbox" checked={draft.squads.includes(s.uuid)} onChange={() => toggleSquad(s.uuid)} disabled={busy} />
              <span>{s.name}</span>
            </label>
          ))}
        </div>
        {errors.squads ? <span className="err">{errors.squads}</span> : <span className="cap">Серверы, которые получит клиент по ссылке</span>}
      </fieldset>

      <div className="pf-grid pf-three">
        <Field id={ids.days} label="Срок подписки, дней" error={errors.days}>
          <input
            id={ids.days}
            className="field num"
            inputMode="numeric"
            value={draft.days}
            onChange={(e) => set('days', e.target.value.replace(/\D/g, ''))}
            maxLength={4}
            aria-invalid={errors.days ? true : undefined}
            disabled={busy}
          />
        </Field>
        <Field id={ids.traffic} label="Трафик, ГБ" error={errors.traffic} hint="Пусто — без лимита">
          <input
            id={ids.traffic}
            className="field num"
            inputMode="numeric"
            value={draft.traffic}
            onChange={(e) => set('traffic', e.target.value.replace(/\D/g, ''))}
            maxLength={7}
            aria-invalid={errors.traffic ? true : undefined}
            aria-describedby={described(ids.traffic, errors.traffic)}
            disabled={busy}
          />
        </Field>
        <Field id={ids.devices} label="Устройств" error={errors.devices} hint="0 — не ограничивать">
          <input
            id={ids.devices}
            className="field num"
            inputMode="numeric"
            value={draft.devices}
            onChange={(e) => set('devices', e.target.value.replace(/\D/g, ''))}
            maxLength={3}
            aria-invalid={errors.devices ? true : undefined}
            aria-describedby={described(ids.devices, errors.devices)}
            disabled={busy}
          />
        </Field>
      </div>

      {limited ? (
        <Field id={ids.reset} label="Сброс трафика">
          <select id={ids.reset} className="field" value={draft.reset} onChange={(e) => set('reset', e.target.value as Draft['reset'])} disabled={busy}>
            {RESETS.filter((r) => r.value !== 'on_payment').map((r) => (
              <option key={r.value} value={r.value}>
                {r.label}
              </option>
            ))}
          </select>
        </Field>
      ) : null}

      <div className="pf-grid">
        <Field id={ids.max} label="Сколько раз можно активировать" error={errors.max} hint="Всего по ссылке. Пусто — без лимита. Один клиент — один раз">
          <input
            id={ids.max}
            className="field num"
            inputMode="numeric"
            value={draft.max}
            onChange={(e) => set('max', e.target.value.replace(/\D/g, ''))}
            maxLength={8}
            aria-invalid={errors.max ? true : undefined}
            aria-describedby={described(ids.max, errors.max)}
            disabled={busy}
          />
        </Field>
        <Field id={ids.until} label="Ссылка работает до" error={errors.until} hint="Включительно. Пусто — без срока">
          <input
            id={ids.until}
            type="date"
            className="field"
            value={draft.until}
            onChange={(e) => set('until', e.target.value)}
            aria-invalid={errors.until ? true : undefined}
            aria-describedby={described(ids.until, errors.until)}
            disabled={busy}
          />
        </Field>
      </div>

      {failure ? (
        <p className="err" role="alert">
          {failure}
        </p>
      ) : null}
      <div className="pf-actions">
        <button type="submit" className="btn-main btn" disabled={busy}>
          {busy ? 'Создаём…' : 'Создать ссылку'}
        </button>
        <button type="button" className={phone ? 'pf-quiet press' : 'b40 hov'} onClick={onCancel} disabled={busy}>
          Отменить
        </button>
      </div>
    </form>
  );
}
