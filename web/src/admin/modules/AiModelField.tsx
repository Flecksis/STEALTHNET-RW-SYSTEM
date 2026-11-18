import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { api, ApiError } from '../../lib/api';
import { groupDigits } from '../../lib/format';
import { secretFilled, type FieldSlot, type FieldValues } from '../SchemaFields';

type ModelOption = { id: string; free: boolean; context?: number; price_in?: number; price_out?: number };

type Listing = { ok: boolean; cached: boolean; provider: string; model: string; model_found: boolean; models: ModelOption[]; error: string | null };

type Loading = { kind: 'idle'; note: string } | { kind: 'loading' } | { kind: 'ready'; listing: Listing } | { kind: 'failed'; text: string };

type Row = { id: string; meta: string; missing: boolean };

const SHOWN = 60;
const KEY_MIN = 8;
const WAIT = 700;

function text(value: FieldValues[string] | undefined): string {
  return typeof value === 'string' ? value.trim() : '';
}

function stored(config: Record<string, unknown>, key: string): string {
  const value = config[key];
  return typeof value === 'string' ? value.trim() : '';
}

function bare(url: string): string {
  return url.replace(/\/+$/, '');
}

function tokens(count: number): string {
  return count >= 1_000_000 ? `${+(count / 1_000_000).toFixed(1)}M` : `${Math.max(1, Math.round(count / 1000))}K`;
}

function usd(amount: number): string {
  if (amount === 0) {
    return '$0';
  }
  return amount < 0.01 ? `$${amount.toPrecision(1)}` : `$${amount.toFixed(2)}`;
}

function meta(option: ModelOption): string {
  const parts: string[] = [];
  if (option.free) {
    parts.push('бесплатная');
  }
  if (option.context) {
    parts.push(`контекст ${tokens(option.context)}`);
  }
  if (!option.free && option.price_in !== undefined && option.price_out !== undefined) {
    parts.push(`${usd(option.price_in)} / ${usd(option.price_out)} за 1 млн токенов`);
  }
  return parts.join(' · ');
}

type Props = { slot: FieldSlot; values: FieldValues; config: Record<string, unknown>; busy: boolean };

export function AiModelField({ slot, values, config, busy }: Props) {
  const provider = text(values.provider) || 'groq';
  const apiUrl = text(values.api_url);
  const typedKey = text(values.api_key);
  const savedProvider = stored(config, 'provider') || 'groq';
  const sameAddress = provider === savedProvider && bare(apiUrl) === bare(stored(config, 'api_url'));
  const source = typedKey.length >= KEY_MIN ? 'typed' : secretFilled(config, 'api_key') && sameAddress ? 'saved' : null;
  const needsUrl = provider === 'custom' && !apiUrl;

  const [loading, setLoading] = useState<Loading>({ kind: 'idle', note: '' });
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState(false);
  const [active, setActive] = useState(-1);
  const seq = useRef(0);
  const model = useRef(slot.value);
  const listId = `${slot.id}-list`;

  useEffect(() => {
    model.current = slot.value;
  }, [slot.value]);

  const load = useCallback(
    async (refresh: boolean) => {
      const mine = ++seq.current;
      setLoading({ kind: 'loading' });
      try {
        const listing = await api<Listing>('/api/admin/m/ai/models', {
          method: 'POST',
          body: { provider, api_url: apiUrl, api_key: typedKey, model: model.current.trim(), refresh },
        });
        if (seq.current === mine) {
          setLoading(listing.ok ? { kind: 'ready', listing } : { kind: 'failed', text: listing.error ?? 'Провайдер не ответил' });
        }
      } catch (err) {
        if (seq.current === mine) {
          setLoading({ kind: 'failed', text: err instanceof ApiError ? err.message : 'Список моделей не загрузился' });
        }
      }
    },
    [provider, apiUrl, typedKey],
  );

  useEffect(() => {
    seq.current += 1;
    if (needsUrl) {
      setLoading({ kind: 'idle', note: 'Укажите адрес API, и список моделей загрузится сам.' });
      return;
    }
    if (source === null) {
      setLoading({ kind: 'idle', note: 'Вставьте ключ, и список моделей загрузится сам.' });
      return;
    }
    const timer = window.setTimeout(() => void load(false), source === 'typed' ? WAIT : 0);
    return () => window.clearTimeout(timer);
  }, [load, needsUrl, source]);

  const listing = loading.kind === 'ready' ? loading.listing : null;
  const models = listing?.models ?? [];
  const current = slot.value.trim() || listing?.model || '';
  const found = listing !== null && (current === listing.model ? listing.model_found : models.some((m) => m.id === current));
  const missing = listing !== null && models.length > 0 && current !== '' && !found;
  const query = typed ? slot.value.trim().toLowerCase() : '';
  const chosen = models.find((m) => m.id === current);
  const pinned: Row[] = query || !current ? [] : chosen ? [{ id: current, meta: meta(chosen), missing: false }] : missing ? [{ id: current, meta: 'нет у провайдера', missing: true }] : [];
  const matches = query ? models.filter((m) => m.id.toLowerCase().includes(query)) : models.filter((m) => m.id !== current);
  const rows: Row[] = [...pinned, ...matches.slice(0, SHOWN).map((m) => ({ id: m.id, meta: meta(m), missing: false }))];
  const more = matches.length - Math.min(matches.length, SHOWN);
  const shown = open && listing !== null && rows.length > 0;
  const free = models.filter((m) => m.free).length;

  useEffect(() => {
    if (shown && active >= 0) {
      document.getElementById(`${listId}-${active}`)?.scrollIntoView({ block: 'nearest' });
    }
  }, [active, listId, shown]);

  const pick = (id: string) => {
    slot.set(id);
    setOpen(false);
    setTyped(false);
    setActive(-1);
  };

  const onKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' && rows.length) {
      event.preventDefault();
      setOpen(true);
      setActive((n) => Math.min(n + 1, rows.length - 1));
    } else if (event.key === 'ArrowUp' && rows.length) {
      event.preventDefault();
      setActive((n) => Math.max(n - 1, 0));
    } else if (event.key === 'Enter' && shown && active >= 0 && rows[active]) {
      event.preventDefault();
      pick(rows[active].id);
    } else if (event.key === 'Escape' && open) {
      event.preventDefault();
      setOpen(false);
    }
  };

  const status =
    loading.kind === 'loading'
      ? { text: 'Загружаем модели…', bad: false }
      : loading.kind === 'failed'
        ? { text: loading.text, bad: true }
        : loading.kind === 'idle'
          ? { text: loading.note, bad: false }
          : models.length
            ? { text: `Ключ работает. Моделей у провайдера: ${groupDigits(models.length)}${free ? `, бесплатных: ${groupDigits(free)}` : ''}.`, bad: false }
            : { text: 'Ключ работает, но провайдер не отдал список моделей. Впишите модель вручную.', bad: false };

  return (
    <>
      <div className="ai-pick">
        <input
          id={slot.id}
          className="field"
          role="combobox"
          aria-expanded={shown}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={shown && active >= 0 ? `${listId}-${active}` : undefined}
          aria-describedby={slot.hintId}
          value={slot.value}
          placeholder={slot.placeholder}
          spellCheck={false}
          autoComplete="off"
          disabled={busy}
          onChange={(e) => {
            slot.set(e.target.value);
            setTyped(true);
            setOpen(true);
            setActive(-1);
          }}
          onFocus={() => {
            setTyped(false);
            setOpen(true);
            setActive(pinned.length ? 0 : -1);
          }}
          onClick={() => setOpen(true)}
          onBlur={() => setOpen(false)}
          onKeyDown={onKey}
        />
        {shown ? (
          <ul id={listId} role="listbox" className="ai-list" aria-label="Модели провайдера">
            {rows.map((row, index) => (
              <li
                key={`${row.missing ? 'missing' : 'model'}-${row.id}`}
                id={`${listId}-${index}`}
                role="option"
                aria-selected={row.id === current}
                className={index === active ? 'ai-option active' : 'ai-option'}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pick(row.id)}
              >
                <span className="ai-option-id">{row.id}</span>
                {row.meta ? <span className={row.missing ? 'ai-option-meta warn' : 'ai-option-meta'}>{row.meta}</span> : null}
              </li>
            ))}
            {more > 0 ? (
              <li className="ai-more" role="presentation">
                Ещё {groupDigits(more)}: уточните поиск
              </li>
            ) : null}
          </ul>
        ) : null}
      </div>
      <div className="ai-status">
        {status.text ? (
          <span role="status" className={status.bad ? 'md-warn' : 'muted'}>
            {status.text}
          </span>
        ) : null}
        {source !== null && !needsUrl ? (
          <button type="button" className="b34 hov" onClick={() => void load(true)} disabled={busy || loading.kind === 'loading'}>
            Обновить список
          </button>
        ) : null}
      </div>
      {missing ? <span className="md-warn">Модели {current} нет у провайдера. Выберите другую из списка или оставьте, если уверены в названии.</span> : null}
      {slot.hint}
    </>
  );
}
