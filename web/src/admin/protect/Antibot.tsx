import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
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
  CAPTCHA_PROVIDERS,
  captchaAlert,
  captchaProblem,
  failText,
  isOff,
  JOURNAL_FILTERS,
  journalNote,
  markState,
  MODES,
  reasonsText,
  type AntibotSummary,
  type CaptchaProvider,
  type JournalItem,
  type MarkItem,
  type Mode,
  type Page,
} from './model';
import { clientFrom } from './sections';
import { dayLimited, hiddenShort, hiddenText, PACE_NUMBERS, paceError, pacesOf, siteLine, SITE_NUMBERS, type SiteLimits } from './siteLimits';
import '../form.css';
import '../promo/promo.css';
import '../referral/referral.css';
import '../home/phone.css';
import './protect.css';

const PAGE = 30;

function journalPath(kind: string, client: number | null): string {
  return `${ANTIBOT}/journal?kind=${kind}&limit=${PAGE}${client ? `&client=${client}` : ''}`;
}

function useAntibot(client: number | null) {
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
      const page = await api<Page<JournalItem>>(journalPath(kind, client));
      setJournal(page.items);
      setJournalNext(page.next);
    } catch (err) {
      setJournal([]);
      setError(failText(err, 'Журнал не загрузился'));
    }
  }, [client]);

  useEffect(() => {
    void loadSummary();
    void loadMarks();
  }, [loadSummary, loadMarks]);

  useEffect(() => {
    setFilterState('all');
    setJournal(null);
    void loadJournal('all');
  }, [loadJournal]);

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
        const page = await api<Page<JournalItem>>(`${journalPath(filter, client)}&after=${journalNext}`);
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

  return { summary, off, error, marks, marksNext, journal, journalNext, filter, setFilter, loading, more, clear, busy, client, reload: loadSummary };
}

type Data = ReturnType<typeof useAntibot>;

export function Antibot() {
  const phone = usePhone();
  const [params] = useSearchParams();
  const data = useAntibot(clientFrom(params.get('client')));
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
  captcha: CaptchaProvider;
  captcha_site_key: string;
  captcha_secret: string;
  captcha_trial: 'all' | 'suspicious';
  captcha_signup: boolean;
  captcha_domains: string;
  site_address_day: string;
  site_network_day: string;
  site_minute: string;
  email_ip: string;
  telegram_ip: string;
  verify_ip: string;
};

function limitsOf(summary: AntibotSummary): SiteLimits {
  const s = summary.settings;
  return { site_address_day: s.site_address_day ?? 0, site_network_day: s.site_network_day ?? 0, site_minute: s.site_minute ?? 0 };
}

function formOf(summary: AntibotSummary): FormState {
  const s = summary.settings;
  const paces = pacesOf(s);
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
    captcha: s.captcha ?? 'none',
    captcha_site_key: s.captcha_site_key ?? '',
    captcha_secret: '',
    captcha_trial: s.captcha_trial ?? 'all',
    captcha_signup: s.captcha_signup ?? true,
    captcha_domains: (s.captcha_domains ?? []).join(', '),
    site_address_day: String(s.site_address_day ?? 0),
    site_network_day: String(s.site_network_day ?? 0),
    site_minute: String(s.site_minute ?? 0),
    email_ip: String(paces.email_ip),
    telegram_ip: String(paces.telegram_ip),
    verify_ip: String(paces.verify_ip),
  };
}

function KeyCheck({ form, keep, busy }: { form: FormState; keep: boolean; busy: boolean }) {
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [checking, setChecking] = useState(false);
  const typed = [form.captcha, form.captcha_site_key.trim(), form.captcha_secret.trim()].join('\n');
  const latest = useRef(typed);
  latest.current = typed;
  useEffect(() => setResult(null), [typed]);

  const check = async () => {
    const asked = typed;
    setChecking(true);
    setResult(null);
    let next: { ok: boolean; text: string };
    try {
      next = await api<{ ok: boolean; text: string }>(`${ANTIBOT}/captcha/check`, {
        method: 'POST',
        body: { provider: form.captcha, site_key: form.captcha_site_key.trim(), secret: form.captcha_secret.trim() || null },
      });
    } catch (err) {
      next = { ok: false, text: failText(err, 'Проверить ключи не получилось') };
    }
    setChecking(false);
    if (latest.current === asked) {
      setResult(next);
    }
  };

  const ready = form.captcha_site_key.trim() !== '' && (form.captcha_secret.trim() !== '' || keep);
  return (
    <div className="pt-keys">
      <button type="button" className="btn-line hov" onClick={() => void check()} disabled={busy || checking || !ready}>
        {checking ? 'Проверяем…' : 'Проверить ключи'}
      </button>
      {result ? (
        <p className={result.ok ? 'cap' : 'err'} role={result.ok ? 'status' : 'alert'}>
          {result.text}
        </p>
      ) : null}
    </div>
  );
}

function CaptchaFields({ form, summary, set, busy }: { form: FormState; summary: AntibotSummary; set: <K extends keyof FormState>(key: K, value: FormState[K]) => void; busy: boolean }) {
  const saved = summary.settings.captcha ?? 'none';
  const keep = summary.captcha?.secret_set === true && form.captcha === saved;
  const problem = summary.captcha?.problem ?? null;
  return (
    <fieldset className="pt-modes">
      <legend className="pf-label">Капча</legend>
      {CAPTCHA_PROVIDERS.map((p) => (
        <label key={p.id} className="pt-mode">
          <input type="radio" name="ab-captcha" checked={form.captcha === p.id} onChange={() => set('captcha', p.id)} disabled={busy} />
          <span>
            <span className="pt-mode-title">{p.title}</span>
            <span className="cap">{p.text}</span>
          </span>
        </label>
      ))}
      {form.captcha === 'none' ? null : (
        <>
          <div className="pt-grid">
            <label className="pf-field">
              <span className="pf-label">Ключ сайта</span>
              <input className="field" value={form.captcha_site_key} onChange={(e) => set('captcha_site_key', e.target.value)} maxLength={200} autoComplete="off" spellCheck={false} disabled={busy} />
              <span className="cap">Публичный, его видит браузер клиента.</span>
            </label>
            <label className="pf-field">
              <span className="pf-label">Секретный ключ</span>
              <input
                className="field"
                type="password"
                value={form.captcha_secret}
                onChange={(e) => set('captcha_secret', e.target.value)}
                placeholder={keep ? 'Сохранён. Пусто — не менять' : ''}
                maxLength={200}
                autoComplete="new-password"
                spellCheck={false}
                disabled={busy}
              />
              <span className="cap">Хранится зашифрованным, нужен только серверу.</span>
            </label>
          </div>
          <KeyCheck form={form} keep={keep} busy={busy} />
          <label className="pf-field">
            <span className="pf-label">Домены</span>
            <input className="field" value={form.captcha_domains} onChange={(e) => set('captcha_domains', e.target.value)} placeholder={window.location.hostname} maxLength={2000} autoComplete="off" spellCheck={false} disabled={busy} />
            <span className="cap">Через запятую, поддомены входят. Если заполнить, ответ капчи с другого домена не засчитывается. Пусто — не сверять.</span>
          </label>
          <fieldset className="pt-modes">
            <legend className="pf-label">Кому капча перед пробным периодом</legend>
            <label className="pt-mode">
              <input type="radio" name="ab-captcha-trial" checked={form.captcha_trial === 'all'} onChange={() => set('captcha_trial', 'all')} disabled={busy} />
              <span>
                <span className="pt-mode-title">Всем</span>
                <span className="cap">В боте кнопка открывает страницу проверки, в кабинете капча появляется на месте кнопки пробника.</span>
              </span>
            </label>
            <label className="pt-mode">
              <input type="radio" name="ab-captcha-trial" checked={form.captcha_trial === 'suspicious'} onChange={() => set('captcha_trial', 'suspicious')} disabled={busy} />
              <span>
                <span className="pt-mode-title">Только подозрительным</span>
                <span className="cap">Вместо кнопки «Я не робот» в режиме «Просить подтвердить». Остальные получают пробник сразу.</span>
              </span>
            </label>
          </fieldset>
          <div className="sf-flag">
            <span className="sf-text">
              <span className="sf-label">Капча при регистрации по почте</span>
              <span className="cap">В кабинете: в форме регистрации по паролю, а при входе кодом из письма — перед отправкой кода.</span>
            </span>
            <Switch on={form.captcha_signup} onToggle={() => set('captcha_signup', !form.captcha_signup)} label="Капча при регистрации по почте" disabled={busy} />
          </div>
          <p className="cap">
            Если сервис капчи не отвечает, пробный период и регистрация по почте не выдаются, клиент видит просьбу повторить через минуту. Иначе боты получали бы пробники как раз тогда, когда проверка не работает.
          </p>
          {problem ? (
            <p className="err" role="status">
              {captchaProblem(problem)}
            </p>
          ) : null}
        </>
      )}
    </fieldset>
  );
}

function SiteFields({ form, summary, set, busy }: { form: FormState; summary: AntibotSummary; set: <K extends keyof FormState>(key: K, value: FormState[K]) => void; busy: boolean }) {
  const hidden = summary.site?.hidden ?? null;
  const daily = dayLimited(limitsOf(summary)) || Number(form.site_address_day) > 0 || Number(form.site_network_day) > 0;
  return (
    <fieldset className="pt-modes pt-group">
      <legend>Когда не создавать новый аккаунт на сайте</legend>
      <p className="cap">
        Это отказ, а не отметка: сверх предела аккаунт не создаётся. Считаются регистрация по почте, вход через Google и Apple и вход через Telegram на сайте. Вход в существующий аккаунт, бот и мини-приложение пределы не задевают.
      </p>
      <div className="pt-grid">
        {SITE_NUMBERS.map(([key, label, hint]) => (
          <label key={key} className="pf-field">
            <span className="pf-label">{label}</span>
            <input className="field num" inputMode="numeric" value={form[key]} onChange={(e) => set(key, e.target.value)} maxLength={6} disabled={busy} />
            <span className="cap">{hint}</span>
          </label>
        ))}
      </div>
      {hidden ? (
        <div role={daily ? 'alert' : 'status'}>
          <p className={daily ? 'err' : 'cap'}>{hiddenText(hidden, dateTime(hidden.at).toLowerCase())}</p>
          {hidden.kind === 'cloudflare' && summary.site?.trusted ? <code className="pt-code">{summary.site.trusted}</code> : null}
        </div>
      ) : null}
    </fieldset>
  );
}

function PaceFields({ form, set, busy }: { form: FormState; set: <K extends keyof FormState>(key: K, value: FormState[K]) => void; busy: boolean }) {
  return (
    <fieldset className="pt-modes pt-group">
      <legend>Сколько обращений с одного адреса пропускать за 10 минут</legend>
      <p className="cap">
        Сверх числа клиент видит «Слишком много попыток» и ждёт. За мобильным оператором и общим Wi-Fi под одним адресом бывает много людей: если они упираются, поднимите число. Предел проверок кодов и паролей от этого не меняется.
      </p>
      <div className="pt-grid">
        {PACE_NUMBERS.map(([key, label, hint]) => (
          <label key={key} className="pf-field">
            <span className="pf-label">{label}</span>
            <input className="field num" inputMode="numeric" value={form[key]} onChange={(e) => set(key, e.target.value)} maxLength={4} disabled={busy} />
            <span className="cap">{hint}</span>
          </label>
        ))}
      </div>
    </fieldset>
  );
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
  ['ip_limit', 'Регистраций на сайте с одного адреса за сутки без отметки', 'Считаются регистрации по почте и через Google и Apple, вход через Telegram не считается. Следующие аккаунты создаются, но отмечаются: что они получат, решает режим. 0 — не отмечать.'],
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
      captcha: form.captcha,
      captcha_site_key: form.captcha_site_key.trim() || null,
      captcha_trial: form.captcha_trial,
      captcha_signup: form.captcha_signup,
      captcha_domains: form.captcha_domains.trim() || null,
    };
    if (form.captcha_secret.trim()) {
      config.captcha_secret = form.captcha_secret.trim();
    }
    if (form.captcha !== 'none') {
      const saved = summary.settings.captcha ?? 'none';
      if (!form.captcha_site_key.trim()) {
        setError('Капча: укажите ключ сайта');
        return;
      }
      if (!form.captcha_secret.trim() && !(summary.captcha?.secret_set === true && form.captcha === saved)) {
        setError('Капча: укажите секретный ключ');
        return;
      }
    }
    for (const [key, label] of NUMBERS) {
      const raw = String(form[key]).trim();
      if (!/^\d{1,4}$/.test(raw)) {
        setError(`${label}: целое число от 0`);
        return;
      }
      config[key] = Number(raw);
    }
    for (const [key, label] of SITE_NUMBERS) {
      const raw = form[key].trim();
      if (!/^\d{1,6}$/.test(raw) || Number(raw) > 100000) {
        setError(`${label}: целое число от 0 до 100000`);
        return;
      }
      config[key] = Number(raw);
    }
    for (const [key, label] of PACE_NUMBERS) {
      const raw = form[key].trim();
      const wrong = paceError(label, raw);
      if (wrong) {
        setError(wrong);
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
      <fieldset className="pt-modes pt-group">
        <legend>Когда отмечать</legend>
        <div className="pt-grid">
          {NUMBERS.map(([key, label, hint]) => (
            <label key={key} className="pf-field">
              <span className="pf-label">{label}</span>
              <input className="field num" inputMode="numeric" value={String(form[key])} onChange={(e) => set(key, e.target.value)} maxLength={4} disabled={busy} />
              <span className="cap">{hint}</span>
            </label>
          ))}
        </div>
      </fieldset>
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
          <span className="cap">То же устройство или почта, что у пригласившего, общий адрес, если устройства не отличить, либо много новых аккаунтов от одного клиента за сутки.</span>
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
      <SiteFields form={form} summary={summary} set={set} busy={busy} />
      <PaceFields form={form} set={set} busy={busy} />
      <CaptchaFields form={form} summary={summary} set={set} busy={busy} />
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

function AllJournal() {
  return (
    <Link to="/admin/clients/antibot" className="pr-inline-link">
      Показать всех
    </Link>
  );
}

function CaptchaAlert({ summary, className }: { summary: AntibotSummary; className: string }) {
  const text = captchaAlert(summary);
  return text ? (
    <p className={className} role="alert">
      <span className="err rf-err">{text}</span>
    </p>
  ) : null;
}

function SiteLead({ summary, open, className }: { summary: AntibotSummary; open: boolean; className: string }) {
  const site = summary.site;
  const limits = limitsOf(summary);
  const line = site ? siteLine(limits, site.day) : null;
  const hidden = site?.hidden && dayLimited(limits) && !open ? site.hidden : null;
  return (
    <>
      {line ? <p className={className}>{line}</p> : null}
      {hidden ? (
        <p className={className} role="alert">
          <span className="err rf-err">{hiddenShort(hidden)}</span>
        </p>
      ) : null}
    </>
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
      setDone(`Отметка с клиента ${personName(m.client, m.client_id)} снята: пробный период, промо-ссылки, бонусы, рулетка и конкурсы снова доступны как всем.`);
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
      {data.summary && !data.off ? <SiteLead summary={data.summary} open={settings} className="rf-lead" /> : null}
      {data.summary ? <CaptchaAlert summary={data.summary} className="rf-lead" /> : null}
      {data.error && !data.off ? (
        <p className="rf-lead" role="alert">
          <span className="err rf-err">{data.error}</span>
        </p>
      ) : null}
      {settings && data.summary ? <SettingsCard summary={data.summary} onSaved={() => void data.reload()} onClose={() => setSettings(false)} /> : null}

      {data.off || data.client ? null : (
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
                            <b>Снять отметку с клиента {personName(m.client, m.client_id)}?</b> Антибот перестанет проверять этот аккаунт: пробный период, дни по промокодам, подписки по промо-ссылкам, бонусы пригласившему, бесплатные вращения рулетки и конкурсы станут доступны.
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
              {data.client ? 'Журнал срабатываний клиента' : 'Журнал срабатываний'}
            </h2>
            {data.client ? <AllJournal /> : null}
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
      {data.summary && !data.off ? <SiteLead summary={data.summary} open={settings} className="pt-lead" /> : null}
      {data.summary ? <CaptchaAlert summary={data.summary} className="pt-lead" /> : null}
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

      {data.off || data.client ? null : (
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
            {data.client ? 'Журнал срабатываний клиента' : 'Журнал срабатываний'}
          </h2>
          {data.client ? <AllJournal /> : null}
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
