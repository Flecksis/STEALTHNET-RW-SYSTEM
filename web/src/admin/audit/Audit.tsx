import { useEffect, useState, type MouseEvent } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router';
import { api } from '../../lib/api';
import { dateTime } from '../../lib/format';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import { ActorMark, Detail } from './Detail';
import { DeskFilters, PhoneFilters } from './Filters';
import { AUDIT, PAGE, actorKind, actorShort, summaryOf, whenShort, type AuditItem } from './model';
import { failText, useJournal, type Journal } from './useJournal';
import '../form.css';
import '../promo/promo.css';
import '../home/phone.css';
import './audit.css';

function useEntry(id: number | null, items: AuditItem[] | null) {
  const found = id !== null ? (items?.find((item) => item.id === id) ?? null) : null;
  const [loaded, setLoaded] = useState<{ id: number; item: AuditItem | null; error: string | null } | null>(null);
  useEffect(() => {
    if (id === null || found) {
      return;
    }
    let alive = true;
    api<AuditItem>(`${AUDIT}/${id}`)
      .then((item) => {
        if (alive) {
          setLoaded({ id, item, error: null });
        }
      })
      .catch((err) => {
        if (alive) {
          setLoaded({ id, item: null, error: failText(err, 'Запись не загрузилась') });
        }
      });
    return () => {
      alive = false;
    };
  }, [id, found]);
  if (found) {
    return { item: found, error: null };
  }
  if (loaded && loaded.id === id) {
    return { item: loaded.item, error: loaded.error };
  }
  return { item: null, error: null };
}

function Empty({ journal, phone }: { journal: Journal; phone?: boolean }) {
  return (
    <p className={phone ? 'hp-quiet hp-card' : 'muted pr-pad'}>
      {journal.filtered ? 'Под эти условия записей нет.' : 'Записей пока нет: они появятся, когда в панели что-то изменят.'}
      {journal.filtered ? (
        <>
          {' '}
          <button type="button" className="au-link" onClick={journal.reset}>
            Сбросить фильтры
          </button>
        </>
      ) : null}
    </p>
  );
}

function More({ journal, phone }: { journal: Journal; phone?: boolean }) {
  if (journal.next === null) {
    return null;
  }
  return phone ? (
    <button type="button" className="pr-padd press" onClick={() => void journal.more()} disabled={journal.busy}>
      {journal.busy ? 'Загружаем…' : 'Показать ещё'}
    </button>
  ) : (
    <div className="pr-pad">
      <button type="button" className="b40 hov" onClick={() => void journal.more()} disabled={journal.busy}>
        {journal.busy ? 'Загружаем…' : 'Показать ещё'}
      </button>
    </div>
  );
}

function Desk({ journal, id }: { journal: Journal; id: number | null }) {
  const navigate = useNavigate();
  const { search } = useLocation();
  const items = journal.items ?? [];
  const selectedId = id ?? items[0]?.id ?? null;
  const entry = useEntry(selectedId, journal.items);
  const open = (item: AuditItem) => navigate(`${PAGE}/${item.id}${search}`, { replace: true });
  const onRow = (event: MouseEvent, item: AuditItem) => {
    if ((event.target as HTMLElement).closest('a, button')) {
      return;
    }
    open(item);
  };
  return (
    <div className="pr-page au-page">
      <div className="pr-head">
        <h1 className="page-title tight">Журнал действий</h1>
      </div>
      <DeskFilters journal={journal} />
      <div className="au-split">
        <section className="card pr-list au-list" aria-label="Записи журнала">
          {journal.error ? (
            <p className="err pr-pad" role="alert">
              {journal.error}
            </p>
          ) : null}
          {journal.items === null ? <p className="muted pr-pad">Загружаем журнал…</p> : null}
          {journal.items !== null && items.length === 0 && !journal.error ? <Empty journal={journal} /> : null}
          {items.length ? (
            <div role="table" aria-label="Журнал действий">
              <div role="row" className="au-row pr-th">
                <span role="columnheader">Когда</span>
                <span role="columnheader">Кто</span>
                <span role="columnheader">Что сделано</span>
                <span role="columnheader">С чем</span>
              </div>
              {items.map((item) => {
                const summary = summaryOf(item);
                const current = item.id === selectedId;
                return (
                  <div key={item.id} role="row" className={current ? 'au-row on' : 'au-row hov'} aria-selected={current} onClick={(event) => onRow(event, item)}>
                    <span role="cell" className="au-when num">
                      {whenShort(item.at)}
                    </span>
                    <span role="cell" className="au-who-cell">
                      <ActorMark actor={item.actor} size={28} />
                      <span className="pr-cell">
                        <span className="au-ellipsis">{actorShort(item.actor)}</span>
                        <span className="pr-sub au-ellipsis">{actorKind(item.actor)}</span>
                      </span>
                    </span>
                    <span role="cell" className="pr-cell">
                      <Link
                        to={`${PAGE}/${item.id}${search}`}
                        replace
                        className="au-what au-ellipsis"
                        aria-current={current ? 'true' : undefined}
                      >
                        {item.title}
                      </Link>
                      {summary ? <span className="pr-sub au-ellipsis">{summary}</span> : null}
                    </span>
                    <span role="cell" className="pr-cell">
                      <span className="au-ellipsis">{item.target?.label ?? '—'}</span>
                      {item.target?.note ? <span className="pr-sub au-ellipsis">{item.target.note}</span> : null}
                    </span>
                  </div>
                );
              })}
            </div>
          ) : null}
          <More journal={journal} />
        </section>
        {entry.item || entry.error ? (
          <aside className="card au-side" aria-label="Подробности записи">
            {entry.item ? <Detail item={entry.item} onFilter={journal.setFilters} /> : <p className="err pr-pad">{entry.error}</p>}
          </aside>
        ) : null}
      </div>
    </div>
  );
}

function PhoneList({ journal }: { journal: Journal }) {
  const { search } = useLocation();
  const items = journal.items ?? [];
  return (
    <div className="hp au-phone">
      <PhoneHeader title="Журнал действий" />
      <PhoneFilters journal={journal} />
      {journal.error ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{journal.error}</span>
        </p>
      ) : null}
      {journal.items === null ? <p className="hp-quiet hp-card">Загружаем журнал…</p> : null}
      {journal.items !== null && items.length === 0 && !journal.error ? <Empty journal={journal} phone /> : null}
      {items.length ? (
        <ul className="hp-card hp-list au-plist" aria-label="Журнал действий">
          {items.map((item) => (
            <li key={item.id}>
              <Link to={`${PAGE}/${item.id}${search}`} className="au-plink">
                <span className="hp-title">{item.title}</span>
                {item.target ? <span className="hp-note">{item.target.label}</span> : null}
                <span className="hp-note muted">
                  {dateTime(item.at)} · {actorShort(item.actor)}
                </span>
              </Link>
            </li>
          ))}
        </ul>
      ) : null}
      <More journal={journal} phone />
    </div>
  );
}

function PhoneEntry({ journal, id }: { journal: Journal; id: number }) {
  const { search } = useLocation();
  const entry = useEntry(id, journal.items);
  return (
    <div className="hp au-phone">
      <PhoneHeader title="Запись журнала" back={{ to: `${PAGE}${search}`, label: 'Журнал действий' }} />
      {entry.item ? <Detail item={entry.item} phone onFilter={journal.setFilters} /> : null}
      {entry.error ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{entry.error}</span>
        </p>
      ) : null}
      {!entry.item && !entry.error ? <p className="hp-quiet hp-card">Загружаем запись…</p> : null}
    </div>
  );
}

function Locked() {
  return (
    <div className="pr-page au-page">
      <div className="pr-head">
        <h1 className="page-title tight">Журнал действий</h1>
      </div>
      <p className="card card-pad muted">Журнал видят владелец и администраторы с правом «Журнал действий». Право выдаёт владелец в «Настройки → Администраторы».</p>
    </div>
  );
}

export function Audit() {
  const { can } = useSession();
  const phone = usePhone();
  const params = useParams();
  const parsed = Number(params.id);
  const id = Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null;
  const allowed = can('audit_log');
  const journal = useJournal(allowed);
  if (!allowed) {
    return <Locked />;
  }
  if (phone) {
    return id !== null ? <PhoneEntry journal={journal} id={id} /> : <PhoneList journal={journal} />;
  }
  return <Desk journal={journal} id={id} />;
}
