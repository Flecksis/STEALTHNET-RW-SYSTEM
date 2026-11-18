import { Fragment, useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import { dateTime } from '../../lib/format';
import { usePhone } from '../../lib/media';
import { Avatar } from '../Avatar';
import { personName } from '../home/model';
import { PhoneHeader } from '../phone';
import { contactLine } from '../referral/model';
import { useSession } from '../session';
import { ClientsTabs } from './ClientsTabs';
import {
  actionsText,
  AUDIENCES,
  emptyRule,
  exceedState,
  failText,
  isOff,
  LIMITS,
  limitsLead,
  PERIODS,
  RESTRICTIONS,
  ruleInput,
  SCOPES,
  usageText,
  windowNote,
  type Exceed,
  type LimitsSummary,
  type Page,
  type Rule,
  type RuleInput,
  type Squad,
} from './model';
import '../form.css';
import '../promo/promo.css';
import '../referral/referral.css';
import '../home/phone.css';
import './protect.css';

const PAGE = 30;

type ExceedPage = Page<Exceed> & { offset_minutes: number };

function useLimits() {
  const [summary, setSummary] = useState<LimitsSummary | null>(null);
  const [off, setOff] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [rules, setRules] = useState<Rule[] | null>(null);
  const [current, setCurrent] = useState<Exceed[] | null>(null);
  const [history, setHistory] = useState<Exceed[] | null>(null);
  const [historyNext, setHistoryNext] = useState<number | null>(null);
  const [more, setMore] = useState(false);
  const [squads, setSquads] = useState<Squad[] | null>(null);

  const load = useCallback(async () => {
    try {
      const [s, r, c, h] = await Promise.all([
        api<LimitsSummary>(`${LIMITS}/summary`),
        api<Rule[]>(`${LIMITS}/rules`),
        api<ExceedPage>(`${LIMITS}/exceeds?state=current&limit=100`),
        api<ExceedPage>(`${LIMITS}/exceeds?state=history&limit=${PAGE}`),
      ]);
      setSummary(s);
      setRules(r);
      setCurrent(c.items);
      setHistory(h.items);
      setHistoryNext(h.next);
      setOff(false);
      setError(null);
    } catch (err) {
      setOff(isOff(err));
      setError(failText(err, 'Лимиты трафика не загрузились'));
      setRules((x) => x ?? []);
      setCurrent((x) => x ?? []);
      setHistory((x) => x ?? []);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const loadSquads = useCallback(async () => {
    if (squads !== null) {
      return;
    }
    try {
      setSquads(await api<Squad[]>('/api/admin/tariffs/squads'));
    } catch {
      setSquads([]);
    }
  }, [squads]);

  const moreHistory = async () => {
    if (historyNext === null) {
      return;
    }
    setMore(true);
    try {
      const page = await api<ExceedPage>(`${LIMITS}/exceeds?state=history&limit=${PAGE}&after=${historyNext}`);
      setHistory((list) => [...(list ?? []), ...page.items]);
      setHistoryNext(page.next);
    } catch (err) {
      setError(failText(err, 'История не загрузилась'));
    } finally {
      setMore(false);
    }
  };

  const save = async (id: number | null, input: RuleInput) => {
    await api<Rule>(id === null ? `${LIMITS}/rules` : `${LIMITS}/rules/${id}`, { method: id === null ? 'POST' : 'PUT', body: input });
    await load();
  };

  const remove = async (id: number) => {
    await api(`${LIMITS}/rules/${id}`, { method: 'DELETE' });
    await load();
  };

  const lift = async (id: number) => {
    await api(`${LIMITS}/exceeds/${id}/lift`, { method: 'POST', body: {} });
    await load();
  };

  return { summary, off, error, rules, current, history, historyNext, more, moreHistory, save, remove, lift, squads, loadSquads };
}

type Data = ReturnType<typeof useLimits>;

export function Limits() {
  const phone = usePhone();
  const data = useLimits();
  return phone ? <LimitsPhone data={data} /> : <LimitsDesk data={data} />;
}

function Who({ id, person }: { id: number; person: Exceed['client'] }) {
  const name = personName(person, id);
  const line = contactLine(person);
  const contact = line === name ? null : line;
  return (
    <Link to={`/admin/clients/${id}`} className="rf-who">
      <Avatar id={id} name={name} className="rf-ava" />
      <span className="pr-cell">
        <span className="rf-name">{name}</span>
        {contact ? <span className="pr-sub">{contact}</span> : null}
      </span>
    </Link>
  );
}

function Segs<T extends string>({ label, options, value, onChange, disabled }: { label: string; options: Array<[T, string]>; value: T; onChange: (v: T) => void; disabled: boolean }) {
  return (
    <div role="group" aria-label={label} className="segs pt-segs">
      {options.map(([key, text]) => (
        <button key={key} type="button" aria-pressed={value === key} onClick={() => onChange(key)} disabled={disabled}>
          {text}
        </button>
      ))}
    </div>
  );
}

function RuleForm({ rule, data, onDone, phone }: { rule: Rule | null; data: Data; onDone: (text: string | null) => void; phone?: boolean }) {
  const [form, setForm] = useState<RuleInput>(() => (rule ? ruleInput(rule) : emptyRule()));
  const [gb, setGb] = useState(() => String(rule ? ruleInput(rule).limit_gb : emptyRule().limit_gb).replace('.', ','));
  const [devices, setDevices] = useState(() => (rule?.devices_min ? String(rule.devices_min) : ''));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const set = <K extends keyof RuleInput>(key: K, value: RuleInput[K]) => setForm((f) => ({ ...f, [key]: value }));
  const { loadSquads } = data;

  useEffect(() => {
    if (form.restriction === 'squad') {
      void loadSquads();
    }
  }, [form.restriction, loadSquads]);

  const save = async () => {
    const limit = Number(gb.trim().replace(',', '.'));
    if (!Number.isFinite(limit) || limit <= 0) {
      setError('Порог — число гигабайт больше нуля');
      return;
    }
    const min = devices.trim() ? Number(devices.trim()) : null;
    if (min !== null && (!Number.isInteger(min) || min < 1)) {
      setError('Устройств — целое число от 1 или пусто');
      return;
    }
    if (form.restriction === 'squad' && !form.squad) {
      setError('Выберите сквад, куда переводить');
      return;
    }
    if (!form.warn && !form.alert && form.restriction === 'none') {
      setError('Выберите хотя бы одно действие');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await data.save(rule?.id ?? null, { ...form, limit_gb: limit, devices_min: min, note: form.note?.trim() || null });
      onDone(rule ? 'Правило сохранено' : 'Правило добавлено, проверка начнётся в течение 10 минут');
    } catch (err) {
      setError(failText(err, 'Правило не сохранилось'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className={phone ? 'hp-card pt-form' : 'card pt-form'}
      aria-labelledby="lm-form-h"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <h2 id="lm-form-h" className="section-title">
        {rule ? 'Правило' : 'Новое правило'}
      </h2>
      <div className="pt-grid">
        <div className="pf-field">
          <span className="pf-label">Каким подпискам</span>
          <Segs label="Каким подпискам" options={AUDIENCES} value={form.audience} onChange={(v) => set('audience', v)} disabled={busy} />
        </div>
        <div className="pf-field">
          <span className="pf-label">Трафик по тарифу</span>
          <Segs label="Трафик по тарифу" options={SCOPES} value={form.traffic} onChange={(v) => set('traffic', v)} disabled={busy} />
        </div>
        <label className="pf-field">
          <span className="pf-label">Устройств в подписке, не меньше</span>
          <input className="field num" inputMode="numeric" value={devices} onChange={(e) => setDevices(e.target.value)} placeholder="Любое число" maxLength={4} disabled={busy} />
          <span className="cap">Например, отдельный порог для семейных тарифов</span>
        </label>
        <div className="pf-field">
          <span className="pf-label">Окно</span>
          <Segs label="Окно" options={PERIODS} value={form.period} onChange={(v) => set('period', v)} disabled={busy} />
        </div>
        <label className="pf-field">
          <span className="pf-label">Порог, ГБ</span>
          <input className="field num" inputMode="decimal" value={gb} onChange={(e) => setGb(e.target.value)} maxLength={9} disabled={busy} />
        </label>
        <label className="pf-check pt-check">
          <input type="checkbox" checked={form.per_device} onChange={(e) => set('per_device', e.target.checked)} disabled={busy} />
          <span>
            На каждое устройство
            <span className="cap">Порог умножается на число устройств в подписке</span>
          </span>
        </label>
      </div>
      <fieldset className="pt-modes">
        <legend className="pf-label">Что сделать при превышении</legend>
        <label className="pf-check">
          <input type="checkbox" checked={form.warn} onChange={(e) => set('warn', e.target.checked)} disabled={busy} />
          <span>
            Предупредить клиента
            <span className="cap">Сообщение в боте и письмо на почту, если она есть</span>
          </span>
        </label>
        <label className="pf-check">
          <input type="checkbox" checked={form.alert} onChange={(e) => set('alert', e.target.checked)} disabled={busy} />
          <span>
            Сообщить админам
            <span className="cap">В группу уведомлений Telegram, тема «Трафик»</span>
          </span>
        </label>
        {RESTRICTIONS.map(([key, title, hint]) => (
          <label key={key} className="pt-mode">
            <input type="radio" name="lm-restriction" checked={form.restriction === key} onChange={() => set('restriction', key)} disabled={busy} />
            <span>
              <span className="pt-mode-title">{title}</span>
              <span className="cap">{hint}</span>
            </span>
          </label>
        ))}
        {form.restriction === 'squad' ? (
          <label className="pf-field pt-squad">
            <span className="pf-label">Сквад на время ограничения</span>
            <select className="field" value={form.squad ?? ''} onChange={(e) => set('squad', e.target.value || null)} disabled={busy || data.squads === null}>
              <option value="">{data.squads === null ? 'Загружаем сквады…' : 'Выберите сквад'}</option>
              {(data.squads ?? []).map((s) => (
                <option key={s.uuid} value={s.uuid}>
                  {s.name}
                </option>
              ))}
            </select>
            {data.squads !== null && data.squads.length === 0 ? <span className="cap">Сквады не загрузились: нужно право «Тарифы» и подключённый Remnawave</span> : null}
          </label>
        ) : null}
      </fieldset>
      <label className="pf-field">
        <span className="pf-label">Заметка</span>
        <input className="field" value={form.note ?? ''} onChange={(e) => set('note', e.target.value)} maxLength={200} placeholder="Видят только администраторы" disabled={busy} />
      </label>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="pf-actions">
        <button type="submit" className="btn-main btn" disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" onClick={() => onDone(null)} disabled={busy}>
          Отменить
        </button>
      </div>
    </form>
  );
}

function Off({ phone }: { phone?: boolean }) {
  const text = (
    <>
      <span className="muted">Модуль «Лимиты трафика» выключен. </span>
      <Link to="/admin/modules/limits" className="pr-inline-link">
        Включить модуль
      </Link>
    </>
  );
  return phone ? (
    <p className="hp-quiet hp-card">{text}</p>
  ) : (
    <section className="card card-pad">
      <p>{text}</p>
    </section>
  );
}

type Editing = { rule: Rule | null } | null;

function LimitsDesk({ data }: { data: Data }) {
  const { can } = useSession();
  const [editing, setEditing] = useState<Editing>(null);
  const [note, setNote] = useState<string | null>(null);
  const [liftNote, setLiftNote] = useState<string | null>(null);
  const [asking, setAsking] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [askError, setAskError] = useState<string | null>(null);
  const editable = can('modules');
  const writable = can('clients_write');
  const squads = data.squads ?? [];
  const { loadSquads } = data;

  useEffect(() => {
    if ((data.rules ?? []).some((r) => r.restriction === 'squad')) {
      void loadSquads();
    }
  }, [data.rules, loadSquads]);

  const run = async (work: () => Promise<void>, done: () => void) => {
    setBusy(true);
    setAskError(null);
    try {
      await work();
      setAsking(null);
      done();
    } catch (err) {
      setAskError(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const rules = data.rules ?? [];
  const current = data.current ?? [];
  const history = data.history ?? [];

  const exceedRow = (e: Exceed, actions: boolean) => {
    const state = exceedState(e);
    const key = `lift${e.id}`;
    return (
      <Fragment key={e.id}>
        <div role="row" className="pt-row pt-erow">
          <span role="cell" className="pr-cell">
            <Who id={e.client_id} person={e.client} />
          </span>
          <span role="cell" className="pr-cell">
            <span className="pt-strong">{usageText(e)}</span>
            <span className="pr-sub">{[e.tariff, e.rule].filter(Boolean).join(' · ')}</span>
          </span>
          <span role="cell" className="pr-cell">
            <span className={`pr-state ${state.tone}`}>{state.text}</span>
            {state.sub ? <span className="pr-sub">{state.sub}</span> : null}
          </span>
          <span role="cell" className="pr-cell">
            {dateTime(e.exceeded_at)}
          </span>
          <span role="cell" className="pr-cell pr-act">
            {actions && state.active && writable ? (
              <button type="button" className="b34 hov" aria-expanded={asking === key} onClick={() => setAsking(asking === key ? null : key)}>
                Снять ограничение
              </button>
            ) : null}
          </span>
        </div>
        {asking === key ? (
          <div role="row" className="pr-ask-row">
            <div role="cell" className="pr-ask">
              <span>
                <b>Снять ограничение сейчас?</b> Подписка клиента {personName(e.client, e.client_id)} снова заработает, не дожидаясь конца окна.
              </span>
              <span className="pr-ask-btns">
                <button type="button" className="b34 on-surface hov" disabled={busy} onClick={() => void run(() => data.lift(e.id), () => setLiftNote('Ограничение снято, клиент получит сообщение'))}>
                  {busy ? 'Снимаем…' : 'Снять'}
                </button>
                <button type="button" className="b34 on-surface hov" disabled={busy} onClick={() => setAsking(null)}>
                  Отменить
                </button>
              </span>
              {askError ? (
                <span className="err rf-ask-err" role="alert">
                  {askError}
                </span>
              ) : null}
            </div>
          </div>
        ) : null}
      </Fragment>
    );
  };

  return (
    <div className="pr-page">
      <div className="pr-head">
        <h1 className="page-title tight">Лимиты трафика</h1>
        {editable && !data.off ? (
          <span className="pr-head-acts">
            <button type="button" className="btn-main btn" onClick={() => setEditing({ rule: null })} disabled={editing !== null}>
              Новое правило
            </button>
          </span>
        ) : null}
      </div>
      <ClientsTabs />
      {data.off ? <Off /> : null}
      {data.summary ? <p className="rf-lead">{limitsLead(data.summary)}</p> : null}
      {data.error && !data.off ? (
        <p className="rf-lead" role="alert">
          <span className="err rf-err">{data.error}</span>
        </p>
      ) : null}
      {editing ? (
        <RuleForm
          rule={editing.rule}
          data={data}
          onDone={(text) => {
            setEditing(null);
            setNote(text);
          }}
        />
      ) : null}

      {data.off ? null : (
        <section className="card pr-list" aria-labelledby="lm-r-h">
          <div className="pr-top">
            <h2 id="lm-r-h" className="section-title">
              Правила
            </h2>
            <span className="muted">Группы тарифов задаются свойствами подписки, а не названиями</span>
          </div>
          {note ? (
            <p className="pr-note" role="status">
              {note}
            </p>
          ) : null}
          {data.rules === null ? <p className="muted pr-pad">Загружаем правила…</p> : null}
          {data.rules !== null && rules.length === 0 ? <p className="muted pr-pad">Правил нет. Например: подписки без лимита трафика, 100 ГБ за сутки — предупредить клиента и сообщить админам.</p> : null}
          {rules.length ? (
            <div role="table" aria-label="Правила">
              <div role="row" className="pt-row pt-rrow pr-th">
                <span role="columnheader">Порог</span>
                <span role="columnheader">Что делаем</span>
                <span role="columnheader">Состояние</span>
                <span role="columnheader">
                  <span className="sr-only">Действия</span>
                </span>
              </div>
              {rules.map((r) => {
                const key = `del${r.id}`;
                return (
                  <Fragment key={r.id}>
                    <div role="row" className={r.enabled ? 'pt-row pt-rrow' : 'pt-row pt-rrow pr-dim'}>
                      <span role="cell" className="pr-cell">
                        <span className="pt-strong">{r.description}</span>
                        {r.note ? <span className="pr-sub">{r.note}</span> : null}
                      </span>
                      <span role="cell" className="pr-cell">
                        {actionsText(r, squads)}
                      </span>
                      <span role="cell" className="pr-cell">
                        <span className={`pr-state ${r.enabled ? 'ink' : 'muted'}`}>{r.enabled ? 'Работает' : 'Выключено'}</span>
                      </span>
                      <span role="cell" className="pr-cell pr-act pt-acts">
                        {editable ? (
                          <>
                            <button type="button" className="b34 hov" onClick={() => setEditing({ rule: r })} disabled={editing !== null}>
                              Изменить
                            </button>
                            <button
                              type="button"
                              className="b34 hov"
                              disabled={busy}
                              onClick={() => void run(() => data.save(r.id, { ...ruleInput(r), enabled: !r.enabled }), () => setNote(r.enabled ? 'Правило выключено' : 'Правило включено'))}
                            >
                              {r.enabled ? 'Выключить' : 'Включить'}
                            </button>
                            <button type="button" className="b34 hov" aria-expanded={asking === key} onClick={() => setAsking(asking === key ? null : key)}>
                              Удалить
                            </button>
                          </>
                        ) : null}
                      </span>
                    </div>
                    {asking === key ? (
                      <div role="row" className="pr-ask-row">
                        <div role="cell" className="pr-ask">
                          <span>
                            <b>Удалить правило?</b> Новые превышения по нему не появятся. Уже наложенные ограничения снимутся в свой срок, история сохранится.
                          </span>
                          <span className="pr-ask-btns">
                            <button type="button" className="b34 on-surface hov rf-bad" disabled={busy} onClick={() => void run(() => data.remove(r.id), () => setNote('Правило удалено'))}>
                              {busy ? 'Удаляем…' : 'Удалить'}
                            </button>
                            <button type="button" className="b34 on-surface hov" disabled={busy} onClick={() => setAsking(null)}>
                              Отменить
                            </button>
                          </span>
                          {askError ? (
                            <span className="err rf-ask-err" role="alert">
                              {askError}
                            </span>
                          ) : null}
                        </div>
                      </div>
                    ) : null}
                  </Fragment>
                );
              })}
            </div>
          ) : null}
        </section>
      )}

      {data.off ? null : (
        <section className="card pr-list" aria-labelledby="lm-c-h">
          <div className="pr-top">
            <h2 id="lm-c-h" className="section-title">
              Превысили сейчас
            </h2>
            <span className="muted">{windowNote(data.summary)}</span>
          </div>
          {liftNote ? (
            <p className="pr-note" role="status">
              {liftNote}
            </p>
          ) : null}
          {data.current === null ? <p className="muted pr-pad">Загружаем…</p> : null}
          {data.current !== null && current.length === 0 ? <p className="muted pr-pad">Сейчас никто не превысил порог</p> : null}
          {current.length ? (
            <div role="table" aria-label="Превысили сейчас">
              <div role="row" className="pt-row pt-erow pr-th">
                <span role="columnheader">Клиент</span>
                <span role="columnheader">Сколько скачал</span>
                <span role="columnheader">Что сделано</span>
                <span role="columnheader">Превысил</span>
                <span role="columnheader">
                  <span className="sr-only">Действия</span>
                </span>
              </div>
              {current.map((e) => exceedRow(e, true))}
            </div>
          ) : null}
        </section>
      )}

      {data.off ? null : (
        <section className="card pr-list" aria-labelledby="lm-h-h">
          <div className="pr-top">
            <h2 id="lm-h-h" className="section-title">
              История
            </h2>
          </div>
          {data.history !== null && history.length === 0 ? <p className="muted pr-pad">Превышений ещё не было</p> : null}
          {history.length ? (
            <div role="table" aria-label="История превышений">
              <div role="row" className="pt-row pt-erow pr-th">
                <span role="columnheader">Клиент</span>
                <span role="columnheader">Сколько скачал</span>
                <span role="columnheader">Что сделано</span>
                <span role="columnheader">Превысил</span>
                <span role="columnheader">
                  <span className="sr-only">Действия</span>
                </span>
              </div>
              {history.map((e) => exceedRow(e, false))}
            </div>
          ) : null}
          {data.historyNext !== null ? (
            <div className="pr-pad">
              <button type="button" className="b40 hov" onClick={() => void data.moreHistory()} disabled={data.more}>
                {data.more ? 'Загружаем…' : 'Показать ещё'}
              </button>
            </div>
          ) : null}
        </section>
      )}
    </div>
  );
}

function LimitsPhone({ data }: { data: Data }) {
  const { can } = useSession();
  const [editing, setEditing] = useState<Editing>(null);
  const [asking, setAsking] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const editable = can('modules');
  const writable = can('clients_write');
  const squads = data.squads ?? [];

  const run = async (work: () => Promise<void>, text: string) => {
    setBusy(true);
    try {
      await work();
      setAsking(null);
      setToast(text);
    } catch (err) {
      setToast(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const exceedItem = (e: Exceed, actions: boolean) => {
    const state = exceedState(e);
    const key = `lift${e.id}`;
    return (
      <li key={e.id} className="pt-pitem">
        <Link to={`/admin/clients/${e.client_id}`} className="pt-plink">
          <span className="hp-title">{personName(e.client, e.client_id)}</span>
          <span className="hp-note">{usageText(e)}</span>
          <span className={`hp-note pr-state ${state.tone}`}>{state.text}</span>
          <span className="hp-note muted">{dateTime(e.exceeded_at)}</span>
        </Link>
        {actions && state.active && writable ? (
          asking === key ? (
            <div className="np-ask">
              <p>
                <strong>Снять ограничение сейчас?</strong> Подписка снова заработает.
              </p>
              <div className="np-ask-btns">
                <button type="button" className="np-ok press" disabled={busy} onClick={() => void run(() => data.lift(e.id), 'Ограничение снято')}>
                  {busy ? 'Снимаем…' : 'Снять'}
                </button>
                <button type="button" className="np-cancel press" onClick={() => setAsking(null)}>
                  Отменить
                </button>
              </div>
            </div>
          ) : (
            <button type="button" className="hp-pill pt-pbtn" onClick={() => setAsking(key)}>
              <span>Снять ограничение</span>
            </button>
          )
        ) : null}
      </li>
    );
  };

  const rules = data.rules ?? [];
  const current = data.current ?? [];
  const history = data.history ?? [];

  return (
    <div className="hp pt-phone">
      <PhoneHeader title="Лимиты трафика" back={{ to: '/admin/clients', label: 'Все клиенты' }} />
      <ClientsTabs />
      {data.off ? <Off phone /> : null}
      {data.summary ? <p className="pt-lead">{limitsLead(data.summary)}</p> : null}
      {data.error && !data.off ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{data.error}</span>
        </p>
      ) : null}
      {editing ? (
        <RuleForm
          phone
          rule={editing.rule}
          data={data}
          onDone={(text) => {
            setEditing(null);
            if (text) {
              setToast(text);
            }
          }}
        />
      ) : editable && !data.off ? (
        <button type="button" className="pr-padd press" onClick={() => setEditing({ rule: null })}>
          Новое правило
        </button>
      ) : null}

      {data.off ? null : (
        <section className="hp-group" aria-labelledby="lm-pr-h">
          <h2 id="lm-pr-h" className="hp-h2">
            Правила
          </h2>
          {data.rules !== null && rules.length === 0 ? <p className="hp-quiet hp-card">Правил нет: трафик не проверяется</p> : null}
          {rules.length ? (
            <ul className="hp-card hp-list np-ul" aria-label="Правила">
              {rules.map((r) => (
                <li key={r.id} className={r.enabled ? 'pt-pitem' : 'pt-pitem pr-dim'}>
                  <span className="pt-plink">
                    <span className="hp-title">{r.description}</span>
                    <span className="hp-note muted">{actionsText(r, squads)}</span>
                    {!r.enabled ? <span className="hp-note muted">Выключено</span> : null}
                  </span>
                  {editable ? (
                    <span className="pt-pbtns">
                      <button type="button" className="hp-pill" onClick={() => setEditing({ rule: r })}>
                        <span>Изменить</span>
                      </button>
                      <button type="button" className="hp-pill" disabled={busy} onClick={() => void run(() => data.save(r.id, { ...ruleInput(r), enabled: !r.enabled }), r.enabled ? 'Правило выключено' : 'Правило включено')}>
                        <span>{r.enabled ? 'Выключить' : 'Включить'}</span>
                      </button>
                    </span>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
        </section>
      )}

      {data.off ? null : (
        <section className="hp-group" aria-labelledby="lm-pc-h">
          <h2 id="lm-pc-h" className="hp-h2">
            Превысили сейчас
          </h2>
          {data.current !== null && current.length === 0 ? <p className="hp-quiet hp-card">Сейчас никто не превысил порог</p> : null}
          {current.length ? (
            <ul className="hp-card hp-list np-ul" aria-label="Превысили сейчас">
              {current.map((e) => exceedItem(e, true))}
            </ul>
          ) : null}
          <p className="hp-quiet">{windowNote(data.summary)}</p>
        </section>
      )}

      {data.off ? null : (
        <section className="hp-group" aria-labelledby="lm-ph-h">
          <h2 id="lm-ph-h" className="hp-h2">
            История
          </h2>
          {data.history !== null && history.length === 0 ? <p className="hp-quiet hp-card">Превышений ещё не было</p> : null}
          {history.length ? (
            <ul className="hp-card hp-list np-ul" aria-label="История превышений">
              {history.map((e) => exceedItem(e, false))}
            </ul>
          ) : null}
          {data.historyNext !== null ? (
            <button type="button" className="pr-padd press" onClick={() => void data.moreHistory()} disabled={data.more}>
              {data.more ? 'Загружаем…' : 'Показать ещё'}
            </button>
          ) : null}
        </section>
      )}

      {toast ? (
        <div className="np-toast-wrap" role="status">
          <p className="np-toast">{toast}</p>
        </div>
      ) : null}
    </div>
  );
}
