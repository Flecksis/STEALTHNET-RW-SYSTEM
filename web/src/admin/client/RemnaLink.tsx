import { useId, useState, type FormEvent } from 'react';
import { api, ApiError } from '../../lib/api';
import { dateLong, traffic, trafficPair } from '../../lib/format';
import type { Mirror, Subscription } from '../clients/types';
import { Dialog } from './dialogs';
import './remna-link.css';

type Terms = {
  status: Mirror['status'];
  expire_at: string | null;
  traffic_limit_bytes: number | null;
  traffic_used_bytes: number | null;
  traffic_strategy: string | null;
  hwid_limit: number | null;
  squads: string[];
  synced: boolean;
};

type Holder = {
  subscription_id: number;
  client: { client_id: number; name: string | null; telegram_username: string | null };
  same_client: boolean;
};

type FoundBy = 'username' | 'short_link' | 'telegram' | 'email';

type TariffPick = { kind: 'one'; id: number; name: string; extra_devices: number } | { kind: 'many'; names: string[] } | { kind: 'none' };

type Candidate = {
  id: number;
  username: string;
  short_uuid: string;
  telegram_id: number | null;
  email: string | null;
  description: string | null;
  found_by: FoundBy[];
  terms: Terms;
  holder: Holder | null;
  tariff?: TariffPick;
};

type Search = { panel?: Terms; users: Candidate[] };

type Linked = { user_id: number; username: string; refreshed: boolean; released: number[] };

type Added = { subscription_id: number; user_id: number; username: string; tariff: string | null; refreshed: boolean };

type Target = { kind: 'link'; subscriptionId: number } | { kind: 'add'; clientId: number };

const FOUND_BY: Record<FoundBy, string> = {
  username: 'по имени',
  short_link: 'по ссылке',
  telegram: 'по Telegram ID',
  email: 'по почте',
};

const STATUS: Record<string, string> = {
  active: 'активна',
  limited: 'трафик исчерпан',
  expired: 'истекла',
  disabled: 'отключена',
};

const AGAIN = new Set(['remna_link.taken', 'remna_link.user_changed', 'remna_link.user_gone']);

const LATER = '. Данные из Remnawave подтянутся при следующей сверке';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function holderName(holder: Holder): string {
  const { client } = holder;
  if (client.telegram_username) {
    return `@${client.telegram_username}`;
  }
  return client.name ?? `№${client.client_id}`;
}

type Cell = { text: string; key: string | null };

type Row = { label: string; panel: string; remna: string; differs: boolean };

function expireCell(t: Terms): Cell {
  return t.expire_at ? { text: `до ${dateLong(t.expire_at, true)}`, key: t.expire_at.slice(0, 10) } : { text: '—', key: null };
}

function trafficCell(t: Terms): Cell {
  if (t.traffic_limit_bytes === null) {
    return { text: '—', key: null };
  }
  const used = t.traffic_used_bytes ?? 0;
  const limit = t.traffic_limit_bytes;
  return { text: limit > 0 ? trafficPair(used, limit) : `без лимита, ${traffic(used)}`, key: String(limit) };
}

function devicesCell(t: Terms): Cell {
  if (t.hwid_limit !== null) {
    return { text: String(t.hwid_limit), key: String(t.hwid_limit) };
  }
  return t.synced ? { text: 'без ограничения', key: 'none' } : { text: '—', key: null };
}

function squadsCell(t: Terms, side: 'panel' | 'remna'): Cell {
  if (t.squads.length) {
    const sorted = [...t.squads].sort();
    return { text: sorted.join(', '), key: sorted.join('\n') };
  }
  return side === 'remna' ? { text: 'нет', key: '' } : { text: '—', key: null };
}

function statusCell(t: Terms): Cell {
  return t.status ? { text: STATUS[t.status] ?? t.status, key: t.status } : { text: '—', key: null };
}

function rows(panel: Terms, user: Terms): Row[] {
  const pairs: Array<[string, Cell, Cell]> = [
    ['Срок', expireCell(panel), expireCell(user)],
    ['Трафик', trafficCell(panel), trafficCell(user)],
    ['Устройства', devicesCell(panel), devicesCell(user)],
    ['Сквады', squadsCell(panel, 'panel'), squadsCell(user, 'remna')],
    ['Статус', statusCell(panel), statusCell(user)],
  ];
  return pairs.map(([label, mine, theirs]) => ({
    label,
    panel: mine.text,
    remna: theirs.text,
    differs: mine.key !== null && theirs.key !== null && mine.key !== theirs.key,
  }));
}

function Comparison({ table, phone }: { table: Row[]; phone: boolean }) {
  if (phone) {
    return (
      <div className="rlink-pairs">
        <div className="rlink-cols cap" aria-hidden="true">
          <span>В панели</span>
          <span>В Remnawave</span>
        </div>
        <dl className="rlink-dl">
          {table.map((row) => (
            <div key={row.label} className="rlink-pair">
              <dt>{row.label}</dt>
              <dd>
                <span className="sr-only">В панели: </span>
                {row.panel}
              </dd>
              <dd className={row.differs ? 'rlink-diff' : undefined}>
                <span className="sr-only">В Remnawave: </span>
                {row.remna}
              </dd>
            </div>
          ))}
        </dl>
      </div>
    );
  }
  return (
    <table className="rlink-cmp">
      <thead>
        <tr>
          <th scope="col">
            <span className="sr-only">Что сравниваем</span>
          </th>
          <th scope="col">В панели</th>
          <th scope="col">В Remnawave</th>
        </tr>
      </thead>
      <tbody>
        {table.map((row) => (
          <tr key={row.label}>
            <th scope="row">{row.label}</th>
            <td>{row.panel}</td>
            <td className={row.differs ? 'rlink-diff' : undefined}>{row.remna}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Summary({ table }: { table: Row[] }) {
  const differ = table.filter((row) => row.differs).map((row) => row.label.toLowerCase());
  const unknown = table.filter((row) => row.panel === '—').map((row) => row.label.toLowerCase());
  return (
    <>
      {differ.length ? <p className="warn-line">Не совпадает: {differ.join(', ')}</p> : null}
      {unknown.length ? <p className="cap">В панели нет данных: {unknown.join(', ')}</p> : null}
      {!differ.length && !unknown.length ? <p className="cap">Срок, трафик, устройства, сквады и статус совпадают</p> : null}
    </>
  );
}

function Facts({ terms }: { terms: Terms }) {
  const facts: Array<[string, Cell]> = [
    ['Срок', expireCell(terms)],
    ['Трафик', trafficCell(terms)],
    ['Устройства', devicesCell(terms)],
    ['Сквады', squadsCell(terms, 'remna')],
    ['Статус', statusCell(terms)],
  ];
  return (
    <dl className="rlink-facts">
      {facts.map(([label, cell]) => (
        <div key={label} className="rlink-fact">
          <dt>{label}</dt>
          <dd>{cell.text}</dd>
        </div>
      ))}
    </dl>
  );
}

function tariffText(pick: TariffPick | undefined): string {
  if (!pick || pick.kind === 'none') {
    return 'Тариф с такими сквадами, трафиком и устройствами не нашёлся: подписка будет без тарифа.';
  }
  if (pick.kind === 'many') {
    return `Подходят несколько тарифов (${pick.names.join(', ')}): подписка будет без тарифа.`;
  }
  return pick.extra_devices > 0 ? `Тариф «${pick.name}», доп. устройств: ${pick.extra_devices}.` : `Тариф «${pick.name}».`;
}

type DialogProps = {
  target: Target;
  lead: string;
  phone: boolean;
  onClose: () => void;
  onDone: (text: string, subscriptionId: number | null) => void;
};

function RemnaDialog({ target, lead, phone, onClose, onDone }: DialogProps) {
  const [query, setQuery] = useState('');
  const [asked, setAsked] = useState('');
  const [found, setFound] = useState<Search | null>(null);
  const [chosen, setChosen] = useState<Candidate | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fieldId = useId();
  const linking = target.kind === 'link';
  const base = linking ? `/api/admin/subscriptions/${target.subscriptionId}/remna` : `/api/admin/clients/${target.clientId}/remna`;
  const title = linking ? 'Привязать к пользователю Remnawave' : 'Добавить подписку из Remnawave';

  const search = async (text: string, keepError = false) => {
    setBusy(true);
    if (!keepError) {
      setError(null);
    }
    try {
      const result = await api<Search>(`${base}/search`, { method: 'POST', body: { q: text } });
      setFound(result);
      setAsked(text);
      setChosen(null);
    } catch (err) {
      setError(failText(err, 'Поиск не удался'));
    } finally {
      setBusy(false);
    }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (query.trim()) {
      void search(query.trim());
    }
  };

  const confirm = async (user: Candidate) => {
    setBusy(true);
    setError(null);
    const body = { user_id: user.id, username: user.username };
    try {
      if (linking) {
        const linked = await api<Linked>(`${base}/link`, { method: 'POST', body });
        onDone(`Подписка привязана к пользователю Remnawave ${linked.username}${linked.refreshed ? '' : LATER}`, null);
      } else {
        const added = await api<Added>(`${base}/add`, { method: 'POST', body });
        const tariff = added.tariff ? `тариф «${added.tariff}»` : 'без тарифа';
        onDone(`Добавлена подписка из Remnawave ${added.username}, ${tariff}${added.refreshed ? '' : LATER}`, added.subscription_id);
      }
    } catch (err) {
      setError(failText(err, linking ? 'Привязать не получилось' : 'Подписка не добавилась'));
      setBusy(false);
      if (err instanceof ApiError && AGAIN.has(err.code) && asked) {
        await search(asked, true);
      }
    }
  };

  if (chosen && found) {
    const who = [`№${chosen.id}`, chosen.telegram_id ? `Telegram ${chosen.telegram_id}` : null, chosen.email].filter(Boolean).join(' · ');
    const table = found.panel ? rows(found.panel, chosen.terms) : null;
    return (
      <Dialog title={title} busy={busy} onClose={onClose} wide>
        <div className="rlink-who">
          <span className="rlink-name">{chosen.username}</span>
          <span className="cap">{who}</span>
          {chosen.description ? <span className="cap rlink-desc">{chosen.description}</span> : null}
        </div>
        {table ? (
          <>
            <Comparison table={table} phone={phone} />
            <Summary table={table} />
            <p className="cap">
              Панель возьмёт срок, трафик, устройства и ссылку подписки у этого пользователя. В Remnawave ничего не изменится, клиенту ничего не придёт.
            </p>
          </>
        ) : (
          <>
            <Facts terms={chosen.terms} />
            <p>{tariffText(chosen.tariff)}</p>
            <p className="cap">
              Клиенту добавится подписка с этим пользователем: срок, трафик, устройства и ссылка как в Remnawave, автопродление выключено. В Remnawave ничего не изменится, клиенту ничего не придёт.
            </p>
          </>
        )}
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <div className="dialog-actions">
          <button type="button" className="btn-main btn" onClick={() => void confirm(chosen)} disabled={busy}>
            {busy ? (linking ? 'Привязываем…' : 'Добавляем…') : linking ? 'Привязать' : 'Добавить подписку'}
          </button>
          <button
            type="button"
            className="btn-line hov"
            onClick={() => {
              setChosen(null);
              setError(null);
            }}
            disabled={busy}
          >
            Назад
          </button>
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog title={title} busy={busy} onClose={onClose} wide>
      <p className="muted">{lead}</p>
      <form className="field-row" onSubmit={submit}>
        <label htmlFor={fieldId}>Кого искать в Remnawave</label>
        <span className="rlink-search">
          <input
            id={fieldId}
            className="field"
            value={query}
            maxLength={200}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Имя, ссылка, Telegram ID или почта"
            disabled={busy}
            autoFocus
            autoComplete="off"
            spellCheck={false}
          />
          <button type="submit" className="b40 hov" disabled={busy || !query.trim()}>
            Найти
          </button>
        </span>
        <span className="cap">Имя ищется точно, как в Remnawave. Ссылку можно вставить целиком</span>
      </form>
      {found && found.users.length === 0 ? <p className="muted">Никого не нашли. Проверьте написание или поищите по другому признаку</p> : null}
      {found && found.users.length > 0 ? (
        <ul className="rlink-list" aria-label="Найденные пользователи Remnawave">
          {found.users.map((user) => {
            const meta = [
              user.found_by.map((by) => FOUND_BY[by]).join(', '),
              user.terms.expire_at ? `до ${dateLong(user.terms.expire_at, true)}` : null,
              user.telegram_id && !user.found_by.includes('telegram') ? `Telegram ${user.telegram_id}` : null,
            ]
              .filter(Boolean)
              .join(' · ');
            if (user.holder) {
              const holder = user.holder;
              return (
                <li key={user.id} className="rlink-item">
                  <span className="rlink-name">
                    {user.username} <span className="muted">№{user.id}</span>
                  </span>
                  <span className="cap">{meta}</span>
                  <span className="warn-line">
                    {holder.same_client
                      ? `Уже привязан к ${linking ? 'другой ' : ''}подписке этого клиента, №${holder.subscription_id}`
                      : `Уже привязан к подписке №${holder.subscription_id} клиента `}
                    {holder.same_client ? null : (
                      <a href={`/admin/clients/${holder.client.client_id}`} target="_blank" rel="noreferrer">
                        {holderName(holder)}
                      </a>
                    )}
                  </span>
                </li>
              );
            }
            return (
              <li key={user.id}>
                <button type="button" className="rlink-item rlink-pick hov" onClick={() => setChosen(user)} disabled={busy}>
                  <span className="rlink-name">
                    {user.username} <span className="muted">№{user.id}</span>
                  </span>
                  <span className="cap">{meta}</span>
                </button>
              </li>
            );
          })}
        </ul>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

type NoteProps = {
  sub: Subscription;
  missingSince: string | null;
  writable: boolean;
  phone?: boolean;
  onLinked: (text: string) => void;
};

export function RemnaLinkNote({ sub, missingSince, writable, phone = false, onLinked }: NoteProps) {
  const [open, setOpen] = useState(false);
  if (sub.ended_at || (sub.remna && !missingSince)) {
    return null;
  }
  const head = sub.remna
    ? `Пользователя Remnawave ${sub.remna.username} нет${missingSince ? ` с ${dateLong(missingSince)}` : ''}.`
    : 'Подписка не связана с пользователем Remnawave.';
  const tail = sub.remna ? ' Его удалили, или его номер теперь у другого пользователя.' : ' Панель не знает её ссылку и устройства.';
  const lead = sub.remna
    ? `Пользователя ${sub.remna.username} в Remnawave больше нет или под его номером теперь другой. Найдите нужного пользователя по имени, ссылке подписки, Telegram ID или почте.`
    : 'Найдите пользователя Remnawave этой подписки по имени, ссылке подписки, Telegram ID или почте.';
  const dialog = open ? (
    <RemnaDialog
      target={{ kind: 'link', subscriptionId: sub.id }}
      lead={lead}
      phone={phone}
      onClose={() => setOpen(false)}
      onDone={(text) => {
        setOpen(false);
        onLinked(text);
      }}
    />
  ) : null;
  if (phone) {
    return (
      <>
        <div role="note" className="pc-ask rlink-note">
          <span>
            <b>{head}</b>
            {tail}
          </span>
          {writable ? (
            <span className="pc-ask-btns">
              <button type="button" className="pc-ok press" onClick={() => setOpen(true)}>
                Привязать
              </button>
            </span>
          ) : null}
        </div>
        {dialog}
      </>
    );
  }
  return (
    <>
      <div role="note" className="cc-note">
        <span className="cc-note-text wide">
          <strong>{head}</strong>
          <span>{tail}</span>
        </span>
        {writable ? (
          <span className="cc-btns">
            <button type="button" className="b34 on-surface hov" onClick={() => setOpen(true)}>
              Привязать к пользователю Remnawave
            </button>
          </span>
        ) : null}
      </div>
      {dialog}
    </>
  );
}

const ADD_LEAD =
  'Найдите пользователя Remnawave, которого ещё нет в панели: по имени, ссылке подписки, Telegram ID или почте. Панель добавит клиенту подписку с этим пользователем.';

type AddDialogProps = {
  clientId: number;
  phone?: boolean;
  onClose: () => void;
  onDone: (text: string, subscriptionId: number) => void;
};

export function RemnaAddDialog({ clientId, phone = false, onClose, onDone }: AddDialogProps) {
  return (
    <RemnaDialog
      target={{ kind: 'add', clientId }}
      lead={ADD_LEAD}
      phone={phone}
      onClose={onClose}
      onDone={(text, subscriptionId) => onDone(text, subscriptionId ?? 0)}
    />
  );
}

type AddButtonProps = {
  clientId: number;
  more?: boolean;
  onAdded: (text: string, subscriptionId: number) => void;
};

export function RemnaAddButton({ clientId, more = false, onAdded }: AddButtonProps) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className={more ? 'cc-more rlink-more' : 'b38 hov'} onClick={() => setOpen(true)}>
        {more ? 'Добавить ещё подписку из Remnawave' : 'Добавить из Remnawave'}
      </button>
      {open ? (
        <RemnaAddDialog
          clientId={clientId}
          onClose={() => setOpen(false)}
          onDone={(text, subscriptionId) => {
            setOpen(false);
            onAdded(text, subscriptionId);
          }}
        />
      ) : null}
    </>
  );
}
