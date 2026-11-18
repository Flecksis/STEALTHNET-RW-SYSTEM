import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { BrandMark } from '../../lib/brand';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { AUTO_ROOT } from '../broadcast/model';
import { useSession } from '../session';
import { Switch } from '../ui';
import '../form.css';
import './mail-letters.css';
import { MailTabs } from './MailTabs';
import { SettingsLayout } from './SettingsLayout';
import { unclosedTag } from './telegram';

type Lang = 'ru' | 'en';
type Field = 'subject' | 'text' | 'button';
type GroupKey = 'login' | 'payments' | 'renewal' | 'support' | 'service';
type Var = { name: string; hint: string; always: boolean };
type Texts = { subject: string; text: string; button: string };
type Saved = { subject?: string | null; text?: string | null; button?: string | null };
type Letter = {
  id: string;
  group: GroupKey;
  title: string;
  when: string;
  vars: Var[];
  required: string[];
  button: boolean;
  path: string | null;
  default: Record<Lang, Texts>;
  saved: Record<Lang, Saved | null>;
};
type Group = { key: GroupKey; title: string; switch: boolean; on: boolean };
type Limits = { subject: number; text: number; button: number; sign: number; sender: number };
type View = {
  groups: Group[];
  letters: Letter[];
  sign: { saved: Record<Lang, string | null>; default: Record<Lang, string>; vars: Var[] };
  sender: string | null;
  brand: string;
  from_email: string | null;
  logo_url: string | null;
  accent: boolean;
  sends: boolean;
  site: boolean;
  english: boolean;
  limits: Limits;
};
type Preview = { from: string; subject: string; html: string; text: string };
type Drafts = Record<string, Record<Lang, Texts>>;
type Spot = { letter: string; field: Field | 'sign' };
type Note = { text: string; bad: boolean };

const FRAME = 'frame';
const LANGS: Array<[Lang, string]> = [
  ['ru', 'Русский'],
  ['en', 'English'],
];
const FIELD_TITLES: Record<Field, string> = { subject: 'тема', text: 'текст', button: 'кнопка' };
const NAME = /^[a-z0-9_]{1,32}$/;
const WHERE: Record<string, string> = {
  '/': 'ведёт на сайт',
  '/cabinet': 'ведёт в личный кабинет',
  '/cabinet/payments': 'ведёт в историю оплат',
  '/cabinet/topup': 'ведёт на пополнение баланса',
  '/cabinet/pay': 'ведёт на оплату подписки',
  '/cabinet/gifts': 'ведёт в подарки',
  '/cabinet/help': 'ведёт в поддержку',
  '/cabinet/help/{ticket}': 'ведёт в переписку по обращению',
  '/cabinet/security': 'ведёт в настройки входа',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function initial(letter: Letter, lang: Lang): Texts {
  const saved = letter.saved[lang];
  const base = letter.default[lang];
  return {
    subject: saved?.subject ?? base.subject,
    text: saved?.text ?? base.text,
    button: saved?.button ?? base.button,
  };
}

function same(a: Texts, b: Texts): boolean {
  return a.subject.trim() === b.subject.trim() && a.text.trim() === b.text.trim() && a.button.trim() === b.button.trim();
}

function length(value: string): number {
  return [...value.trim()].length;
}

function braces(value: string): string[] {
  const found: string[] = [];
  const pattern = /\{([^{}\s]{1,32})\}/g;
  let match = pattern.exec(value);
  while (match) {
    found.push(match[1] ?? '');
    match = pattern.exec(value);
  }
  return found;
}

function problemOf(value: string, vars: Var[], max: number, oneLine: boolean, required: string[] = []): string | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  if (oneLine && trimmed.includes('\n')) {
    return 'нужна одна строка без переносов';
  }
  if ([...trimmed].length > max) {
    return `длиннее ${max} символов`;
  }
  for (const name of braces(trimmed)) {
    if (vars.some((v) => v.name === name)) {
      continue;
    }
    const allowed = [...new Set(vars.map((v) => `{${v.name}}`))].join(', ');
    const start = NAME.test(name) ? `подстановки {${name}} в этом письме нет` : `подстановка {${name}} записана неверно`;
    return allowed ? `${start}, можно: ${allowed}` : start;
  }
  if (!oneLine) {
    const tag = unclosedTag(trimmed);
    if (tag) {
      return tag;
    }
  }
  const used = braces(trimmed);
  for (const name of required) {
    if (!used.includes(name)) {
      const hint = vars.find((v) => v.name === name)?.hint ?? 'значение';
      return `нужна подстановка {${name}}: без неё клиент не увидит ${hint}`;
    }
  }
  return null;
}

function letterProblem(letter: Letter, texts: Texts, limits: Limits): [Field, string] | null {
  const checks: Array<[Field, string | null]> = [
    ['subject', problemOf(texts.subject, letter.vars, limits.subject, true)],
    ['text', problemOf(texts.text, letter.vars, limits.text, false, letter.required)],
    ['button', letter.button ? problemOf(texts.button, letter.vars, limits.button, true) : null],
  ];
  for (const [field, problem] of checks) {
    if (problem) {
      return [field, problem];
    }
  }
  return null;
}

function rowsOf(value: string, least: number): number {
  return Math.min(12, Math.max(least, value.split('\n').length + (value.length > 90 ? 1 : 0)));
}

function firstLine(value: string, brand: string): string {
  return (value.split('\n').find((line) => line.trim()) ?? '').replace(/\{brand\}/g, brand).trim();
}

export function SettingsMailLetters() {
  const { me, can } = useSession();
  const phone = usePhone();
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [lang, setLang] = useState<Lang>('ru');
  const [drafts, setDrafts] = useState<Drafts>({});
  const [signs, setSigns] = useState<Record<Lang, string>>({ ru: '', en: '' });
  const [sender, setSender] = useState('');
  const [groups, setGroups] = useState<Partial<Record<GroupKey, boolean>>>({});
  const [open, setOpen] = useState<string | null>(null);
  const [shown, setShown] = useState('subscription_ready');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<Note | null>(null);
  const [sheet, setSheet] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [plain, setPlain] = useState(false);
  const [to, setTo] = useState(me.email ?? '');
  const [testing, setTesting] = useState(false);
  const [testNote, setTestNote] = useState<Note | null>(null);
  const fields = useRef<Record<string, HTMLInputElement | HTMLTextAreaElement | null>>({});
  const spot = useRef<Spot | null>(null);
  const asked = useRef(0);
  const modules = can('modules');

  const apply = useCallback((data: View) => {
    setView(data);
    const next: Drafts = {};
    for (const letter of data.letters) {
      next[letter.id] = { ru: initial(letter, 'ru'), en: initial(letter, 'en') };
    }
    setDrafts(next);
    setSigns({
      ru: data.sign.saved.ru ?? data.sign.default.ru,
      en: data.sign.saved.en ?? data.sign.default.en,
    });
    setSender(data.sender ?? '');
    setGroups(Object.fromEntries(data.groups.map((g) => [g.key, g.on])) as Partial<Record<GroupKey, boolean>>);
  }, []);

  useEffect(() => {
    api<View>('/api/admin/mail/letters')
      .then((data) => {
        apply(data);
        setError(null);
      })
      .catch((err: unknown) => setError(failText(err, 'Шаблоны писем не загрузились')));
  }, [apply]);

  const letter = view?.letters.find((l) => l.id === shown) ?? null;
  const draft = letter ? drafts[letter.id]?.[lang] : undefined;
  const sign = signs[lang];

  useEffect(() => {
    if (!letter || !draft) {
      return;
    }
    const ticket = ++asked.current;
    const timer = window.setTimeout(() => {
      api<Preview>('/api/admin/mail/letters/preview', {
        method: 'POST',
        body: { letter: letter.id, lang, ...draft, sign },
      })
        .then((next) => {
          if (ticket === asked.current) {
            setPreview(next);
            setPreviewError(null);
          }
        })
        .catch((err: unknown) => {
          if (ticket === asked.current) {
            setPreviewError(failText(err, 'Письмо не собралось'));
          }
        });
    }, 350);
    return () => window.clearTimeout(timer);
  }, [letter, draft, lang, sign]);

  const changed = useMemo(() => {
    const counts: Record<Lang, number> = { ru: 0, en: 0 };
    if (!view) {
      return { counts, letters: [] as Array<[Letter, Lang]>, signs: [] as Lang[], sender: false, groups: [] as GroupKey[] };
    }
    const letters: Array<[Letter, Lang]> = [];
    for (const item of view.letters) {
      for (const [code] of LANGS) {
        const current = drafts[item.id]?.[code];
        if (current && !same(current, initial(item, code))) {
          letters.push([item, code]);
          counts[code] += 1;
        }
      }
    }
    const changedSigns = LANGS.map(([code]) => code).filter(
      (code) => signs[code].trim() !== (view.sign.saved[code] ?? view.sign.default[code]).trim(),
    );
    for (const code of changedSigns) {
      counts[code] += 1;
    }
    const changedGroups = view.groups.filter((g) => g.switch && groups[g.key] !== g.on).map((g) => g.key);
    return {
      counts,
      letters,
      signs: changedSigns,
      sender: sender.trim() !== (view.sender ?? ''),
      groups: changedGroups,
    };
  }, [view, drafts, signs, sender, groups]);

  const unsaved = changed.letters.length + changed.signs.length + (changed.sender ? 1 : 0) + changed.groups.length;
  const dirty = unsaved > 0;

  useEffect(() => {
    if (!dirty) {
      return;
    }
    const onLeave = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', onLeave);
    return () => window.removeEventListener('beforeunload', onLeave);
  }, [dirty]);

  useEffect(() => {
    if (!sheet) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setSheet(false);
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [sheet]);

  useEffect(() => {
    if (!phone) {
      setSheet(false);
    }
  }, [phone]);

  if (!view) {
    return (
      <SettingsLayout>
        <MailTabs />
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем шаблоны писем…</p>
        )}
      </SettingsLayout>
    );
  }

  const english = lang === 'en';
  const signVars = view.sign.vars;
  const signProblem = problemOf(sign, signVars, view.limits.sign, false);

  const setDraft = (id: string, patch: Partial<Texts>) => {
    setDrafts((all) => {
      const current = all[id];
      if (!current) {
        return all;
      }
      return { ...all, [id]: { ...current, [lang]: { ...current[lang], ...patch } } };
    });
    setStatus(null);
  };

  const toggle = (id: string) => {
    setOpen((current) => (current === id ? null : id));
    if (id !== FRAME) {
      setShown(id);
    }
  };

  const insert = (id: string, name: string) => {
    const token = `{${name}}`;
    const target: Field | 'sign' = spot.current?.letter === id ? spot.current.field : id === FRAME ? 'sign' : 'text';
    const key = `${id}:${target}`;
    const element = fields.current[key];
    const current = target === 'sign' ? sign : (drafts[id]?.[lang]?.[target] ?? '');
    const start = element?.selectionStart ?? current.length;
    const end = element?.selectionEnd ?? current.length;
    const next = current.slice(0, start) + token + current.slice(end);
    if (target === 'sign') {
      setSigns({ ...signs, [lang]: next });
    } else {
      setDraft(id, { [target]: next });
    }
    window.requestAnimationFrame(() => {
      element?.focus();
      element?.setSelectionRange(start + token.length, start + token.length);
    });
  };

  const bind = (id: string, field: Field | 'sign') => ({
    ref: (el: HTMLInputElement | HTMLTextAreaElement | null) => {
      fields.current[`${id}:${field}`] = el;
    },
    onFocus: () => {
      spot.current = { letter: id, field };
    },
  });

  const save = async () => {
    if (!dirty || busy) {
      return;
    }
    for (const [item, code] of changed.letters) {
      const texts = drafts[item.id]?.[code];
      const problem = texts ? letterProblem(item, texts, view.limits) : null;
      if (problem) {
        setLang(code);
        setOpen(item.id);
        setShown(item.id);
        setStatus({ text: `«${item.title}»${code === 'en' ? ', английский' : ''}, ${FIELD_TITLES[problem[0]]}: ${problem[1]}`, bad: true });
        return;
      }
    }
    for (const code of changed.signs) {
      const problem = problemOf(signs[code], signVars, view.limits.sign, false);
      if (problem) {
        setLang(code);
        setOpen(FRAME);
        setStatus({ text: `Подпись${code === 'en' ? ', английский' : ''}: ${problem}`, bad: true });
        return;
      }
    }
    const letters: Record<string, Partial<Record<Lang, Texts>>> = {};
    for (const [item, code] of changed.letters) {
      const texts = drafts[item.id]?.[code];
      if (texts) {
        letters[item.id] = { ...(letters[item.id] ?? {}), [code]: texts };
      }
    }
    const body: {
      letters: typeof letters;
      sign: Partial<Record<Lang, string>>;
      sender?: string;
      groups?: Partial<Record<GroupKey, boolean>>;
    } = { letters, sign: {} };
    for (const code of changed.signs) {
      body.sign[code] = signs[code];
    }
    if (changed.sender) {
      body.sender = sender.trim();
    }
    if (changed.groups.length) {
      body.groups = Object.fromEntries(changed.groups.map((key) => [key, groups[key] === true]));
    }
    setBusy(true);
    setStatus(null);
    try {
      apply(await api<View>('/api/admin/mail/letters', { method: 'PUT', body }));
      setStatus({ text: 'Сохранено: письма уже уходят по-новому', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Шаблоны не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const cancel = () => {
    apply(view);
    setStatus(null);
  };

  const sendTest = async (event: FormEvent) => {
    event.preventDefault();
    if (!letter || !draft || testing || !to.trim()) {
      return;
    }
    setTesting(true);
    setTestNote(null);
    try {
      await api('/api/admin/mail/letters/test', {
        method: 'POST',
        body: { to: to.trim(), letter: letter.id, lang, ...draft, sign },
      });
      setTestNote({ text: `Письмо отправлено на ${to.trim()}. Если его нет во «Входящих», проверьте «Спам».`, bad: false });
    } catch (err) {
      setTestNote({ text: failText(err, 'Письмо не отправилось'), bad: true });
    } finally {
      setTesting(false);
    }
  };

  const statusText = status?.text ?? (dirty ? `Не сохранено: ${unsaved}` : '');

  const previewBody = (
    <>
      <span className="segs ml-pv-tools" role="group" aria-label="Что показать">
        <button type="button" aria-pressed={!plain} onClick={() => setPlain(false)}>
          Письмо
        </button>
        <button type="button" aria-pressed={plain} onClick={() => setPlain(true)}>
          Только текст
        </button>
      </span>
      {preview ? (
        <>
          <span className="ml-env">
            <span className="ml-env-from">{preview.from}</span>
            <span className="ml-env-subject">{preview.subject}</span>
          </span>
          {plain ? (
            <pre className="ml-plain">{preview.text}</pre>
          ) : (
            <iframe
              className="ml-frame"
              title={`Письмо «${letter?.title ?? ''}»`}
              sandbox="allow-same-origin"
              srcDoc={preview.html}
              onLoad={(event) => {
                const frame = event.currentTarget;
                const root = frame.contentDocument?.documentElement;
                if (!root) {
                  return;
                }
                frame.style.height = '0px';
                frame.style.height = `${root.scrollHeight}px`;
              }}
            />
          )}
        </>
      ) : (
        <span className="cap">{previewError ? null : 'Собираем письмо…'}</span>
      )}
      {previewError ? <span className="err">{previewError}</span> : null}
      {view.site ? null : (
        <span className="cap">
          У панели нет адреса сайта, поэтому в письмах нет кнопок и логотипа. Адрес задаётся на сервере командой <code className="mono">stealthnet domain</code>.
        </span>
      )}
      <form className="ml-test" onSubmit={(event) => void sendTest(event)}>
        <label className="ml-label" htmlFor="ml-test-to">
          Проверка
        </label>
        <span className="ml-test-row">
          <input
            id="ml-test-to"
            className="field"
            type="email"
            value={to}
            placeholder="Куда отправить"
            onChange={(event) => setTo(event.target.value)}
            disabled={testing}
          />
          <button type="submit" className="b40 hov" disabled={testing || !to.trim() || !view.sends}>
            {testing ? 'Отправляем…' : 'Отправить себе'}
          </button>
        </span>
        <span className={testNote?.bad ? 'err' : 'cap'} role={testNote?.bad ? 'alert' : 'status'}>
          {testNote?.text ??
            (view.sends ? 'Придёт это письмо с теми же выдуманными данными' : 'Проверка заработает, когда во вкладке «Отправка» выберут способ отправки')}
        </span>
      </form>
    </>
  );

  const frameItem = (
    <Item
      key={FRAME}
      title="Оформление писем"
      summary={`Логотип и кнопки как в кабинете, подпись «${firstLine(sign, view.brand)}»`}
      mark={changed.sender || changed.signs.includes(lang) || sign.trim() !== view.sign.default[lang]}
      open={open === FRAME}
      onToggle={() => toggle(FRAME)}
    >
      <label className="ml-field">
        <span className="ml-label">Имя отправителя</span>
        <input
          className="field"
          value={sender}
          placeholder={view.brand}
          maxLength={view.limits.sender * 2}
          onChange={(event) => {
            setSender(event.target.value);
            setStatus(null);
          }}
          disabled={busy || !modules}
        />
        <span className="cap">
          Так письма подписаны в поле «От»{view.from_email ? `: ${sender.trim() || view.brand} <${view.from_email}>` : ''}. Адрес — во вкладке «Отправка».
        </span>
      </label>
      <div className="ml-field">
        <label className="ml-label" htmlFor={`ml-sign-${lang}`}>
          Подпись внизу письма
        </label>
        <textarea
          id={`ml-sign-${lang}`}
          className="field ml-text"
          rows={rowsOf(sign, 2)}
          value={sign}
          aria-invalid={signProblem !== null}
          onChange={(event) => {
            setSigns({ ...signs, [lang]: event.target.value });
            setStatus(null);
          }}
          disabled={busy}
          lang={lang}
          {...bind(FRAME, 'sign')}
        />
        <Chips vars={signVars} onInsert={(name) => insert(FRAME, name)} disabled={busy} />
        <span className="ml-meta">
          <span className={signProblem ? 'err' : 'cap'}>
            {signProblem ?? (english ? 'Подпись писем на английском' : 'Одна на все письма. У писем на английском — своя')}
          </span>
          <span className="cap num">
            {length(sign)} / {view.limits.sign}
          </span>
          {sign.trim() !== view.sign.default[lang] ? (
            <button type="button" className="bt-reset hov" onClick={() => setSigns({ ...signs, [lang]: view.sign.default[lang] })} disabled={busy}>
              Вернуть как было
            </button>
          ) : null}
        </span>
      </div>
      <div className="ml-field">
        <span className="ml-label">Логотип и кнопки</span>
        <span className="ml-logo">
          <BrandMark name={view.brand} logo={view.logo_url ?? undefined} className="mark" />
          <span>
            Как в кабинете: {view.logo_url ? 'ваш логотип' : 'первая буква названия'}, {view.accent ? 'кнопки цвета акцента' : 'монохромные кнопки'}
          </span>
        </span>
        <Link to="/admin/settings/look" className="ml-link">
          Изменить в «Оформлении»
          <Icon name="external" size={15} />
        </Link>
      </div>
    </Item>
  );

  return (
    <SettingsLayout>
      <MailTabs />
      <div className="ml-head">
        <div className="head-stack">
          <h2 className="section-title">Шаблоны писем</h2>
          <span className="muted">Темы и тексты писем, которые панель отправляет клиентам сама. Рассылки пишутся в разделе «Рассылки».</span>
        </div>
        <span className="segs ml-langs" role="group" aria-label="Язык писем">
          {LANGS.map(([code, title]) => (
            <button key={code} type="button" aria-pressed={lang === code} onClick={() => setLang(code)}>
              {title}
              {changed.counts[code] ? <span className="bt-count">{changed.counts[code]}</span> : null}
            </button>
          ))}
        </span>
      </div>
      {view.sends ? null : (
        <p className="cap ml-note">
          Письма сейчас не уходят: во вкладке <Link to="/admin/settings/mail">«Отправка»</Link> не выбран способ отправки. Шаблоны можно подготовить заранее.
        </p>
      )}
      {english ? (
        <p className="cap ml-note">
          {view.english
            ? 'Эти письма получают клиенты, которые выбрали английский. Пустое поле — стандартный английский текст.'
            : 'Английский сейчас выключен в «Языках»: письма сохранятся и заработают, когда его включат.'}
        </p>
      ) : null}
      <div className={phone ? 'ml-grid single' : 'ml-grid'}>
        <section className="sec ml-list" aria-label="Письма">
          {frameItem}
          {view.groups.map((group) => {
            const items = view.letters.filter((l) => l.group === group.key);
            const on = groups[group.key] !== false;
            return (
              <Fragment key={group.key}>
                <div className="ml-group">
                  <span className="ml-group-text">
                    <span className="ml-group-name">{group.title}</span>
                    {group.switch ? (
                      on ? null : <span className="ml-group-note">не отправляются</span>
                    ) : (
                      <span className="ml-group-note">уходят всегда</span>
                    )}
                    {group.key === 'renewal' ? (
                      <span className="ml-group-note">
                        Напоминания о конце подписки — в <Link to={AUTO_ROOT}>автоматических рассылках</Link>
                      </span>
                    ) : null}
                  </span>
                  {group.switch ? (
                    <Switch
                      on={on}
                      onToggle={() => {
                        setGroups({ ...groups, [group.key]: !on });
                        setStatus(null);
                      }}
                      label={`Отправлять письма: ${group.title}`}
                      disabled={busy || !modules}
                    />
                  ) : null}
                </div>
                {items.map((item) => {
                  const texts = drafts[item.id]?.[lang] ?? initial(item, lang);
                  return (
                    <Item
                      key={item.id}
                      title={item.title}
                      summary={item.when}
                      mark={!same(texts, item.default[lang])}
                      open={open === item.id}
                      off={!on}
                      onToggle={() => toggle(item.id)}
                    >
                      <LetterEditor
                        letter={item}
                        lang={lang}
                        texts={texts}
                        limits={view.limits}
                        disabled={busy}
                        bind={(field) => bind(item.id, field)}
                        onChange={(patch) => setDraft(item.id, patch)}
                        onInsert={(name) => insert(item.id, name)}
                      />
                    </Item>
                  );
                })}
              </Fragment>
            );
          })}
        </section>
        {phone ? null : (
          <aside className="sec ml-pv" aria-label={`Как клиент увидит письмо «${letter?.title ?? ''}»`}>
            <span className="ml-pv-title">{letter?.title}</span>
            {previewBody}
          </aside>
        )}
      </div>
      <div className="ml-save">
        {phone ? (
          <>
            {statusText ? (
              <span role="status" className={status?.bad ? 'ml-save-status bad' : 'ml-save-status'}>
                {statusText}
              </span>
            ) : null}
            <span className="ml-save-btns">
              {dirty ? (
                <button type="button" className="btn-line hov" onClick={cancel} disabled={busy}>
                  Отменить
                </button>
              ) : null}
              <button type="button" className="btn-line hov" onClick={() => setSheet(true)}>
                Как увидит клиент
              </button>
              <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || !dirty}>
                {busy ? 'Сохраняем…' : 'Сохранить'}
              </button>
            </span>
          </>
        ) : (
          <>
            <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || !dirty}>
              {busy ? 'Сохраняем…' : 'Сохранить'}
            </button>
            <button type="button" className="btn-line hov" onClick={cancel} disabled={busy || !dirty}>
              Отменить
            </button>
            <span role="status" className={status?.bad ? 'bad' : undefined}>
              {statusText}
            </span>
          </>
        )}
      </div>
      {sheet ? (
        <div className="ml-sheet" role="dialog" aria-modal="true" aria-label="Как увидит клиент">
          <div className="ml-sheet-head">
            <span className="ml-sheet-title">{letter?.title}</span>
            <button type="button" className="ml-close hov" aria-label="Закрыть" onClick={() => setSheet(false)} autoFocus>
              <Icon name="close" size={20} />
            </button>
          </div>
          <div className="ml-sheet-body">{previewBody}</div>
        </div>
      ) : null}
    </SettingsLayout>
  );
}

type ItemProps = {
  title: string;
  summary: string;
  mark: boolean;
  open: boolean;
  off?: boolean;
  onToggle: () => void;
  children: React.ReactNode;
};

function Item({ title, summary, mark, open, off = false, onToggle, children }: ItemProps) {
  const classes = ['ml-item', open ? 'open' : '', off ? 'off' : ''].filter(Boolean).join(' ');
  return (
    <div className={classes}>
      <button type="button" className="ml-row hov" aria-expanded={open} onClick={onToggle}>
        <span className="ml-row-text">
          <span className="ml-row-name">
            {title}
            {mark ? <span className="ml-mark">изменено</span> : null}
          </span>
          <span className="ml-row-sum">{summary}</span>
        </span>
        <Icon name="chevron-down" size={18} />
      </button>
      {open ? <div className="ml-body">{children}</div> : null}
    </div>
  );
}

function Chips({ vars, onInsert, disabled }: { vars: Var[]; onInsert: (name: string) => void; disabled: boolean }) {
  if (!vars.length) {
    return null;
  }
  return (
    <span className="bt-vars ml-chips" aria-label="Подстановки">
      {vars.map((v) => (
        <button key={v.name} type="button" className="bt-chip hov" title={v.always ? v.hint : `${v.hint}, бывает не всегда`} onClick={() => onInsert(v.name)} disabled={disabled}>
          {`{${v.name}}`}
          <span className="bt-chip-hint">{v.always ? v.hint : `${v.hint}, не всегда`}</span>
        </button>
      ))}
    </span>
  );
}

type EditorProps = {
  letter: Letter;
  lang: Lang;
  texts: Texts;
  limits: Limits;
  disabled: boolean;
  bind: (field: Field) => {
    ref: (el: HTMLInputElement | HTMLTextAreaElement | null) => void;
    onFocus: () => void;
  };
  onChange: (patch: Partial<Texts>) => void;
  onInsert: (name: string) => void;
};

function LetterEditor({ letter, lang, texts, limits, disabled, bind, onChange, onInsert }: EditorProps) {
  const base = letter.default[lang];
  const id = `ml-${letter.id}-${lang}`;
  const subjectProblem = problemOf(texts.subject, letter.vars, limits.subject, true);
  const textProblem = problemOf(texts.text, letter.vars, limits.text, false, letter.required);
  const buttonProblem = letter.button ? problemOf(texts.button, letter.vars, limits.button, true) : null;
  const custom = !same(texts, base);
  return (
    <>
      <div className="ml-field">
        <label className="ml-label" htmlFor={`${id}-subject`}>
          Тема
          <span className="cap num">
            {length(texts.subject)} / {limits.subject}
          </span>
        </label>
        <input
          id={`${id}-subject`}
          className="field"
          value={texts.subject}
          placeholder={base.subject}
          aria-invalid={subjectProblem !== null}
          onChange={(event) => onChange({ subject: event.target.value })}
          disabled={disabled}
          lang={lang}
          {...bind('subject')}
        />
        {subjectProblem ? <span className="err">{subjectProblem}</span> : null}
      </div>
      <div className="ml-field">
        <label className="ml-label" htmlFor={`${id}-text`}>
          Текст
        </label>
        <textarea
          id={`${id}-text`}
          className="field ml-text"
          rows={rowsOf(texts.text, 4)}
          value={texts.text}
          placeholder={base.text}
          aria-invalid={textProblem !== null}
          onChange={(event) => onChange({ text: event.target.value })}
          disabled={disabled}
          spellCheck
          lang={lang}
          {...bind('text')}
        />
        <Chips vars={letter.vars} onInsert={onInsert} disabled={disabled} />
        <span className="ml-meta">
          <span className={textProblem ? 'err' : 'cap'}>{textProblem ?? 'Разметка как в текстах бота'}</span>
          <span className="cap num">
            {length(texts.text)} / {limits.text}
          </span>
          {custom ? (
            <button type="button" className="bt-reset hov" onClick={() => onChange({ ...base })} disabled={disabled}>
              Вернуть как было
            </button>
          ) : null}
        </span>
      </div>
      {letter.button ? (
        <div className="ml-field">
          <label className="ml-label" htmlFor={`${id}-button`}>
            Кнопка
            <span className="cap">{(letter.path && WHERE[letter.path]) ?? ''}</span>
          </label>
          <input
            id={`${id}-button`}
            className="field"
            value={texts.button}
            placeholder={base.button}
            aria-invalid={buttonProblem !== null}
            onChange={(event) => onChange({ button: event.target.value })}
            disabled={disabled}
            lang={lang}
            {...bind('button')}
          />
          {buttonProblem ? <span className="err">{buttonProblem}</span> : null}
        </div>
      ) : null}
    </>
  );
}
