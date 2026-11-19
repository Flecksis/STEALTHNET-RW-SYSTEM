import { useEffect, useRef, useState, type FormEvent } from 'react';
import { api } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { AUDIT, PERIODS, chipsOf, filterCount, type AuditMeta, type ClientChoice, type Period } from './model';
import type { Journal } from './useJournal';

function ActorSelect({ journal }: { journal: Journal }) {
  const { filters, setFilters, meta, client } = journal;
  const admins = meta?.admins ?? [];
  const keys = meta?.keys ?? [];
  const modules = meta?.modules ?? [];
  const custom = filters.actor.startsWith('client:') ? filters.actor : '';
  return (
    <select className="field" value={filters.actor} onChange={(e) => setFilters({ actor: e.target.value })}>
      <option value="">Все участники</option>
      {admins.length ? (
        <optgroup label="Администраторы">
          {admins.map((a) => (
            <option key={a.id} value={`admin:${a.id}`}>
              {a.disabled ? `${a.email} (доступ отключён)` : a.email}
            </option>
          ))}
        </optgroup>
      ) : null}
      {keys.length ? (
        <optgroup label="API-ключи">
          {keys.map((k) => (
            <option key={k.id} value={`key:${k.id}`}>
              {k.revoked ? `${k.name} (отозван)` : k.name}
            </option>
          ))}
        </optgroup>
      ) : null}
      {modules.length ? (
        <optgroup label="Внешние модули">
          {modules.map((m) => (
            <option key={m.id} value={`module:${m.id}`}>
              {m.name}
            </option>
          ))}
        </optgroup>
      ) : null}
      <optgroup label="Другие">
        <option value="system">Панель, автоматически</option>
        <option value="clients">Сами клиенты</option>
      </optgroup>
      {custom ? <option value={custom}>{client && custom === `client:${client.client_id}` ? `${client.label}: действия из кабинета и бота` : 'Клиент: действия из кабинета и бота'}</option> : null}
      {filters.actor && !custom && !hasActor(filters.actor, meta) ? <option value={filters.actor}>{filters.actor}</option> : null}
    </select>
  );
}

function hasActor(value: string, meta: AuditMeta | null): boolean {
  if (value === 'system' || value === 'clients') {
    return true;
  }
  const [kind, id] = value.split(':');
  if (!meta) {
    return true;
  }
  if (kind === 'admin') {
    return meta.admins.some((a) => String(a.id) === id);
  }
  if (kind === 'key') {
    return meta.keys.some((k) => String(k.id) === id);
  }
  return meta.modules.some((m) => m.id === id);
}

function WhatSelect({ journal }: { journal: Journal }) {
  const { filters, setFilters, meta } = journal;
  return (
    <select className="field" value={filters.what} onChange={(e) => setFilters({ what: e.target.value })}>
      <option value="">Все действия</option>
      {(meta?.areas ?? []).map((area) => (
        <optgroup key={area.id} label={area.title}>
          <option value={`area:${area.id}`}>Все: {area.title.toLowerCase()}</option>
          {area.actions.map((action) => (
            <option key={action.code} value={`action:${action.code}`}>
              {action.title}
            </option>
          ))}
        </optgroup>
      ))}
      {filters.what && !meta ? <option value={filters.what}>{filters.what}</option> : null}
    </select>
  );
}

function PeriodFields({ journal }: { journal: Journal }) {
  const { filters, setFilters } = journal;
  return (
    <>
      <select className="field" value={filters.period} onChange={(e) => setFilters({ period: e.target.value as Period })}>
        {PERIODS.map(([key, label]) => (
          <option key={key} value={key}>
            {label}
          </option>
        ))}
      </select>
      {filters.period === 'custom' ? (
        <span className="au-dates">
          <label>
            <span className="sr-only">С какого дня</span>
            <input type="date" className="field" value={filters.since} max={filters.until || undefined} onChange={(e) => setFilters({ since: e.target.value })} />
          </label>
          <span className="muted" aria-hidden="true">
            —
          </span>
          <label>
            <span className="sr-only">По какой день</span>
            <input type="date" className="field" value={filters.until} min={filters.since || undefined} onChange={(e) => setFilters({ until: e.target.value })} />
          </label>
        </span>
      ) : null}
    </>
  );
}

function ClientPicker({ journal }: { journal: Journal }) {
  const { filters, setFilters, client } = journal;
  const [text, setText] = useState('');
  const [choices, setChoices] = useState<ClientChoice[] | null>(null);
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const query = text.trim();
    if (query.length < 2) {
      setChoices(null);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      api<ClientChoice[]>(`${AUDIT}/clients?q=${encodeURIComponent(query)}`)
        .then((list) => {
          if (alive) {
            setChoices(list);
          }
        })
        .catch(() => {
          if (alive) {
            setChoices([]);
          }
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [text]);
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (box.current && !box.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);
  if (filters.client) {
    const label = client && String(client.client_id) === filters.client ? client.label : `Клиент №${filters.client}`;
    return (
      <span className="au-picked">
        <span className="au-picked-name">{label}</span>
        <button type="button" className="au-picked-x" aria-label="Убрать клиента из фильтра" onClick={() => setFilters({ client: '' })}>
          <Icon name="close" size={15} stroke={2.2} />
        </button>
      </span>
    );
  }
  const pick = (choice: ClientChoice) => {
    setText('');
    setOpen(false);
    setChoices(null);
    setFilters({ client: String(choice.id) });
  };
  return (
    <div className="au-pick" ref={box}>
      <input
        type="search"
        className="field"
        value={text}
        placeholder="Имя, @username или номер"
        maxLength={200}
        role="combobox"
        aria-expanded={open && choices !== null}
        aria-autocomplete="list"
        onFocus={() => setOpen(true)}
        onChange={(e) => {
          setText(e.target.value);
          setOpen(true);
        }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            setOpen(false);
          }
          if (e.key === 'Enter' && choices?.[0]) {
            e.preventDefault();
            pick(choices[0]);
          }
        }}
      />
      {open && choices !== null ? (
        <ul className="au-pick-list" role="listbox">
          {choices.length ? (
            choices.map((choice) => (
              <li key={choice.id} role="option" aria-selected="false">
                <button type="button" className="au-pick-row hov" onClick={() => pick(choice)}>
                  <span className="au-pick-name">{choice.label}</span>
                  {choice.note ? <span className="au-pick-note">{choice.note}</span> : null}
                </button>
              </li>
            ))
          ) : (
            <li className="au-pick-empty">Никого не нашли</li>
          )}
        </ul>
      ) : null}
    </div>
  );
}

function SearchField({ journal, phone }: { journal: Journal; phone?: boolean }) {
  const { filters, setFilters, meta } = journal;
  const min = meta?.search_min ?? 3;
  const [text, setText] = useState(filters.q);
  const [short, setShort] = useState(false);
  useEffect(() => {
    setText(filters.q);
  }, [filters.q]);
  const apply = () => {
    const value = text.trim();
    if (value && value.length < min) {
      setShort(true);
      return;
    }
    setShort(false);
    if (value !== filters.q.trim()) {
      setFilters({ q: value });
    }
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    apply();
  };
  return (
    <form role="search" className="au-search" onSubmit={submit}>
      <input
        type="search"
        className="field"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setShort(false);
        }}
        onBlur={apply}
        placeholder={phone ? 'Поиск' : 'Промокод, тариф, причина, адрес…'}
        maxLength={200}
        enterKeyHint="search"
        aria-label="Поиск в подробностях"
      />
      {short ? <span className="au-hint">Хотя бы {min} символа</span> : null}
    </form>
  );
}

export function DeskFilters({ journal }: { journal: Journal }) {
  return (
    <div className="card au-bar" role="group" aria-label="Фильтры журнала">
      <label className="au-bar-field">
        <span className="au-bar-label">Кто</span>
        <ActorSelect journal={journal} />
      </label>
      <label className="au-bar-field">
        <span className="au-bar-label">Действие</span>
        <WhatSelect journal={journal} />
      </label>
      <div className="au-bar-field">
        <span className="au-bar-label">Клиент</span>
        <ClientPicker journal={journal} />
      </div>
      <div className="au-bar-field au-bar-period">
        <span className="au-bar-label">Период</span>
        <span className="au-period">
          <PeriodFields journal={journal} />
        </span>
      </div>
      <div className="au-bar-field grow">
        <span className="au-bar-label">Поиск в подробностях</span>
        <SearchField journal={journal} />
      </div>
      {journal.filtered ? (
        <button type="button" className="au-reset" onClick={journal.reset}>
          Сбросить фильтры
        </button>
      ) : null}
    </div>
  );
}

export function PhoneFilters({ journal }: { journal: Journal }) {
  const [open, setOpen] = useState(false);
  const count = filterCount(journal.filters);
  const chips = chipsOf(journal.filters, journal.meta, journal.client).filter((chip) => chip.key !== 'q');
  useEffect(() => {
    if (!open) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open]);
  return (
    <>
      <div className="au-pbar">
        <SearchField journal={journal} phone />
        <button type="button" className="au-pfilter press" aria-expanded={open} onClick={() => setOpen(true)}>
          <Icon name="sliders" size={18} />
          Фильтры
          {count ? <span className="au-pcount">{count}</span> : null}
        </button>
      </div>
      {chips.length ? (
        <ul className="au-chips" aria-label="Выбранные фильтры">
          {chips.map((chip) => (
            <li key={chip.key}>
              <span className="au-chip">
                <span className="au-chip-text">{chip.label}</span>
                <button type="button" className="au-chip-x" aria-label={`Убрать фильтр: ${chip.label}`} onClick={() => journal.setFilters(chip.clear)}>
                  <Icon name="close" size={14} stroke={2.2} />
                </button>
              </span>
            </li>
          ))}
        </ul>
      ) : null}
      {open ? (
        <div className="psheet-back" role="presentation" onClick={() => setOpen(false)}>
          <div className="psheet au-sheet" role="dialog" aria-modal="true" aria-label="Фильтры журнала" onClick={(event) => event.stopPropagation()}>
            <label className="au-bar-field">
              <span className="au-bar-label">Кто</span>
              <ActorSelect journal={journal} />
            </label>
            <label className="au-bar-field">
              <span className="au-bar-label">Действие</span>
              <WhatSelect journal={journal} />
            </label>
            <div className="au-bar-field">
              <span className="au-bar-label">Клиент</span>
              <ClientPicker journal={journal} />
            </div>
            <div className="au-bar-field">
              <span className="au-bar-label">Период</span>
              <span className="au-period">
                <PeriodFields journal={journal} />
              </span>
            </div>
            <div className="au-sheet-acts">
              {journal.filtered ? (
                <button type="button" className="b44" onClick={journal.reset}>
                  Сбросить
                </button>
              ) : null}
              <button type="button" className="btn-main btn" onClick={() => setOpen(false)}>
                Готово
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
