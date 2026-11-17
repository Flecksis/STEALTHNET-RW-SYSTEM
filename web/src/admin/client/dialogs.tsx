import { useEffect, useId, useMemo, useState, type ReactNode } from 'react';
import { encode } from 'uqr';
import { api, ApiError } from '../../lib/api';
import { days, groupDigits, money, NBSP, periodLabel, type Money } from '../../lib/format';
import type { Currency } from '../../lib/types';
import type { TariffBrief } from './types';
import '../form.css';

type DialogProps = { title: string; busy?: boolean; onClose: () => void; children: ReactNode; wide?: boolean };

export function Dialog({ title, busy = false, onClose, children, wide = false }: DialogProps) {
  const id = useId();
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !busy) {
        onClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onClose]);
  return (
    <div className="dialog-back" role="presentation" onClick={() => (busy ? undefined : onClose())}>
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={id}
        style={wide ? { maxWidth: 520 } : undefined}
        onClick={(event) => event.stopPropagation()}
      >
        <h2 id={id}>{title}</h2>
        {children}
      </div>
    </div>
  );
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

type MessageProps = {
  clientId: number;
  blockedReason: string | null;
  onClose: () => void;
  onSent: () => void;
};

export function MessageDialog({ clientId, blockedReason, onClose, onSent }: MessageProps) {
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fieldId = useId();
  const send = async () => {
    setBusy(true);
    setError(null);
    try {
      await api(`/api/admin/clients/${clientId}/message`, { method: 'POST', body: { text } });
      onSent();
    } catch (err) {
      setError(failText(err, 'Не отправилось'));
      setBusy(false);
    }
  };
  return (
    <Dialog title="Написать клиенту" busy={busy} onClose={onClose}>
      {blockedReason ? (
        <p className="muted">{blockedReason}</p>
      ) : (
        <div className="field-row">
          <label htmlFor={fieldId}>Сообщение придёт от бота</label>
          <textarea
            id={fieldId}
            className="field"
            rows={5}
            maxLength={3500}
            value={text}
            onChange={(event) => setText(event.target.value)}
            disabled={busy}
            autoFocus
          />
          <span className="cap">{groupDigits(text.trim().length)} из 3 500 символов</span>
        </div>
      )}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        {blockedReason ? null : (
          <button type="button" className="btn-main btn" onClick={() => void send()} disabled={busy || !text.trim()}>
            {busy ? 'Отправляем…' : 'Отправить'}
          </button>
        )}
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          {blockedReason ? 'Понятно' : 'Отменить'}
        </button>
      </div>
    </Dialog>
  );
}

type BalanceProps = {
  clientId: number;
  currency: Currency;
  balance: Money;
  initial: 'add' | 'take';
  onClose: () => void;
  onDone: (balance: Money) => void;
};

function parseMinor(raw: string): number | null {
  const clean = raw.replace(/\s/g, '').replace(',', '.');
  if (!/^\d+(\.\d{0,2})?$/.test(clean)) {
    return null;
  }
  const value = Math.round(Number(clean) * 100);
  return value > 0 && Number.isSafeInteger(value) ? value : null;
}

export function BalanceDialog({ clientId, currency, balance, initial, onClose, onDone }: BalanceProps) {
  const [mode, setMode] = useState(initial);
  const [amount, setAmount] = useState('');
  const [comment, setComment] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const key = useMemo(() => (typeof crypto.randomUUID === 'function' ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`), []);
  const amountId = useId();
  const commentId = useId();
  const minor = parseMinor(amount);
  const save = async () => {
    if (minor === null) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await api<{ balance: Money }>(`/api/admin/clients/${clientId}/balance`, {
        method: 'POST',
        body: { delta: { minor: mode === 'add' ? minor : -minor, currency }, comment: comment.trim(), idempotency_key: key },
      });
      onDone(result.balance);
    } catch (err) {
      setError(failText(err, 'Баланс не изменился'));
      setBusy(false);
    }
  };
  return (
    <Dialog title="Изменить баланс" busy={busy} onClose={onClose}>
      <p className="muted">Сейчас на балансе {money(balance)}</p>
      <div role="group" aria-label="Что сделать" className="segs" style={{ alignSelf: 'flex-start' }}>
        <button type="button" aria-pressed={mode === 'add'} onClick={() => setMode('add')} disabled={busy}>
          Пополнить
        </button>
        <button type="button" aria-pressed={mode === 'take'} onClick={() => setMode('take')} disabled={busy}>
          Списать
        </button>
      </div>
      <div className="field-row">
        <label htmlFor={amountId}>Сумма, {currency === 'RUB' ? '₽' : '$'}</label>
        <input
          id={amountId}
          className="field num"
          inputMode="decimal"
          value={amount}
          onChange={(event) => setAmount(event.target.value)}
          placeholder={currency === 'RUB' ? '500' : '5.00'}
          disabled={busy}
          autoFocus
        />
      </div>
      <div className="field-row">
        <label htmlFor={commentId}>Причина</label>
        <input
          id={commentId}
          className="field"
          value={comment}
          maxLength={300}
          onChange={(event) => setComment(event.target.value)}
          placeholder="Например: компенсация за перебои 12 ноября"
          disabled={busy}
        />
        <span className="cap">Причину увидят администраторы в истории клиента</span>
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || minor === null || !comment.trim()}>
          {busy ? 'Сохраняем…' : mode === 'add' ? 'Пополнить' : 'Списать'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

type LimitsProps = {
  subscriptionId: number;
  tariff: TariffBrief | null;
  trafficLimit: number | null;
  extraDevices: number;
  onClose: () => void;
  onDone: (view: unknown) => void;
};

const GIB = 1024 ** 3;

export function LimitsDialog({ subscriptionId, tariff, trafficLimit, extraDevices, onClose, onDone }: LimitsProps) {
  const [unlimited, setUnlimited] = useState(!trafficLimit);
  const [gb, setGb] = useState(trafficLimit ? String(Math.round(trafficLimit / GIB)) : '');
  const [extra, setExtra] = useState(String(extraDevices));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const gbId = useId();
  const extraId = useId();
  const maxExtra = tariff?.devices.extra?.max ?? 0;
  const gbValue = Number(gb);
  const gbValid = unlimited || (Number.isInteger(gbValue) && gbValue > 0 && gbValue <= 1_000_000);
  const extraValue = Number(extra);
  const extraValid = maxExtra === 0 || (Number.isInteger(extraValue) && extraValue >= 0 && extraValue <= maxExtra);
  const save = async () => {
    const limits: Record<string, number | null> = {};
    const nextLimit = unlimited ? null : gbValue * GIB;
    if ((nextLimit ?? 0) !== (trafficLimit ?? 0)) {
      limits.traffic_limit_bytes = nextLimit;
    }
    if (maxExtra > 0 && extraValue !== extraDevices) {
      limits.extra_devices = extraValue;
    }
    if (Object.keys(limits).length === 0) {
      onClose();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const view = await api<unknown>(`/api/admin/subscriptions/${subscriptionId}/actions/limits`, { method: 'POST', body: { limits } });
      onDone(view);
    } catch (err) {
      setError(failText(err, 'Лимиты не изменились'));
      setBusy(false);
    }
  };
  return (
    <Dialog title="Изменить лимиты" busy={busy} onClose={onClose}>
      <div className="field-row">
        <span style={{ fontWeight: 600 }}>Трафик</span>
        <div role="group" aria-label="Трафик" className="segs" style={{ alignSelf: 'flex-start' }}>
          <button type="button" aria-pressed={unlimited} onClick={() => setUnlimited(true)} disabled={busy}>
            Без лимита
          </button>
          <button type="button" aria-pressed={!unlimited} onClick={() => setUnlimited(false)} disabled={busy}>
            С лимитом
          </button>
        </div>
        {unlimited ? null : (
          <>
            <label htmlFor={gbId} className="sr-only">
              Лимит трафика, ГБ
            </label>
            <span className="unit-field">
              <input id={gbId} className="field num" inputMode="numeric" value={gb} onChange={(event) => setGb(event.target.value)} disabled={busy} />
              <span className="muted">ГБ</span>
            </span>
          </>
        )}
      </div>
      {maxExtra > 0 ? (
        <div className="field-row">
          <label htmlFor={extraId}>Дополнительные устройства</label>
          <span className="unit-field">
            <input
              id={extraId}
              className="field num"
              inputMode="numeric"
              value={extra}
              onChange={(event) => setExtra(event.target.value)}
              disabled={busy}
            />
            <span className="muted">из{NBSP}{maxExtra}</span>
          </span>
          <span className="cap">
            В тариф входит {tariff?.devices.included ?? 0}, всего станет {(tariff?.devices.included ?? 0) + (extraValid ? extraValue : 0)}
          </span>
        </div>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || !gbValid || !extraValid}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

export function QrDialog({ url, onClose }: { url: string; onClose: () => void }) {
  const code = useMemo(() => encode(url, { border: 2 }), [url]);
  const path = useMemo(() => {
    const parts: string[] = [];
    code.data.forEach((row, y) => {
      row.forEach((dark, x) => {
        if (dark) {
          parts.push(`M${x} ${y}h1v1h-1z`);
        }
      });
    });
    return parts.join('');
  }, [code]);
  return (
    <Dialog title="QR-код подписки" onClose={onClose}>
      <svg viewBox={`0 0 ${code.size} ${code.size}`} width="240" height="240" role="img" aria-label="QR-код ссылки подписки" className="qr">
        <rect width={code.size} height={code.size} fill="#ffffff" />
        <path d={path} fill="#000000" shapeRendering="crispEdges" />
      </svg>
      <p className="mono" style={{ overflowWrap: 'anywhere' }}>
        {url}
      </p>
      <div className="dialog-actions">
        <button type="button" className="btn-line hov" onClick={onClose}>
          Закрыть
        </button>
      </div>
    </Dialog>
  );
}

type BlockProps = { clientId: number; onClose: () => void; onDone: () => void };

export function BlockDialog({ clientId, onClose, onDone }: BlockProps) {
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fieldId = useId();
  const block = async () => {
    setBusy(true);
    setError(null);
    try {
      await api(`/api/admin/clients/${clientId}/block`, { method: 'POST', body: reason.trim() ? { reason: reason.trim() } : {} });
      onDone();
    } catch (err) {
      setError(failText(err, 'Не получилось заблокировать'));
      setBusy(false);
    }
  };
  return (
    <Dialog title="Заблокировать клиента" busy={busy} onClose={onClose}>
      <p className="muted">Подписки клиента отключатся, вход в кабинет закроется, неоплаченные счета отменятся.</p>
      <div className="field-row">
        <label htmlFor={fieldId}>Причина</label>
        <input
          id={fieldId}
          className="field"
          maxLength={500}
          value={reason}
          onChange={(event) => setReason(event.target.value)}
          placeholder="Например: злоупотребление трафиком"
          disabled={busy}
          autoFocus
        />
        <span className="cap">Видят только администраторы</span>
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-bad" onClick={() => void block()} disabled={busy}>
          {busy ? 'Блокируем…' : 'Заблокировать'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

type GrantPeriodOption = { id: number; days: number; active: boolean };
type GrantTariffOption = {
  id: number;
  name: string;
  category_id: number;
  on_sale: boolean;
  periods: GrantPeriodOption[];
  ready: boolean;
};

type GrantProps = {
  clientId: number;
  subscriptionId: number | null;
  currentTariffId: number | null;
  currentTrial: boolean;
  onClose: () => void;
  onDone: (result: { plan: 'new' | 'renewal' | 'change'; days: number }) => void;
};

export function GrantDialog({ clientId, subscriptionId, currentTariffId, currentTrial, onClose, onDone }: GrantProps) {
  const [tariffs, setTariffs] = useState<GrantTariffOption[] | null>(null);
  const [tariffId, setTariffId] = useState<number | null>(null);
  const [periodId, setPeriodId] = useState<number | 'custom' | null>(null);
  const [custom, setCustom] = useState('30');
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const tariffField = useId();
  const daysField = useId();
  const noteField = useId();

  useEffect(() => {
    let cancelled = false;
    api<{ tariffs: GrantTariffOption[] }>('/api/admin/clients/grant-options')
      .then((data) => {
        if (cancelled) {
          return;
        }
        const usable = data.tariffs.filter((t) => t.ready && t.periods.length > 0);
        setTariffs(usable);
        const first = usable.find((t) => t.id !== currentTariffId) ?? usable[0];
        if (first) {
          setTariffId(subscriptionId && currentTariffId && usable.some((t) => t.id === currentTariffId) ? currentTariffId : first.id);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(failText(err, 'Тарифы не загрузились'));
          setTariffs([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [currentTariffId, subscriptionId]);

  const tariff = tariffs?.find((t) => t.id === tariffId) ?? null;
  const periods = (tariff?.periods ?? []).filter((p) => p.active);
  const shown = periods.length > 0 ? periods : (tariff?.periods ?? []);

  useEffect(() => {
    setPeriodId(shown[0]?.id ?? 'custom');
  }, [tariffId]);

  const currentCategory = tariffs?.find((t) => t.id === currentTariffId)?.category_id ?? null;
  const plan: 'new' | 'renewal' | 'change' = !subscriptionId
    ? 'new'
    : currentTrial || !tariff || currentCategory !== tariff.category_id
      ? 'change'
      : 'renewal';
  const customDays = Number(custom);
  const customValid = Number.isInteger(customDays) && customDays >= 1 && customDays <= 3650;
  const chosenDays = periodId === 'custom' ? (customValid ? customDays : 0) : (shown.find((p) => p.id === periodId)?.days ?? 0);

  const submit = async () => {
    if (!tariff || chosenDays <= 0) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const body: Record<string, unknown> = { tariff_id: tariff.id };
      if (periodId === 'custom') {
        body.days = customDays;
      } else {
        body.period_id = periodId;
      }
      if (subscriptionId) {
        body.subscription_id = subscriptionId;
      }
      if (note.trim()) {
        body.note = note.trim();
      }
      const result = await api<{ plan: 'new' | 'renewal' | 'change'; days: number }>(`/api/admin/clients/${clientId}/grant`, {
        method: 'POST',
        body,
      });
      onDone(result);
    } catch (err) {
      setError(failText(err, 'Подписка не выдалась'));
      setBusy(false);
    }
  };

  const explain =
    plan === 'new'
      ? 'Клиент получит новую подписку без оплаты. Срок начнётся сегодня.'
      : plan === 'renewal'
        ? 'Тариф той же категории: дни добавятся к текущему сроку, устройства и ссылка сохранятся.'
        : 'Срок начнётся заново с сегодняшнего дня, трафик обнулится, сквады сменятся на сквады нового тарифа. Ссылка сохранится.';

  return (
    <Dialog title={subscriptionId ? 'Сменить тариф' : 'Выдать подписку'} busy={busy} onClose={onClose} wide>
      {tariffs === null ? <p className="muted">Загружаем тарифы…</p> : null}
      {tariffs && tariffs.length === 0 && !error ? <p className="muted">Нет тарифов со сквадами и сроками. Создайте тариф в разделе «Тарифы».</p> : null}
      {tariffs && tariffs.length > 0 ? (
        <>
          <div className="field-row">
            <label htmlFor={tariffField}>Тариф</label>
            <select
              id={tariffField}
              className="field"
              value={tariffId ?? ''}
              onChange={(event) => setTariffId(Number(event.target.value))}
              disabled={busy}
            >
              {tariffs.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                  {t.on_sale ? '' : ' (не продаётся)'}
                  {t.id === currentTariffId ? ' — текущий' : ''}
                </option>
              ))}
            </select>
          </div>
          <div className="field-row">
            <span style={{ fontWeight: 600 }}>Срок</span>
            <div role="group" aria-label="Срок" className="segs" style={{ alignSelf: 'flex-start' }}>
              {shown.map((p) => (
                <button key={p.id} type="button" aria-pressed={periodId === p.id} onClick={() => setPeriodId(p.id)} disabled={busy}>
                  {periodLabel(p.days)}
                </button>
              ))}
              <button type="button" aria-pressed={periodId === 'custom'} onClick={() => setPeriodId('custom')} disabled={busy}>
                Свой срок
              </button>
            </div>
            {periodId === 'custom' ? (
              <span className="unit-field">
                <label htmlFor={daysField} className="sr-only">
                  Срок в днях
                </label>
                <input id={daysField} className="field num" inputMode="numeric" value={custom} onChange={(event) => setCustom(event.target.value)} disabled={busy} />
                <span className="muted">дней</span>
              </span>
            ) : null}
          </div>
          <p className="cap">{explain}</p>
          <div className="field-row">
            <label htmlFor={noteField}>Комментарий</label>
            <input
              id={noteField}
              className="field"
              maxLength={500}
              value={note}
              onChange={(event) => setNote(event.target.value)}
              placeholder="Например: компенсация за перебои"
              disabled={busy}
            />
            <span className="cap">Увидят администраторы в истории клиента</span>
          </div>
        </>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        {tariffs && tariffs.length > 0 ? (
          <button type="button" className="btn-main btn" onClick={() => void submit()} disabled={busy || !tariff || chosenDays <= 0}>
            {busy ? 'Выдаём…' : subscriptionId ? 'Сменить тариф' : `Выдать на ${days(chosenDays || 0)}`}
          </button>
        ) : null}
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

type ConfirmProps = {
  title: string;
  text: string;
  confirm: string;
  run: () => Promise<void>;
  onClose: () => void;
};

export function ConfirmDialog({ title, text, confirm, run, onClose }: ConfirmProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      await run();
    } catch (err) {
      setError(failText(err, 'Не получилось'));
      setBusy(false);
    }
  };
  return (
    <Dialog title={title} busy={busy} onClose={onClose}>
      <p className="muted">{text}</p>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-bad" onClick={() => void go()} disabled={busy}>
          {busy ? 'Выполняем…' : confirm}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}
