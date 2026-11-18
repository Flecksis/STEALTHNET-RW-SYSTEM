import { useId, useState } from 'react';
import { dateLong, days } from '../../lib/format';
import { Dialog } from './dialogs';

const PRESETS = [7, 30, 90, 180, 365];
const DAY_MS = 86_400_000;

type Props = {
  until: string | null;
  onExtend: (days: number) => Promise<void>;
  onClose: () => void;
};

export function ExtendDialog({ until, onExtend, onClose }: Props) {
  const [choice, setChoice] = useState<number | 'custom'>(30);
  const [custom, setCustom] = useState('');
  const [busy, setBusy] = useState(false);
  const daysField = useId();
  const customDays = Number(custom);
  const customValid = Number.isInteger(customDays) && customDays >= 1 && customDays <= 3650;
  const chosen = choice === 'custom' ? (customValid ? customDays : 0) : choice;
  const end = until ? Date.parse(until) : NaN;
  const from = Number.isFinite(end) && end > Date.now() ? end : Date.now();
  const next = chosen > 0 ? new Date(from + chosen * DAY_MS).toISOString() : null;

  const submit = async () => {
    if (chosen <= 0) {
      return;
    }
    setBusy(true);
    await onExtend(chosen);
    onClose();
  };

  return (
    <Dialog title="Продлить подписку" busy={busy} onClose={onClose}>
      <div className="field-row">
        <span style={{ fontWeight: 600 }}>На сколько</span>
        <div role="group" aria-label="На сколько продлить" className="segs" style={{ alignSelf: 'flex-start' }}>
          {PRESETS.map((n) => (
            <button key={n} type="button" aria-pressed={choice === n} onClick={() => setChoice(n)} disabled={busy}>
              {days(n)}
            </button>
          ))}
          <button type="button" aria-pressed={choice === 'custom'} onClick={() => setChoice('custom')} disabled={busy}>
            Свой срок
          </button>
        </div>
        {choice === 'custom' ? (
          <span className="unit-field">
            <label htmlFor={daysField} className="sr-only">
              Срок в днях
            </label>
            <input
              id={daysField}
              className="field num"
              inputMode="numeric"
              value={custom}
              onChange={(event) => setCustom(event.target.value)}
              disabled={busy}
              autoFocus
            />
            <span className="muted">дней, до 3650</span>
          </span>
        ) : null}
      </div>
      <p className="cap">{next ? `Подписка будет действовать до ${dateLong(next)}.` : 'Введите число дней от 1 до 3650.'}</p>
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void submit()} disabled={busy || chosen <= 0}>
          {busy ? 'Продлеваем…' : chosen > 0 ? `Продлить на ${days(chosen)}` : 'Продлить'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}
