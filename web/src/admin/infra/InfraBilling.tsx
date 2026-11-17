import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { count, dateLong, dayLabel, money, type Money } from '../../lib/format';
import { Flag } from '../../lib/Flag';
import { useSession } from '../session';
import { countryName } from '../home/model';
import { fromRaw, toRaw } from '../tariffs/model';
import { InfraHead } from './InfraHead';
import '../form.css';
import './infra.css';

type Provider = { uuid: string; name: string; login_url: string | null };
type Row = { uuid: string; node_uuid: string | null; node_name: string; country_code: string; city: string | null; provider: Provider; next_billing_at: string; price: Money | null };
type Payment = { provider: string; nodes: string[]; amount: Money; billed_at: string };
type View = {
  rows: Row[];
  providers: Array<{ provider: Provider; nodes: number; monthly: Money }>;
  history: Payment[];
  history_total: number;
  month_total: Money;
  month_paid: Money;
  spent_total: Money;
  revenue_30d: Money | null;
};

const MONTHS = ['Январь', 'Февраль', 'Март', 'Апрель', 'Май', 'Июнь', 'Июль', 'Август', 'Сентябрь', 'Октябрь', 'Ноябрь', 'Декабрь'];

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function daysUntil(iso: string): number {
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  return Math.round((start(new Date(iso)) - start(new Date())) / 86_400_000);
}

function nextMonth(iso: string): string {
  const date = new Date(iso);
  date.setMonth(date.getMonth() + 1);
  return date.toISOString();
}

export function InfraBilling() {
  const { me } = useSession();
  const currency = me.currency;
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ask, setAsk] = useState<string | null>(null);
  const [pricing, setPricing] = useState<string | null>(null);
  const [priceRaw, setPriceRaw] = useState('');
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [providerForm, setProviderForm] = useState<{ uuid: string | null; name: string; url: string } | null>(null);
  const [more, setMore] = useState<Payment[]>([]);
  const [moreBusy, setMoreBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setView(await api<View>('/api/admin/infra/billing/view'));
      setMore([]);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Оплаты серверов не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const markPaid = async (row: Row) => {
    setBusy(true);
    setNote(null);
    try {
      await api(`/api/admin/infra/billing/${row.uuid}/paid`, { method: 'POST', body: {} });
      setAsk(null);
      setNote(`Оплата ${row.node_name} отмечена, следующая — ${dateLong(nextMonth(row.next_billing_at))}`);
      await load();
    } catch (err) {
      setNote(failText(err, 'Оплата не отметилась'));
    } finally {
      setBusy(false);
    }
  };

  const savePrice = async (row: Row) => {
    if (!row.node_uuid) {
      return;
    }
    const minor = fromRaw(priceRaw, currency);
    if (minor === 'bad') {
      setNote(currency === 'RUB' ? 'Цена: целое число рублей' : 'Цена: число, до двух знаков после точки');
      return;
    }
    setBusy(true);
    try {
      await api(`/api/admin/infra/prices/${row.node_uuid}`, { method: 'PUT', body: { amount_minor: minor } });
      setPricing(null);
      await load();
    } catch (err) {
      setNote(failText(err, 'Цена не сохранилась'));
    } finally {
      setBusy(false);
    }
  };

  const saveProvider = async () => {
    if (!providerForm) {
      return;
    }
    setBusy(true);
    setNote(null);
    try {
      const body = { name: providerForm.name.trim(), login_url: providerForm.url.trim() || null };
      if (providerForm.uuid) {
        await api(`/api/admin/infra/providers/${providerForm.uuid}`, { method: 'PUT', body });
      } else {
        await api('/api/admin/infra/providers', { method: 'POST', body });
      }
      setProviderForm(null);
      await load();
    } catch (err) {
      setNote(failText(err, 'Провайдер не сохранился'));
    } finally {
      setBusy(false);
    }
  };

  const loadMore = async () => {
    if (!view) {
      return;
    }
    setMoreBusy(true);
    try {
      const page = await api<{ records: Payment[]; total: number }>(`/api/admin/infra/billing/history?start=${view.history.length + more.length}&size=20`);
      setMore([...more, ...page.records]);
    } catch (err) {
      setNote(failText(err, 'История не загрузилась'));
    } finally {
      setMoreBusy(false);
    }
  };

  const history = view ? [...view.history, ...more] : [];
  const month = MONTHS[new Date().getMonth()] ?? '';
  const share =
    view?.revenue_30d && view.revenue_30d.minor > 0 && view.month_total.minor > 0
      ? ((view.month_total.minor / view.revenue_30d.minor) * 100).toFixed(1).replace('.', ',')
      : null;

  return (
    <div className="inf">
      <InfraHead current="billing" />
      {error && !view ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {view ? (
        <div className="inf-col" style={{ gap: 4 }}>
          <p className="inf-summary">
            {month}: {money(view.month_total)} за {count(view.rows.length, 'ноду', 'ноды', 'нод')},{' '}
            {view.month_paid.minor >= view.month_total.minor && view.month_total.minor > 0 ? 'всё оплачено' : `оплачено ${money(view.month_paid)}`}
          </p>
          {share ? <span className="muted">Серверы — {share}% выручки за 30 дней</span> : null}
          {view.rows.some((r) => !r.price) ? <span className="cap">Укажите цену сервера у каждой ноды — тогда суммы за месяц будут точными</span> : null}
        </div>
      ) : null}
      {note ? (
        <p role="status" className="cap" style={{ color: 'var(--ink-2)' }}>
          {note}
        </p>
      ) : null}

      <section className="inf-card" aria-labelledby="bill-h">
        <h2 id="bill-h" className="hm-h2b" style={{ padding: '18px 20px 12px' }}>
          Ближайшие оплаты
        </h2>
        <div className="inf-scroll">
          <div role="table" aria-label="Ближайшие оплаты" className="bl-table">
            <div role="row" className="bl-row head">
              <span role="columnheader">Нода</span>
              <span role="columnheader">Провайдер</span>
              <span role="columnheader" className="right">
                В месяц
              </span>
              <span role="columnheader">Следующая оплата</span>
              <span role="columnheader">
                <span className="sr-only">Действия</span>
              </span>
            </div>
            {view && view.rows.length === 0 ? <div className="inf-empty">Оплат пока нет: укажите провайдера и дату оплаты при добавлении ноды</div> : null}
            {view?.rows.map((row) => {
              const left = daysUntil(row.next_billing_at);
              const soon = left <= 7;
              return (
                <div key={row.uuid} className="bl-group">
                  <div role="row" className="bl-row">
                    <span role="cell" className="inf-col" style={{ minWidth: 0 }}>
                      <strong>
                        <Flag code={row.country_code} />
                        {row.node_name}
                      </strong>
                      <span className="inf-sub sp-ell">{[row.city, countryName(row.country_code)].filter(Boolean).join(', ')}</span>
                    </span>
                    <span role="cell">{row.provider.name}</span>
                    <span role="cell" className="num right" style={{ fontWeight: 600 }}>
                      {pricing === row.uuid ? (
                        <span className="unit-field" style={{ justifyContent: 'flex-end' }}>
                          {currency === 'USD' ? <span className="muted">$</span> : null}
                          <input
                            className="field num"
                            style={{ width: 90, height: 34 }}
                            value={priceRaw}
                            onChange={(e) => setPriceRaw(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === 'Enter') {
                                void savePrice(row);
                              }
                              if (e.key === 'Escape') {
                                setPricing(null);
                              }
                            }}
                            aria-label={`Цена сервера ${row.node_name} в месяц`}
                            autoFocus
                            disabled={busy}
                          />
                          {currency === 'RUB' ? <span className="muted">₽</span> : null}
                        </span>
                      ) : (
                        <button
                          type="button"
                          className="bl-price"
                          onClick={() => {
                            setPricing(row.uuid);
                            setPriceRaw(toRaw(row.price?.minor ?? null, currency));
                          }}
                          disabled={!row.node_uuid}
                          title="Изменить цену"
                        >
                          {row.price ? money(row.price) : 'указать'}
                        </button>
                      )}
                    </span>
                    <span role="cell" className="inf-col">
                      <span style={{ color: soon ? 'var(--warn)' : 'var(--ink)', fontWeight: soon ? 600 : 500 }}>
                        {dateLong(row.next_billing_at)}
                        {soon ? ` · ${left <= 0 ? (left < 0 ? 'просрочено' : 'сегодня') : `через ${count(left, 'день', 'дня', 'дней')}`}` : ''}
                      </span>
                    </span>
                    <span role="cell" className="bl-actions">
                      <button
                        type="button"
                        className="b34 hov"
                        aria-expanded={ask === row.uuid}
                        aria-label={`Отметить оплату ${row.node_name}`}
                        onClick={() => setAsk(ask === row.uuid ? null : row.uuid)}
                        style={ask === row.uuid ? { background: 'var(--surface-2)' } : undefined}
                        disabled={busy}
                      >
                        Отметить оплату
                      </button>
                      {row.provider.login_url ? (
                        <a href={row.provider.login_url} target="_blank" rel="noreferrer" className="hm-link" aria-label={`Кабинет провайдера ${row.provider.name}`}>
                          Кабинет провайдера
                        </a>
                      ) : null}
                    </span>
                  </div>
                  {ask === row.uuid ? (
                    <div role="group" aria-label={`Подтвердить оплату ${row.node_name}`} style={{ padding: '0 20px 12px' }}>
                      <div className="cc-note">
                        <span className="cc-note-text wide">
                          <strong>Отметить оплату {row.node_name}:</strong> {row.price ? money(row.price) : 'сумма не задана'} сегодня, {dateLong(new Date().toISOString())}. Следующая — {dateLong(nextMonth(row.next_billing_at))}.
                        </span>
                        <span className="cc-btns">
                          <button type="button" className="b34 on-surface" onClick={() => void markPaid(row)} disabled={busy || !row.price}>
                            Отметить
                          </button>
                          <button type="button" className="b34 on-surface" onClick={() => setAsk(null)} disabled={busy}>
                            Отменить
                          </button>
                        </span>
                      </div>
                      {!row.price ? <p className="cap" style={{ paddingTop: 6 }}>Сначала укажите цену сервера в колонке «В месяц»</p> : null}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        </div>
      </section>

      <div className="hm-cols" style={{ alignItems: 'stretch' }}>
        <section className="inf-card bl-box" aria-labelledby="prov-h">
          <h2 id="prov-h" className="hm-h2b" style={{ marginBottom: 10 }}>
            Провайдеры
          </h2>
          {view?.providers.map((p) =>
            providerForm?.uuid === p.provider.uuid ? (
              <ProviderForm key={p.provider.uuid} form={providerForm} setForm={setProviderForm} busy={busy} onSave={() => void saveProvider()} />
            ) : (
              <div key={p.provider.uuid} className="bl-prov">
                <span style={{ flex: 1, minWidth: 0 }}>
                  <strong>{p.provider.name}</strong>
                  <span style={{ color: 'var(--ink-2)' }}>
                    {' '}
                    · {count(p.nodes, 'нода', 'ноды', 'нод')}
                    {p.monthly.minor > 0 ? ` · ${money(p.monthly)} в месяц` : ''}
                  </span>
                </span>
                <button
                  type="button"
                  className="b34 hov"
                  aria-label={`Изменить провайдера ${p.provider.name}`}
                  onClick={() => setProviderForm({ uuid: p.provider.uuid, name: p.provider.name, url: p.provider.login_url ?? '' })}
                >
                  Изменить
                </button>
              </div>
            ),
          )}
          {providerForm && providerForm.uuid === null ? <ProviderForm form={providerForm} setForm={setProviderForm} busy={busy} onSave={() => void saveProvider()} /> : null}
          <button type="button" className="bl-add" onClick={() => setProviderForm({ uuid: null, name: '', url: '' })} disabled={busy}>
            Добавить провайдера
          </button>
        </section>
        <section className="inf-card bl-box" aria-labelledby="hist-h">
          <div className="hm-panehead" style={{ padding: 0, marginBottom: 10 }}>
            <h2 id="hist-h" className="hm-h2b">
              История
            </h2>
            {view ? <span className="muted">С начала учёта {money(view.spent_total)}</span> : null}
          </div>
          {view && history.length === 0 ? <p className="inf-empty" style={{ padding: '11px 0' }}>Оплат ещё не отмечали</p> : null}
          {history.map((h, i) => (
            <div key={`${h.billed_at}-${i}`} className="bl-hist">
              <span className="sp-small muted">{dayLabel(h.billed_at)}</span>
              <span style={{ fontWeight: 600 }}>{[h.provider || 'Провайдер', h.nodes.join(', ')].filter(Boolean).join(' · ')}</span>
              <span className="num" style={{ fontWeight: 700 }}>
                {money(h.amount)}
              </span>
            </div>
          ))}
          {view && view.history_total > history.length ? (
            <button type="button" className="bl-add" onClick={() => void loadMore()} disabled={moreBusy}>
              {moreBusy ? 'Загружаем…' : history.length > view.history.length ? 'Показать ещё' : 'Вся история'}
            </button>
          ) : null}
        </section>
      </div>
    </div>
  );
}

type FormState = { uuid: string | null; name: string; url: string };

function ProviderForm({ form, setForm, busy, onSave }: { form: FormState; setForm: (f: FormState | null) => void; busy: boolean; onSave: () => void }) {
  return (
    <div className="bl-provform">
      <input className="field" value={form.name} maxLength={60} placeholder="Название, например Hetzner" onChange={(e) => setForm({ ...form, name: e.target.value })} disabled={busy} autoFocus />
      <input className="field" value={form.url} maxLength={300} placeholder="Ссылка на кабинет, https://…" onChange={(e) => setForm({ ...form, url: e.target.value })} disabled={busy} />
      <span className="cc-btns">
        <button type="button" className="b34 hov" onClick={onSave} disabled={busy || !form.name.trim()}>
          {form.uuid ? 'Сохранить' : 'Добавить'}
        </button>
        <button type="button" className="b34 hov" onClick={() => setForm(null)} disabled={busy}>
          Отменить
        </button>
      </span>
    </div>
  );
}
