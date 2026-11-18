import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react';
import { Link, useLocation, useSearchParams } from 'react-router';
import { api } from '../../lib/api';
import { dateTime } from '../../lib/format';
import { usePhone } from '../../lib/media';
import { personName } from '../home/model';
import { PhoneHeader } from '../phone';
import { MoneyTabs } from './MoneyTabs';
import {
  EMPTY,
  JOURNAL,
  OUTCOMES,
  PERIODS,
  deliveriesText,
  eventText,
  failText,
  filtersOf,
  legacyText,
  paramsOf,
  queryOf,
  retentionText,
  toneColor,
  verdictText,
  type Filters,
  type JournalItem,
  type JournalPage,
  type Period,
} from './journal';
import '../form.css';
import '../promo/promo.css';
import '../home/phone.css';
import '../protect/protect.css';
import './webhooks.css';

function useJournal() {
  const [params, setParams] = useSearchParams();
  const filters = useMemo(() => filtersOf(params), [params]);
  const [items, setItems] = useState<JournalItem[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [meta, setMeta] = useState<Pick<JournalPage, 'providers' | 'retention'> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async (current: Filters) => {
    setError(null);
    try {
      const page = await api<JournalPage>(queryOf(current, null));
      setItems(page.items);
      setNext(page.next);
      setMeta({ providers: page.providers, retention: page.retention });
    } catch (err) {
      setItems([]);
      setNext(null);
      setError(failText(err, 'Журнал не загрузился'));
    }
  }, []);

  useEffect(() => {
    setItems(null);
    void load(filters);
  }, [filters, load]);

  const setFilters = useCallback(
    (patch: Partial<Filters>) => {
      setParams(paramsOf({ ...filters, ...patch }), { replace: true });
    },
    [filters, setParams],
  );

  const more = async () => {
    if (next === null) {
      return;
    }
    setBusy(true);
    try {
      const page = await api<JournalPage>(queryOf(filters, next));
      setItems((list) => [...(list ?? []), ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'Журнал не загрузился'));
    } finally {
      setBusy(false);
    }
  };

  const filtered = paramsOf(filters).toString() !== '';
  return { filters, setFilters, filtered, items, next, meta, error, busy, more };
}

type Data = ReturnType<typeof useJournal>;

export function Webhooks() {
  const phone = usePhone();
  const data = useJournal();
  return phone ? <JournalPhone data={data} /> : <JournalDesk data={data} />;
}

function cardLink(item: JournalItem): string {
  return `${JOURNAL}/${item.id}`;
}

function useBack(): { back: string } {
  const { search } = useLocation();
  return { back: search };
}

function SearchField({ data, phone }: { data: Data; phone?: boolean }) {
  const [text, setText] = useState(data.filters.q);
  useEffect(() => {
    setText(data.filters.q);
  }, [data.filters.q]);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    data.setFilters({ q: text });
  };
  return (
    <form role="search" className={phone ? 'wh-psearch' : 'wh-search'} onSubmit={submit}>
      <label className="wh-search-in">
        <span className="sr-only">Номер платежа или транзакции</span>
        <input
          type="search"
          className="field"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onBlur={() => {
            if (text.trim() !== data.filters.q.trim()) {
              data.setFilters({ q: text });
            }
          }}
          placeholder="Номер платежа или транзакции"
          maxLength={200}
          enterKeyHint="search"
        />
      </label>
    </form>
  );
}

function FilterFields({ data }: { data: Data }) {
  const { filters, setFilters } = data;
  const providers = data.meta?.providers ?? [];
  return (
    <>
      <label className="wh-select">
        <span className="sr-only">Платёжка</span>
        <select className="field" value={filters.provider} onChange={(e) => setFilters({ provider: e.target.value })}>
          <option value="">Все платёжки</option>
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.title}
            </option>
          ))}
          {filters.provider && !providers.some((p) => p.id === filters.provider) ? <option value={filters.provider}>{filters.provider}</option> : null}
        </select>
      </label>
      <label className="wh-select">
        <span className="sr-only">Итог</span>
        <select className="field" value={filters.outcome} onChange={(e) => setFilters({ outcome: e.target.value })}>
          {OUTCOMES.map(([key, label]) => (
            <option key={key} value={key}>
              {label}
            </option>
          ))}
        </select>
      </label>
      <label className="wh-select">
        <span className="sr-only">Период</span>
        <select className="field" value={filters.period} onChange={(e) => setFilters({ period: e.target.value as Period })}>
          {PERIODS.map(([key, label]) => (
            <option key={key} value={key}>
              {label}
            </option>
          ))}
        </select>
      </label>
      {filters.period === 'custom' ? (
        <span className="wh-dates">
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

function Subject({ item }: { item: JournalItem }) {
  const name = item.client_id ? personName(item.client, item.client_id) : null;
  if (!item.payment && !name) {
    return <span className="muted">{item.amount_text ?? '—'}</span>;
  }
  return (
    <>
      {item.payment ? (
        <Link to={`/admin/money?payment=${item.payment.id}`} className="wh-link">
          Платёж №{item.payment.id} · {item.payment.amount_text}
        </Link>
      ) : null}
      {name && item.client_id ? (
        <Link to={`/admin/clients/${item.client_id}`} className="pr-sub wh-sublink">
          {name}
        </Link>
      ) : null}
    </>
  );
}

function Empty({ data, phone }: { data: Data; phone?: boolean }) {
  const text = data.filtered ? 'Под эти условия уведомлений нет' : 'Уведомлений пока нет: они появятся, когда платёжки начнут присылать оплаты';
  return (
    <p className={phone ? 'hp-quiet hp-card' : 'muted pr-pad'}>
      {text}
      {data.filtered ? (
        <>
          {' '}
          <button type="button" className="wh-reset" onClick={() => data.setFilters(EMPTY)}>
            Сбросить фильтры
          </button>
        </>
      ) : null}
    </p>
  );
}

function JournalDesk({ data }: { data: Data }) {
  const items = data.items ?? [];
  const state = useBack();
  return (
    <div className="pr-page">
      <div className="pr-head">
        <h1 className="page-title tight">Деньги</h1>
      </div>
      <MoneyTabs />
      <p className="wh-lead">
        Всё, что платёжки присылали панели, и чем это закончилось.{data.meta ? ` ${retentionText(data.meta.retention)}` : ''}
      </p>
      <section className="card pr-list" aria-labelledby="wh-h">
        <div className="pr-top wh-top">
          <h2 id="wh-h" className="section-title">
            Уведомления платёжек
          </h2>
          <div className="wh-filters">
            <SearchField data={data} />
            <FilterFields data={data} />
          </div>
        </div>
        {data.error ? (
          <p className="err pr-pad" role="alert">
            {data.error}
          </p>
        ) : null}
        {data.items === null ? <p className="muted pr-pad">Загружаем журнал…</p> : null}
        {data.items !== null && items.length === 0 && !data.error ? <Empty data={data} /> : null}
        {items.length ? (
          <div role="table" aria-label="Уведомления платёжек">
            <div role="row" className="pt-row wh-row pr-th">
              <span role="columnheader">Когда</span>
              <span role="columnheader">Платёжка и событие</span>
              <span role="columnheader">Платёж и клиент</span>
              <span role="columnheader">Итог</span>
              <span role="columnheader">
                <span className="sr-only">Действия</span>
              </span>
            </div>
            {items.map((item) => {
              const repeats = deliveriesText(item.deliveries);
              return (
                <div key={item.id} role="row" className="pt-row wh-row">
                  <span role="cell" className="pr-cell">
                    <Link to={cardLink(item)} state={state} className="wh-when">
                      {dateTime(item.received_at)}
                    </Link>
                    {repeats ? <span className="pr-sub">{repeats}</span> : null}
                  </span>
                  <span role="cell" className="pr-cell">
                    <span className="pt-strong">{item.provider_title}</span>
                    <span className="pr-sub">{eventText(item.event)}</span>
                    {legacyText(item) ? <span className="pr-sub">{legacyText(item)}</span> : null}
                  </span>
                  <span role="cell" className="pr-cell">
                    <Subject item={item} />
                  </span>
                  <span role="cell" className="pr-cell">
                    <span className="wh-verdict" style={{ color: toneColor(item.tone) }}>
                      {item.title}
                    </span>
                    {item.note ? <span className="pr-sub">{item.note}</span> : null}
                  </span>
                  <span role="cell" className="pr-cell pr-act">
                    <Link to={cardLink(item)} state={state} className="b34 hov wh-open" aria-label={`Подробнее: уведомление №${item.id}`}>
                      Подробнее
                    </Link>
                  </span>
                </div>
              );
            })}
          </div>
        ) : null}
        {data.next !== null ? (
          <div className="pr-pad">
            <button type="button" className="b40 hov" onClick={() => void data.more()} disabled={data.busy}>
              {data.busy ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>
    </div>
  );
}

function JournalPhone({ data }: { data: Data }) {
  const items = data.items ?? [];
  const state = useBack();
  return (
    <div className="hp pt-phone">
      <PhoneHeader title="Деньги" />
      <MoneyTabs />
      {data.meta ? <p className="pt-lead">{retentionText(data.meta.retention)}</p> : null}
      <div className="wh-pfilters">
        <SearchField data={data} phone />
        <FilterFields data={data} />
      </div>
      {data.error ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{data.error}</span>
        </p>
      ) : null}
      {data.items === null ? <p className="hp-quiet hp-card">Загружаем журнал…</p> : null}
      {data.items !== null && items.length === 0 && !data.error ? <Empty data={data} phone /> : null}
      {items.length ? (
        <ul className="hp-card hp-list np-ul" aria-label="Уведомления платёжек">
          {items.map((item) => {
            const who = item.client_id ? personName(item.client, item.client_id) : null;
            const line = [dateTime(item.received_at), who, item.payment?.amount_text ?? item.amount_text].filter(Boolean).join(' · ');
            return (
              <li key={item.id} className="pt-pitem">
                <Link to={cardLink(item)} state={state} className="pt-plink">
                  <span className="hp-title">
                    {item.provider_title}
                    {item.event ? ` · ${eventText(item.event)}` : ''}
                  </span>
                  <span className="hp-note muted">{line}</span>
                  <span className="hp-note wh-pverdict" style={{ color: toneColor(item.tone) }}>
                    {verdictText(item)}
                  </span>
                </Link>
              </li>
            );
          })}
        </ul>
      ) : null}
      {data.next !== null ? (
        <button type="button" className="pr-padd press" onClick={() => void data.more()} disabled={data.busy}>
          {data.busy ? 'Загружаем…' : 'Показать ещё'}
        </button>
      ) : null}
    </div>
  );
}
