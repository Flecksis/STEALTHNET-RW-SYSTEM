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
import { Switch } from '../ui';
import { ClientsTabs } from './ClientsTabs';
import {
  ANTIBOT,
  antibotLead,
  failText,
  isOff,
  JOURNAL_FILTERS,
  journalNote,
  markState,
  MODES,
  reasonsText,
  type AntibotSummary,
  type JournalItem,
  type MarkItem,
  type Mode,
  type Page,
} from './model';
import '../form.css';
import '../promo/promo.css';
import '../referral/referral.css';
import '../home/phone.css';
import './protect.css';

const PAGE = 30;

function useAntibot() {
  const [summary, setSummary] = useState<AntibotSummary | null>(null);
  const [off, setOff] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [marks, setMarks] = useState<MarkItem[] | null>(null);
  const [marksNext, setMarksNext] = useState<number | null>(null);
  const [journal, setJournal] = useState<JournalItem[] | null>(null);
  const [journalNext, setJournalNext] = useState<number | null>(null);
  const [filter, setFilterState] = useState('all');
  const [loading, setLoading] = useState<'marks' | 'journal' | null>(null);
  const [busy, setBusy] = useState<number | null>(null);

  const loadSummary = useCallback(async () => {
    try {
      setSummary(await api<AntibotSummary>(`${ANTIBOT}/summary`));
      setOff(false);
    } catch (err) {
      setOff(isOff(err));
      setError(failText(err, 'Антибот не загрузился'));
    }
  }, []);

  const loadMarks = useCallback(async () => {
    try {
      const page = await api<Page<MarkItem>>(`${ANTIBOT}/marks?limit=${PAGE}`);
      setMarks(page.items);
      setMarksNext(page.next);
    } catch (err) {
      setMarks([]);
      setError(failText(err, 'Отмеченные клиенты не загрузились'));
    }
  }, []);

  const loadJournal = useCallback(async (kind: string) => {
    try {
      const page = await api<Page<JournalItem>>(`${ANTIBOT}/journal?kind=${kind}&limit=${PAGE}`);
      setJournal(page.items);
      setJournalNext(page.next);
    } catch (err) {
      setJournal([]);
      setError(failText(err, 'Журнал не загрузился'));
    }
  }, []);

  useEffect(() => {
    void loadSummary();
    void loadMarks();
    void loadJournal('all');
  }, [loadSummary, loadMarks, loadJournal]);

  const setFilter = (kind: string) => {
    setFilterState(kind);
    setJournal(null);
    void loadJournal(kind);
  };

  const more = async (what: 'marks' | 'journal') => {
    setLoading(what);
    try {
      if (what === 'marks' && marksNext !== null) {
        const page = await api<Page<MarkItem>>(`${ANTIBOT}/marks?limit=${PAGE}&after=${marksNext}`);
        setMarks((list) => [...(list ?? []), ...page.items]);
        setMarksNext(page.next);
      }
      if (what === 'journal' && journalNext !== null) {
        const page = await api<Page<JournalItem>>(`${ANTIBOT}/journal?kind=${filter}&limit=${PAGE}&after=${journalNext}`);
        setJournal((list) => [...(list ?? []), ...page.items]);
        setJournalNext(page.next);
      }
    } catch (err) {
      setError(failText(err, 'Не загрузилось'));
    } finally {
      setLoading(null);
    }
  };

  const clear = async (m: MarkItem) => {
    setBusy(m.client_id);
    try {
      await api(`${ANTIBOT}/marks/${m.client_id}/clear`, { method: 'POST', body: {} });
      setMarks((list) => (list ?? []).filter((x) => x.client_id !== m.client_id));
      void loadSummary();
      void loadJournal(filter);
    } finally {
      setBusy(null);
    }
  };

  return { summary, off, error, marks, marksNext, journal, journalNext, filter, setFilter, loading, more, clear, busy, reload: loadSummary };
}

type Data = ReturnType<typeof useAntibot>;

export function Antibot() {
  const phone = usePhone();
  const data = useAntibot();
  return phone ? <AntibotPhone data={data} /> : <AntibotDesk data={data} />;
}

function Who({ id, person }: { id: number; person: MarkItem['client'] }) {
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

type FormState = {
  mode: Mode;
  young_days: string;
  check_username: boolean;
  burst_count: string;
  ip_limit: string;
  extra_domains: string;
  referral_guard: boolean;
  mass_invites: string;
  roulette_guard: boolean;
  contest_guard: boolean;
};

function formOf(summary: AntibotSummary): FormState {
  const s = summary.settings;
  return {
    mode: s.mode,
    young_days: String(s.young_days),
    check_username: s.check_username,
    burst_count: String(s.burst_count),
    ip_limit: String(s.ip_limit),
    extra_domains: s.extra_domains.join(', '),
    referral_guard: s.referral_guard,
    mass_invites: String(s.mass_invites),
    roulette_guard: s.roulette_guard,
    contest_guard: s.contest_guard,
  };
}

const GUARDS: Array<['roulette_guard' | 'contest_guard', string, string]> = [
  ['roulette_guard', 'Защищать рулетку', 'Бесплатные вращения подозрительным — по выбранному режиму. Вращения за покупки не трогаются.'],
  [
    'contest_guard',
    'Защищать конкурсы',
    'Участие подозрительных и зачёт приглашённых друзей — по выбранному режиму. Самоприглашения и массовые приглашения не засчитываются никогда.',
  ],
];

const NUMBERS: Array<[keyof FormState, string, string]> = [
  ['young_days', 'Новый аккаунт Telegram, дней', 'Аккаунт новее всех, кто пришёл к вам раньше этого срока, считается новым. 0 — не учитывать.'],
  ['burst_count', 'Серия регистраций, аккаунтов', 'Столько аккаунтов с соседними номерами Telegram за 10 минут. 0 — не учитывать.'],
  ['ip_limit', 'Регистраций в кабинете с одного адреса за сутки', 'Следующие регистрации с этого адреса отмечаются. 0 — без лимита.'],
  ['mass_invites', 'Массовые приглашения, аккаунтов в сутки', 'Столько новых аккаунтов от одного пригласившего. 0 — не учитывать.'],
];

function SettingsCard({ summary, onSaved, onClose, phone }: { summary: AntibotSummary; onSaved: () => void; onClose: () => void; phone?: boolean }) {
  const [form, setForm] = useState<FormState>(() => formOf(summary));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const set = <K extends keyof FormState>(key: K, value: FormState[K]) => setForm((f) => ({ ...f, [key]: value }));

  const save = async () => {
    const config: Record<string, unknown> = {
      mode: form.mode,
      check_username: form.check_username,
      referral_guard: form.referral_guard,
      roulette_guard: form.roulette_guard,
      contest_guard: form.contest_guard,
      extra_domains: form.extra_domains.trim() || null,
    };
    for (const [key, label] of NUMBERS) {
      const raw = String(form[key]).trim();
      if (!/^\d{1,4}$/.test(raw)) {
        setError(`${label}: целое число от 0`);
        return;
      }
      config[key] = Number(raw);
    }
    setBusy(true);
    setError(null);
    try {
      await api('/api/admin/modules/antibot/config', { method: 'PUT', body: { config, clear: [] } });
      onSaved();
      onClose();
    } catch (err) {
      setError(failText(err, 'Настройки не сохранились'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className={phone ? 'hp-card pt-form' : 'card pt-form'}
      aria-labelledby="ab-set-h"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <h2 id="ab-set-h" className="section-title">
        Настройки антибота
      </h2>
      <fieldset className="pt-modes">
        <legend className="pf-label">Что делать с подозрительными</legend>
        {MODES.map((m) => (
          <label key={m.id} className="pt-mode">
            <input type="radio" name="ab-mode" checked={form.mode === m.id} onChange={() => set('mode', m.id)} disabled={busy} />
            <span>
              <span className="pt-mode-title">{m.title}</span>
              <span className="cap">{m.text}</span>
            </span>
          </label>
        ))}
      </fieldset>
      <div className="pt-grid">
        {NUMBERS.map(([key, label, hint]) => (
          <label key={key} className="pf-field">
            <span className="pf-label">{label}</span>
            <input className="field num" inputMode="numeric" value={String(form[key])} onChange={(e) => set(key, e.target.value)} maxLength={4} disabled={busy} />
            <span className="cap">{hint}</span>
          </label>
        ))}
      </div>
      <label className="pf-field">
        <span className="pf-label">Свои одноразовые почтовые домены</span>
        <input className="field" value={form.extra_domains} onChange={(e) => set('extra_domains', e.target.value)} placeholder="temp.example, junk.example" maxLength={2000} disabled={busy} />
        <span className="cap">Через запятую, в дополнение к встроенному списку известных одноразовых сервисов.</span>
      </label>
      <div className="sf-flag">
        <span className="sf-text">
          <span className="sf-label">Учитывать аккаунты без @username</span>
          <span className="cap">Сам по себе не делает аккаунт подозрительным, только вместе с другим признаком.</span>
        </span>
        <Switch on={form.check_username} onToggle={() => set('check_username', !form.check_username)} label="Учитывать аккаунты без @username" disabled={busy} />
      </div>
      <div className="sf-flag">
        <span className="sf-text">
          <span className="sf-label">Не начислять за самоприглашение и массовые приглашения</span>
          <span className="cap">Общий адрес, устройство или почта с пригласившим, либо много новых аккаунтов от одного клиента за сутки.</span>
        </span>
        <Switch on={form.referral_guard} onToggle={() => set('referral_guard', !form.referral_guard)} label="Не начислять за самоприглашение" disabled={busy} />
      </div>
      {GUARDS.map(([key, label, hint]) => (
        <div key={key} className="sf-flag">
          <span className="sf-text">
            <span className="sf-label">{label}</span>
            <span className="cap">{hint}</span>
          </span>
          <Switch on={form[key]} onToggle={() => set(key, !form[key])} label={label} disabled={busy} />
        </div>
      ))}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="pf-actions">
        <button type="submit" className="btn-main btn" disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </form>
  );
}

function Off({ phone }: { phone?: boolean }) {
  const text = (
    <>
      <span className="muted">Модуль «Антибот» выключен. </span>
      <Link to="/admin/modules/antibot" className="pr-inline-link">
        Включить модуль
      </Link>
    </>
  );
  return phone ? <p className="hp-quiet hp-card">{text}</p> : <section className="card card-pad"><p>{text}</p></section>;
}

function AntibotDesk({ data }: { data: Data }) {
  const { can } = useSession();
  const [settings, setSettings] = useState(false);
  const [asking, setAsking] = useState<number | null>(null);
  const [askError, setAskError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const writable = can('clients_write');

  const clear = async (m: MarkItem) => {
    setAskError(null);
    try {
      await data.clear(m);
      setAsking(null);
      setDone(`Отметка с клиента ${personName(m.client, m.client_id)} снята: пробный период, бонусы, рулетка и конкурсы снова доступны как всем.`);
    } catch (err) {
      setAskError(failText(err, 'Отметка не снялась'));
    }
  };

  const marks = data.marks ?? [];
  const journal = data.journal ?? [];

  return (
    <div className="pr-page">
      <div className="pr-head">
        <h1 className="page-title tight">Антибот</h1>
        {can('modules') && data.summary ? (
          <span className="pr-head-acts">
            <button type="button" className="btn-line hov" aria-expanded={settings} onClick={() => setSettings(!settings)}>
              Настроить
            </button>
          </span>
        ) : null}
      </div>
      <ClientsTabs />
      {data.off ? <Off /> : null}
      {data.summary ? <p className="rf-lead">{antibotLead(data.summary)}</p> : null}
      {data.error && !data.off ? (
        <p className="rf-lead" role="alert">
          <span className="err rf-err">{data.error}</span>
        </p>
      ) : null}
      {settings && data.summary ? <SettingsCard summary={data.summary} onSaved={() => void data.reload()} onClose={() => setSettings(false)} /> : null}

      {data.off ? null : (
        <section className="card pr-list" aria-labelledby="ab-m-h">
          <div className="pr-top">
            <h2 id="ab-m-h" className="section-title">
              Отмеченные клиенты
            </h2>
            <span className="muted">Снимите отметку, если это живой человек</span>
          </div>
          {done ? (
            <p className="pr-note" role="status">
              {done}
            </p>
          ) : null}
          {data.marks === null ? <p className="muted pr-pad">Загружаем…</p> : null}
          {data.marks !== null && marks.length === 0 ? <p className="muted pr-pad">Отмеченных нет: подозрительные аккаунты появятся здесь</p> : null}
          {marks.length ? (
            <div role="table" aria-label="Отмеченные клиенты">
              <div role="row" className="pt-row pt-mrow pr-th">
                <span role="columnheader">Клиент</span>
                <span role="columnheader">Почему</span>
                <span role="columnheader">Что было</span>
                <span role="columnheader">Отмечен</span>
                <span role="columnheader">
                  <span className="sr-only">Действия</span>
                </span>
              </div>
              {marks.map((m) => {
                const state = markState(m);
                return (
                  <Fragment key={m.client_id}>
                    <div role="row" className="pt-row pt-mrow">
                      <span role="cell" className="pr-cell">
                        <Who id={m.client_id} person={m.client} />
                      </span>
                      <span role="cell" className="pr-cell">
                        {reasonsText(m.reasons)}
                      </span>
                      <span role="cell" className="pr-cell">
                        <span className={`pr-state ${state.tone}`}>{state.text}</span>
                      </span>
                      <span role="cell" className="pr-cell">
                        {dateTime(m.marked_at)}
                      </span>
                      <span role="cell" className="pr-cell pr-act">
                        {writable ? (
                          <button type="button" className="b34 hov" aria-expanded={asking === m.client_id} onClick={() => setAsking(asking === m.client_id ? null : m.client_id)}>
                            Снять отметку
                          </button>
                        ) : null}
                      </span>
                    </div>
                    {asking === m.client_id ? (
                      <div role="row" className="pr-ask-row">
                        <div role="cell" className="pr-ask">
                          <span>
                            <b>Снять отметку с клиента {personName(m.client, m.client_id)}?</b> Антибот перестанет проверять этот аккаунт: пробный период, дни по промокодам, бонусы пригласившему, бесплатные вращения рулетки и конкурсы станут доступны.
                          </span>
                          <span className="pr-ask-btns">
                            <button type="button" className="b34 on-surface hov" disabled={data.busy !== null} onClick={() => void clear(m)}>
                              {data.busy === m.client_id ? 'Снимаем…' : 'Снять отметку'}
                            </button>
                            <button type="button" className="b34 on-surface hov" disabled={data.busy !== null} onClick={() => setAsking(null)}>
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
          {data.marksNext !== null ? (
            <div className="pr-pad">
              <button type="button" className="b40 hov" onClick={() => void data.more('marks')} disabled={data.loading !== null}>
                {data.loading === 'marks' ? 'Загружаем…' : 'Показать ещё'}
              </button>
            </div>
          ) : null}
        </section>
      )}

      {data.off ? null : (
        <section className="card pr-list" aria-labelledby="ab-j-h">
          <div className="pr-top">
            <h2 id="ab-j-h" className="section-title">
              Журнал срабатываний
            </h2>
            <div role="group" aria-label="Какие срабатывания показать" className="segs">
              {JOURNAL_FILTERS.map(([key, label]) => (
                <button key={key} type="button" aria-pressed={data.filter === key} onClick={() => data.setFilter(key)}>
                  {label}
                </button>
              ))}
            </div>
          </div>
          {data.journal === null ? <p className="muted pr-pad">Загружаем журнал…</p> : null}
          {data.journal !== null && journal.length === 0 ? <p className="muted pr-pad">Срабатываний пока нет</p> : null}
          {journal.length ? (
            <div role="table" aria-label="Журнал срабатываний">
              <div role="row" className="pt-row pt-jrow pr-th">
                <span role="columnheader">Когда</span>
                <span role="columnheader">Клиент</span>
                <span role="columnheader">Что произошло</span>
                <span role="columnheader">Подробности</span>
              </div>
              {journal.map((item) => {
                const note = journalNote(item);
                return (
                  <div key={item.id} role="row" className="pt-row pt-jrow">
                    <span role="cell" className="pr-cell">
                      {dateTime(item.at)}
                    </span>
                    <span role="cell" className="pr-cell">
                      <Who id={item.client_id} person={item.client} />
                    </span>
                    <span role="cell" className="pr-cell">
                      <span className="pt-strong">{item.title}</span>
                      {item.reasons.length ? <span className="pr-sub">{reasonsText(item.reasons)}</span> : null}
                    </span>
                    <span role="cell" className="pr-cell">
                      {note ? <span className="pr-sub">{note}</span> : <span className="muted">—</span>}
                    </span>
                  </div>
                );
              })}
            </div>
          ) : null}
          {data.journalNext !== null ? (
            <div className="pr-pad">
              <button type="button" className="b40 hov" onClick={() => void data.more('journal')} disabled={data.loading !== null}>
                {data.loading === 'journal' ? 'Загружаем…' : 'Показать ещё'}
              </button>
            </div>
          ) : null}
        </section>
      )}
    </div>
  );
}

function AntibotPhone({ data }: { data: Data }) {
  const { can } = useSession();
  const [settings, setSettings] = useState(false);
  const [asking, setAsking] = useState<number | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const writable = can('clients_write');

  const clear = async (m: MarkItem) => {
    try {
      await data.clear(m);
      setAsking(null);
      setToast(`Отметка с клиента ${personName(m.client, m.client_id)} снята`);
    } catch (err) {
      setToast(failText(err, 'Отметка не снялась'));
    }
  };

  const marks = data.marks ?? [];
  const journal = data.journal ?? [];

  return (
    <div className="hp pt-phone">
      <PhoneHeader title="Антибот" back={{ to: '/admin/clients', label: 'Все клиенты' }} />
      <ClientsTabs />
      {data.off ? <Off phone /> : null}
      {data.summary ? <p className="pt-lead">{antibotLead(data.summary)}</p> : null}
      {data.error && !data.off ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{data.error}</span>
        </p>
      ) : null}
      {can('modules') && data.summary ? (
        settings ? (
          <SettingsCard phone summary={data.summary} onSaved={() => void data.reload()} onClose={() => setSettings(false)} />
        ) : (
          <button type="button" className="pr-padd press" onClick={() => setSettings(true)}>
            Настроить антибот
          </button>
        )
      ) : null}

      {data.off ? null : (
        <section className="hp-group" aria-labelledby="ab-pm-h">
          <h2 id="ab-pm-h" className="hp-h2">
            Отмеченные клиенты
          </h2>
          {data.marks !== null && marks.length === 0 ? <p className="hp-quiet hp-card">Отмеченных нет</p> : null}
          {marks.length ? (
            <ul className="hp-card hp-list np-ul" aria-label="Отмеченные клиенты">
              {marks.map((m) => {
                const state = markState(m);
                return (
                  <li key={m.client_id} className="pt-pitem">
                    <Link to={`/admin/clients/${m.client_id}`} className="pt-plink">
                      <span className="hp-title">{personName(m.client, m.client_id)}</span>
                      <span className="hp-note muted">{reasonsText(m.reasons)}</span>
                      <span className={`hp-note pr-state ${state.tone}`}>{state.text}</span>
                    </Link>
                    {writable ? (
                      asking === m.client_id ? (
                        <div className="np-ask">
                          <p>
                            <strong>Снять отметку?</strong> Пробный период, бонусы, рулетка и конкурсы станут доступны.
                          </p>
                          <div className="np-ask-btns">
                            <button type="button" className="np-ok press" disabled={data.busy !== null} onClick={() => void clear(m)}>
                              {data.busy === m.client_id ? 'Снимаем…' : 'Снять'}
                            </button>
                            <button type="button" className="np-cancel press" onClick={() => setAsking(null)}>
                              Отменить
                            </button>
                          </div>
                        </div>
                      ) : (
                        <button type="button" className="hp-pill pt-pbtn" onClick={() => setAsking(m.client_id)}>
                          <span>Снять отметку</span>
                        </button>
                      )
                    ) : null}
                  </li>
                );
              })}
            </ul>
          ) : null}
          {data.marksNext !== null ? (
            <button type="button" className="pr-padd press" onClick={() => void data.more('marks')} disabled={data.loading !== null}>
              {data.loading === 'marks' ? 'Загружаем…' : 'Показать ещё'}
            </button>
          ) : null}
        </section>
      )}

      {data.off ? null : (
        <section className="hp-group" aria-labelledby="ab-pj-h">
          <h2 id="ab-pj-h" className="hp-h2">
            Журнал срабатываний
          </h2>
          <label className="pt-pselect">
            <span className="sr-only">Какие срабатывания показать</span>
            <select className="field" value={data.filter} onChange={(e) => data.setFilter(e.target.value)}>
              {JOURNAL_FILTERS.map(([key, label]) => (
                <option key={key} value={key}>
                  {label}
                </option>
              ))}
            </select>
          </label>
          {data.journal !== null && journal.length === 0 ? <p className="hp-quiet hp-card">Срабатываний пока нет</p> : null}
          {journal.length ? (
            <ul className="hp-card hp-list np-ul" aria-label="Журнал срабатываний">
              {journal.map((item) => (
                <li key={item.id} className="pt-pitem">
                  <Link to={`/admin/clients/${item.client_id}`} className="pt-plink">
                    <span className="hp-title">{item.title}</span>
                    <span className="hp-note muted">
                      {personName(item.client, item.client_id)} · {dateTime(item.at)}
                    </span>
                    {item.reasons.length ? <span className="hp-note muted">{reasonsText(item.reasons)}</span> : null}
                  </Link>
                </li>
              ))}
            </ul>
          ) : null}
          {data.journalNext !== null ? (
            <button type="button" className="pr-padd press" onClick={() => void data.more('journal')} disabled={data.loading !== null}>
              {data.loading === 'journal' ? 'Загружаем…' : 'Показать ещё'}
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
