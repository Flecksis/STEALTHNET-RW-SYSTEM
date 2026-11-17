import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router';
import { api, upload } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { displayName, type ClientRow } from '../clients/types';
import { useSession } from '../session';
import { unclosedTag, visibleLength } from '../pages/telegram';
import { ChannelPreview } from './Preview';
import {
  AUDIENCES,
  BASE,
  CHANNELS,
  addresses,
  at,
  audienceOf,
  clients,
  count,
  hasBot,
  hasEmail,
  reachText,
  reachTotal,
  failText,
  imageUrl,
  localInput,
  zoneLabel,
  type Audience,
  type AudienceKind,
  type Broadcast,
  type Channel,
  type ButtonSpec,
  type Image,
  type Options,
  type Reach,
  type Section,
} from './model';
import '../form.css';

type Kind = 'url' | 'section';
type ButtonDraft = { text: string; kind: Kind; url: string; section: Section };
type Target = { id: number; name: string; handle: string };
type Note = { text: string; tone: 'ok' | 'bad' | 'plain' };

const TEST_KEY = 'sn.broadcast.test';
const MAIL_KEY = 'sn.broadcast.mailtest';

function stored(key: string): string {
  try {
    return window.localStorage.getItem(key) ?? '';
  } catch {
    return '';
  }
}

function keep(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    return;
  }
}
const URL_START = /^(https?:\/\/[^\s/?#]+|tg:\/\/\S+|t\.me\/\S+|@\w+)/i;

function toDraft(button: ButtonSpec): ButtonDraft {
  return button.section
    ? { text: button.text, kind: 'section', url: '', section: button.section }
    : { text: button.text, kind: 'url', url: button.url ?? '', section: 'menu' };
}

function toSpec(button: ButtonDraft): ButtonSpec {
  return button.kind === 'section' ? { text: button.text, section: button.section } : { text: button.text, url: button.url };
}

function storedTarget(): Target | null {
  try {
    const raw = window.localStorage.getItem(TEST_KEY);
    if (!raw) {
      return null;
    }
    const value = JSON.parse(raw) as Target;
    return typeof value.id === 'number' && typeof value.name === 'string' ? value : null;
  } catch {
    return null;
  }
}

function storeTarget(target: Target | null) {
  try {
    if (target) {
      window.localStorage.setItem(TEST_KEY, JSON.stringify(target));
    } else {
      window.localStorage.removeItem(TEST_KEY);
    }
  } catch {
    return;
  }
}

function handleOf(row: ClientRow): string {
  if (row.telegram_username) {
    return `@${row.telegram_username}`;
  }
  if (row.telegram_id) {
    return `Telegram ${row.telegram_id}`;
  }
  return row.email ?? '';
}

function fileSize(bytes: number): string {
  if (bytes < 1024 * 1024) {
    return `${Math.max(1, Math.round(bytes / 1024))} КБ`;
  }
  return `${(bytes / 1024 / 1024).toFixed(1).replace('.', ',')} МБ`;
}

function nextHour(): Date {
  const date = new Date(Date.now() + 3_600_000);
  date.setMinutes(0, 0, 0);
  return date;
}

function buttonProblem(button: ButtonDraft, max: number): string | null {
  const text = button.text.trim();
  if (!text) {
    return 'Напишите надпись';
  }
  if ([...text].length > max) {
    return `Надпись длиннее ${max} знаков`;
  }
  if (button.kind === 'url') {
    const url = button.url.trim();
    if (!url) {
      return 'Укажите адрес ссылки';
    }
    if (!URL_START.test(url) || /\s/.test(url)) {
      return 'Адрес должен начинаться с https://, http:// или tg://';
    }
  }
  return null;
}

type Props = {
  options: Options;
  initial: Broadcast | null;
  onSaved: (broadcast: Broadcast) => void;
  onDeleted: () => void;
};

export function Composer({ options, initial, onSaved, onDeleted }: Props) {
  const { me } = useSession();
  const limits = options.limits;
  const start: Audience = initial?.audience ?? { kind: 'all' };
  const [id, setId] = useState<number | null>(initial?.id ?? null);
  const [channel, setChannel] = useState<Channel>(initial?.channel ?? 'bot');
  const [subject, setSubject] = useState(initial?.subject ?? '');
  const [text, setText] = useState(initial?.text ?? '');
  const [image, setImage] = useState<Image | null>(initial?.image ?? null);
  const [buttons, setButtons] = useState<ButtonDraft[]>(() => (initial ? initial.buttons.map(toDraft) : []));
  const [kind, setKind] = useState<AudienceKind>(start.kind);
  const [days, setDays] = useState(start.kind === 'expiring' ? String(start.days) : '3');
  const [tariff, setTariff] = useState<number | null>(start.kind === 'tariff' ? start.tariff_id : (options.tariffs[0]?.id ?? null));
  const [reach, setReach] = useState<Reach | null>(null);
  const [reachError, setReachError] = useState<string | null>(null);
  const [when, setWhen] = useState<'now' | 'later'>('now');
  const [later, setLater] = useState(() => localInput(nextHour()));
  const [confirm, setConfirm] = useState<'send' | 'delete' | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<Note | null>(null);
  const [tried, setTried] = useState(false);
  const [query, setQuery] = useState('');
  const [found, setFound] = useState<ClientRow[] | null>(null);
  const [findError, setFindError] = useState<string | null>(null);
  const [target, setTarget] = useState<Target | null>(storedTarget);
  const [testNote, setTestNote] = useState<Note | null>(null);
  const [mailTo, setMailTo] = useState(() => stored(MAIL_KEY));
  const [mailNote, setMailNote] = useState<Note | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const file = useRef<HTMLInputElement>(null);

  const dayCount = Number.parseInt(days, 10);
  const daysOk = /^\d+$/.test(days.trim()) && dayCount >= 1 && dayCount <= limits.days;
  const audience = kind === 'expiring' ? (daysOk ? audienceOf(kind, dayCount, tariff) : null) : audienceOf(kind, dayCount, tariff);
  const audienceKey = JSON.stringify(audience);

  const subjectValue = hasEmail(channel) ? subject.trim().replace(/\s+/g, ' ') : subject.trim() ? subject.trim().replace(/\s+/g, ' ') : null;
  const body = useMemo(
    () => ({ channel, subject: subjectValue, text, image_id: image?.id ?? null, buttons: buttons.map(toSpec), audience }),
    [channel, subjectValue, text, image, buttons, audience],
  );
  const bodyKey = JSON.stringify(body);
  const [savedKey, setSavedKey] = useState(() => (initial ? bodyKey : ''));
  const dirty = bodyKey !== savedKey;

  useEffect(() => {
    const wanted = JSON.parse(audienceKey) as Audience | null;
    if (!wanted) {
      setReach(null);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      api<Reach>(`${BASE}/audience`, { method: 'POST', body: { audience: wanted } })
        .then((value) => {
          if (alive) {
            setReach(value);
            setReachError(null);
          }
        })
        .catch((err) => {
          if (alive) {
            setReach(null);
            setReachError(failText(err, 'Не получилось посчитать получателей'));
          }
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [audienceKey]);

  useEffect(() => {
    const needle = query.trim();
    if (!needle) {
      setFound(null);
      setFindError(null);
      return;
    }
    const timer = window.setTimeout(() => {
      api<{ items: ClientRow[] }>(`/api/admin/clients?limit=6&q=${encodeURIComponent(needle)}`)
        .then((page) => {
          setFound(page.items);
          setFindError(null);
        })
        .catch((err) => setFindError(failText(err, 'Поиск не сработал')));
    }, 300);
    return () => window.clearTimeout(timer);
  }, [query]);

  const limit = image ? limits.caption : limits.text;
  const length = visibleLength(text);
  const markup = text.trim() ? unclosedTag(text) : null;
  const textProblem = !text.trim()
    ? tried
      ? 'Напишите текст сообщения'
      : null
    : markup
      ? `Разметка: ${markup}`
      : length > limit
        ? image
          ? `С картинкой Telegram показывает до ${limit} знаков. Сократите текст или уберите картинку`
          : `Telegram показывает до ${limit} знаков`
        : null;
  const buttonProblems = buttons.map((b) => buttonProblem(b, limits.button_text));
  const subjectLength = [...subject.trim()].length;
  const subjectProblem = !hasEmail(channel)
    ? null
    : !subject.trim()
      ? tried
        ? 'Напишите тему письма'
        : null
      : subjectLength > limits.subject
        ? `Тема длиннее ${limits.subject} знаков`
        : null;
  const subjectOk = !hasEmail(channel) || (subject.trim() !== '' && subjectLength <= limits.subject);
  const messageOk = text.trim() !== '' && !markup && length <= limit && buttonProblems.every((p) => p === null);
  const contentOk = messageOk && subjectOk;
  const mailOff = hasEmail(channel) && !options.mail.ready;
  const laterDate = new Date(later);
  const laterOk =
    when === 'now' ||
    (!Number.isNaN(laterDate.getTime()) &&
      laterDate.getTime() > Date.now() + 60_000 &&
      laterDate.getTime() < Date.now() + limits.schedule_days * 86_400_000);
  const total = reach ? reachTotal(reach, channel) : null;
  const nobody = total !== null && total === 0;
  const canSend = contentOk && audience !== null && laterOk && !mailOff && !(when === 'now' && nobody) && busy === null;

  const wrap = (before: string, after: string, fallback: string) => {
    const el = area.current;
    const startAt = el?.selectionStart ?? text.length;
    const endAt = el?.selectionEnd ?? text.length;
    const selected = text.slice(startAt, endAt) || fallback;
    setText(text.slice(0, startAt) + before + selected + after + text.slice(endAt));
    window.requestAnimationFrame(() => {
      if (el) {
        el.focus();
        el.setSelectionRange(startAt + before.length, startAt + before.length + selected.length);
      }
    });
  };

  const changeButton = (index: number, patch: Partial<ButtonDraft>) => {
    setButtons(buttons.map((b, i) => (i === index ? { ...b, ...patch } : b)));
  };

  const pickFile = async (picked: File | undefined) => {
    if (!picked) {
      return;
    }
    if (picked.type !== 'image/png' && picked.type !== 'image/jpeg') {
      setNote({ text: 'Нужна картинка JPEG или PNG', tone: 'bad' });
      return;
    }
    if (picked.size > limits.image_bytes) {
      setNote({ text: 'Файл больше 5 МБ', tone: 'bad' });
      return;
    }
    setBusy('upload');
    setNote(null);
    try {
      setImage(await upload<Image>(`${BASE}/images`, picked));
    } catch (err) {
      setNote({ text: failText(err, 'Картинка не загрузилась'), tone: 'bad' });
    } finally {
      setBusy(null);
      if (file.current) {
        file.current.value = '';
      }
    }
  };

  const persist = async (): Promise<Broadcast> => {
    const saved =
      id === null
        ? await api<Broadcast>(`${BASE}/broadcasts`, { method: 'POST', body })
        : await api<Broadcast>(`${BASE}/broadcasts/${id}`, { method: 'PUT', body });
    const normalized = {
      channel: saved.channel,
      subject: saved.subject,
      text: saved.text,
      image_id: saved.image?.id ?? null,
      buttons: saved.buttons.map((b) => toSpec(toDraft(b))),
      audience: saved.audience,
    };
    setId(saved.id);
    setSubject(saved.subject ?? '');
    setText(saved.text);
    setButtons(saved.buttons.map(toDraft));
    setSavedKey(JSON.stringify(normalized));
    onSaved(saved);
    return saved;
  };

  const save = async () => {
    setTried(true);
    if (!text.trim()) {
      setNote({ text: 'Напишите текст сообщения', tone: 'bad' });
      return;
    }
    if (!audience) {
      setNote({ text: 'Выберите, кому отправить', tone: 'bad' });
      return;
    }
    setBusy('save');
    setNote(null);
    try {
      await persist();
      setNote({ text: 'Черновик сохранён', tone: 'ok' });
    } catch (err) {
      setNote({ text: failText(err, 'Черновик не сохранился'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const send = async () => {
    setBusy('send');
    setNote(null);
    try {
      const saved = await persist();
      const atIso = when === 'later' ? laterDate.toISOString() : null;
      const started = await api<Broadcast>(`${BASE}/broadcasts/${saved.id}/send`, { method: 'POST', body: { at: atIso } });
      setConfirm(null);
      onSaved(started);
    } catch (err) {
      setConfirm(null);
      setNote({ text: failText(err, 'Рассылка не отправилась'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    if (id === null) {
      onDeleted();
      return;
    }
    setBusy('delete');
    try {
      await api(`${BASE}/broadcasts/${id}`, { method: 'DELETE' });
      onDeleted();
    } catch (err) {
      setConfirm(null);
      setNote({ text: failText(err, 'Черновик не удалился'), tone: 'bad' });
      setBusy(null);
    }
  };

  const test = async () => {
    if (!target) {
      return;
    }
    setTried(true);
    setBusy('test');
    setTestNote(null);
    try {
      const report = await api<{ result: 'sent' | 'blocked'; message: string }>(`${BASE}/test`, {
        method: 'POST',
        body: { client_id: target.id, text: body.text, image_id: body.image_id, buttons: body.buttons },
      });
      setTestNote(
        report.result === 'sent'
          ? { text: `Проверка отправлена: ${target.name}`, tone: 'ok' }
          : { text: report.message, tone: 'bad' },
      );
    } catch (err) {
      setTestNote({ text: failText(err, 'Проверка не отправилась'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const testMail = async () => {
    const address = mailTo.trim();
    if (!address) {
      return;
    }
    setTried(true);
    setBusy('mail');
    setMailNote(null);
    keep(MAIL_KEY, address);
    try {
      const report = await api<{ result: 'sent' | 'blocked'; message: string }>(`${BASE}/test`, {
        method: 'POST',
        body: { email: address, subject: subject, text: body.text, image_id: body.image_id, buttons: body.buttons },
      });
      setMailNote({ text: report.message, tone: 'ok' });
    } catch (err) {
      setMailNote({ text: failText(err, 'Письмо не отправилось'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const choose = (row: ClientRow) => {
    const picked = { id: row.client.id, name: displayName(row), handle: handleOf(row) };
    setTarget(picked);
    storeTarget(picked);
    setQuery('');
    setFound(null);
    setTestNote(null);
  };

  const imageSrc = image ? imageUrl(image.id) : null;
  const preview = (
    <ChannelPreview channel={channel} subject={subject} text={text} image={imageSrc} buttons={buttons.map(toSpec)} brand={me.brand_name} />
  );
  const recipients = reach === null ? null : reachText(reach, channel);
  const sendLabel =
    when === 'later'
      ? laterOk
        ? `Запланировать на ${at(laterDate.toISOString())}`
        : 'Запланировать'
      : total && total > 0
        ? channel === 'bot' && reach
          ? `Отправить ${clients(reach.recipients)}`
          : channel === 'email' && reach
            ? `Отправить на${'\u00a0'}${addresses(reach.emails)}`
            : reach
              ? `Отправить: ${count(reach.recipients)} в${'\u00a0'}бот, ${count(reach.emails)} на${'\u00a0'}почту`
              : `Отправить: ${count(total)}`
        : 'Отправить';

  return (
    <div className="bc-grid">
      <div className="bc-col">
        <section className="sec" aria-labelledby="bc-text-h">
          <div className="sec-head">
            <h2 id="bc-text-h" className="section-title">
              Сообщение
            </h2>
            <span className="muted">Текст с разметкой Telegram. Обычные эмодзи вставляются как есть.</span>
          </div>
          <div className="row">
            <span className="rl">Куда</span>
            <div className="rc">
              <span className="segs bc-channel" role="group" aria-label="Куда отправить">
                {CHANNELS.map((option) => (
                  <button
                    key={option.key}
                    type="button"
                    aria-pressed={channel === option.key}
                    disabled={option.key !== 'bot' && !options.mail.ready && channel !== option.key}
                    onClick={() => {
                      setChannel(option.key);
                      setNote(null);
                    }}
                  >
                    {option.title}
                  </button>
                ))}
              </span>
              {!options.mail.ready ? (
                <span className={`bc-channel-note ${mailOff ? 'warn-line' : 'cap'}`}>
                  {options.mail.reason ?? 'Почта не отправляет'}. <Link to="/admin/settings/mail">Настройки почты</Link>
                </span>
              ) : hasEmail(channel) ? (
                <span className="cap">Письмо соберётся из того же текста, картинки и кнопок. Внизу будет ссылка «Отписаться от рассылок».</span>
              ) : null}
            </div>
          </div>
          {hasEmail(channel) ? (
            <div className="row">
              <label className="rl" htmlFor="bc-subject">
                Тема письма
              </label>
              <div className="rc">
                <input
                  id="bc-subject"
                  className="field"
                  value={subject}
                  maxLength={limits.subject * 2}
                  placeholder="Например: Новые серверы в Нидерландах"
                  aria-invalid={subjectProblem !== null}
                  onChange={(e) => {
                    setSubject(e.target.value);
                    setNote(null);
                  }}
                />
                <span className="bc-count">
                  <span className={subjectProblem ? 'err' : 'cap'}>{subjectProblem ?? ''}</span>
                  <span className={`cap num ${subjectLength > limits.subject ? 'err' : ''}`}>
                    {count(subjectLength)} / {count(limits.subject)}
                  </span>
                </span>
              </div>
            </div>
          ) : null}
          <div className="row">
            <div className="rc">
              <div className="bc-tools" role="toolbar" aria-label="Оформление текста">
                <button type="button" className="bc-tool hov" onClick={() => wrap('<b>', '</b>', 'жирный')} aria-label="Жирный">
                  Ж
                </button>
                <button type="button" className="bc-tool hov" onClick={() => wrap('<i>', '</i>', 'курсив')} aria-label="Курсив">
                  <em>К</em>
                </button>
                <button type="button" className="bc-tool hov" onClick={() => wrap('<a href="https://">', '</a>', 'текст ссылки')}>
                  Ссылка
                </button>
                <button type="button" className="bc-tool hov" onClick={() => wrap('<tg-spoiler>', '</tg-spoiler>', 'скрытый текст')}>
                  Скрытый текст
                </button>
              </div>
              <textarea
                ref={area}
                id="bc-text"
                className="field bc-area"
                rows={8}
                value={text}
                placeholder="Например: Подключили новые серверы в Нидерландах — скорость выше, пинг ниже. Обновите подписку в приложении."
                aria-label="Текст сообщения"
                aria-invalid={textProblem !== null}
                aria-describedby="bc-text-cap"
                onChange={(e) => {
                  setText(e.target.value);
                  setNote(null);
                }}
                spellCheck
              />
              <span className="bc-count">
                <span id="bc-text-cap" className={textProblem ? 'err' : 'cap'}>
                  {textProblem ?? (image ? 'С картинкой текст становится подписью к ней' : '')}
                </span>
                <span className={`cap num ${length > limit ? 'err' : ''}`}>
                  {count(length)} / {count(limit)}
                </span>
              </span>
              <details className="bc-hint">
                <summary>Как оформить текст</summary>
                <ul>
                  <li>
                    <code>{'<b>жирный</b>'}</code>, <code>{'<i>курсив</i>'}</code>, <code>{'<u>подчёркнутый</u>'}</code>, <code>{'<s>зачёркнутый</s>'}</code>
                  </li>
                  <li>
                    <code>{'<a href="https://example.com">ссылка</a>'}</code>, <code>{'<tg-spoiler>скрытый</tg-spoiler>'}</code>,{' '}
                    <code>{'<blockquote>цитата</blockquote>'}</code>, <code>{'<code>код</code>'}</code>
                  </li>
                  <li>
                    Премиум-эмодзи: отправьте боту <code>/emoji</code> с нужными эмодзи и вставьте строку из ответа. Если Telegram их не примет, уйдут обычные.
                  </li>
                </ul>
              </details>
              <div className="bc-inline">{preview}</div>
            </div>
          </div>
        </section>

        <section className="sec" aria-labelledby="bc-image-h">
          <div className="sec-head">
            <h2 id="bc-image-h" className="section-title">
              Картинка
            </h2>
            <span className="muted">Необязательно. JPEG или PNG до 5 МБ, текст к ней — до {count(limits.caption)} знаков.</span>
          </div>
          <div className="row">
            <div className="rc">
              <input
                ref={file}
                className="sr-only"
                type="file"
                accept="image/png,image/jpeg"
                id="bc-file"
                onChange={(e) => void pickFile(e.target.files?.[0])}
                tabIndex={-1}
              />
              {image && imageSrc ? (
                <div className="bc-image">
                  <img src={imageSrc} alt="Картинка рассылки" className="bc-thumb" />
                  <span className="bc-image-info">
                    <span>
                      {image.width} × {image.height}
                    </span>
                    <span className="cap">
                      {image.content_type === 'image/png' ? 'PNG' : 'JPEG'}, {fileSize(image.size)}
                    </span>
                  </span>
                  <button type="button" className="btn-line hov" onClick={() => file.current?.click()} disabled={busy !== null}>
                    Заменить
                  </button>
                  <button type="button" className="bc-link bad" onClick={() => setImage(null)} disabled={busy !== null}>
                    Убрать
                  </button>
                </div>
              ) : (
                <span>
                  <button type="button" className="btn-line hov bc-upload" onClick={() => file.current?.click()} disabled={busy !== null}>
                    <Icon name="image" size={18} />
                    &nbsp;{busy === 'upload' ? 'Загружаем…' : 'Загрузить картинку'}
                  </button>
                </span>
              )}
            </div>
          </div>
        </section>

        <section className="sec" aria-labelledby="bc-buttons-h">
          <div className="sec-head">
            <h2 id="bc-buttons-h" className="section-title">
              Кнопки
            </h2>
            <span className="muted">
              Встают под сообщением, каждая в своей строке. Раздел бота открывается отдельным сообщением, сама рассылка остаётся в чате.
            </span>
          </div>
          <div className="row">
            <div className="rc bc-buttons">
              {buttons.map((button, index) => {
                const problem = buttonProblems[index];
                const sectionTitle = options.sections.find((s) => s.key === button.section);
                return (
                  <div key={index} className="bc-bwrap">
                    <div className="bc-button">
                      <input
                        className="field"
                        value={button.text}
                        maxLength={limits.button_text * 2}
                        placeholder="Надпись на кнопке"
                        aria-label={`Надпись кнопки ${index + 1}`}
                        aria-invalid={tried && problem !== null && !button.text.trim()}
                        onChange={(e) => changeButton(index, { text: e.target.value })}
                      />
                      <span className="segs" role="group" aria-label={`Что открывает кнопка ${index + 1}`}>
                        <button type="button" aria-pressed={button.kind === 'url'} onClick={() => changeButton(index, { kind: 'url' })}>
                          Ссылка
                        </button>
                        <button type="button" aria-pressed={button.kind === 'section'} onClick={() => changeButton(index, { kind: 'section' })}>
                          Раздел бота
                        </button>
                      </span>
                      {button.kind === 'url' ? (
                        <input
                          className="field mono bc-target"
                          value={button.url}
                          inputMode="url"
                          placeholder="https://t.me/ваш_канал"
                          aria-label={`Адрес кнопки ${index + 1}`}
                          onChange={(e) => changeButton(index, { url: e.target.value })}
                        />
                      ) : (
                        <select
                          className="field bc-target"
                          value={button.section}
                          aria-label={`Раздел бота для кнопки ${index + 1}`}
                          onChange={(e) => changeButton(index, { section: e.target.value as Section })}
                        >
                          {options.sections.map((section) => (
                            <option key={section.key} value={section.key}>
                              {section.available ? section.title : `${section.title} — модуль выключен`}
                            </option>
                          ))}
                        </select>
                      )}
                      <button
                        type="button"
                        className="bc-remove hov"
                        aria-label={`Убрать кнопку ${index + 1}`}
                        onClick={() => setButtons(buttons.filter((_, i) => i !== index))}
                      >
                        <Icon name="close" size={18} />
                      </button>
                    </div>
                    {problem && (tried || button.text.trim() || button.url.trim()) ? <span className="err">{problem}</span> : null}
                    {button.kind === 'section' && sectionTitle && !sectionTitle.available ? (
                      <span className="warn-line">Модуль раздела выключен: кнопка откроет главное меню</span>
                    ) : null}
                  </div>
                );
              })}
              {buttons.length < limits.buttons ? (
                <button
                  type="button"
                  className="btn-line hov bc-add"
                  onClick={() => setButtons([...buttons, { text: '', kind: 'url', url: '', section: 'menu' }])}
                >
                  Добавить кнопку
                </button>
              ) : (
                <span className="cap">Больше {limits.buttons} кнопок Telegram покажет неудобно</span>
              )}
            </div>
          </div>
        </section>

        <section className="sec" aria-labelledby="bc-who-h">
          <div className="sec-head">
            <h2 id="bc-who-h" className="section-title">
              Кому
            </h2>
            <span className="muted">
              Список получателей соберётся в момент отправки.
              {channel === 'bot'
                ? ' Тем, кто остановил бота, сообщение не уходит.'
                : channel === 'email'
                  ? ' Тем, кто отписался от рассылок, письмо не уходит.'
                  : ' Кто остановил бота, не получит сообщение в бот, кто отписался от рассылок — письмо.'}
            </span>
          </div>
          <div className="row">
            <div className="rc bc-opts" role="radiogroup" aria-label="Кому отправить">
              {AUDIENCES.map((option) => (
                <div key={option.kind}>
                  <label className="bc-opt">
                    <input type="radio" name="bc-audience" checked={kind === option.kind} onChange={() => setKind(option.kind)} />
                    <span className="bc-opt-text">
                      <span className="bc-opt-title">{option.title}</span>
                      <span className="cap">{option.hint}</span>
                    </span>
                  </label>
                  {option.kind === 'expiring' && kind === 'expiring' ? (
                    <span className="bc-extra">
                      <span>Не больше</span>
                      <input
                        className="fld"
                        value={days}
                        inputMode="numeric"
                        aria-label="Сколько дней осталось"
                        aria-invalid={!daysOk}
                        onChange={(e) => setDays(e.target.value.replace(/[^\d]/g, '').slice(0, 3))}
                      />
                      <span>дней до конца подписки</span>
                      {!daysOk ? <span className="err">от 1 до {limits.days}</span> : null}
                    </span>
                  ) : null}
                  {option.kind === 'tariff' && kind === 'tariff' ? (
                    <span className="bc-extra">
                      {options.tariffs.length ? (
                        <select
                          className="field"
                          value={tariff ?? ''}
                          aria-label="Тариф"
                          onChange={(e) => setTariff(e.target.value ? Number(e.target.value) : null)}
                        >
                          {options.tariffs.map((t) => (
                            <option key={t.id} value={t.id}>
                              {t.name}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <span className="err">Тарифов пока нет</span>
                      )}
                    </span>
                  ) : null}
                </div>
              ))}
            </div>
          </div>
          <div className="bc-reach" aria-live="polite">
            {reachError ? (
              <span className="err">{reachError}</span>
            ) : !audience ? (
              <span className="muted">Уточните аудиторию</span>
            ) : reach === null ? (
              <span className="muted">Считаем получателей…</span>
            ) : (
              <>
                {hasBot(channel) ? (
                  <span className="bc-reach-line">
                    <span>
                      В бот: <strong className="num">{count(reach.recipients)}</strong>
                    </span>
                    {reach.stopped > 0 ? <span className="muted">ещё у {count(reach.stopped)} бот остановлен — им не отправим</span> : null}
                  </span>
                ) : null}
                {hasEmail(channel) ? (
                  <span className="bc-reach-line">
                    <span>
                      На почту: <strong className="num">{count(reach.emails)}</strong>
                    </span>
                    {reach.unsubscribed > 0 ? <span className="muted">ещё {addresses(reach.unsubscribed)} отписались — им не отправим</span> : null}
                  </span>
                ) : null}
              </>
            )}
          </div>
        </section>

        <section className="sec" aria-labelledby="bc-test-h">
          <div className="sec-head">
            <h2 id="bc-test-h" className="section-title">
              Проверка
            </h2>
            <span className="muted">Отправьте рассылку себе и посмотрите, как она выглядит у клиента.</span>
          </div>
          {hasBot(channel) ? (
          <div className="row">
            <span className="rl">В бот, клиенту</span>
            <div className="rc">
              {target ? (
                <span className="bc-picked">
                  <strong>{target.name}</strong>
                  {target.handle ? <span className="muted">{target.handle}</span> : null}
                  <button
                    type="button"
                    className="bc-link"
                    onClick={() => {
                      setTarget(null);
                      storeTarget(null);
                      setTestNote(null);
                    }}
                  >
                    Сменить
                  </button>
                </span>
              ) : (
                <>
                  <input
                    className="field"
                    type="search"
                    value={query}
                    maxLength={200}
                    placeholder="Имя, @username, Telegram ID или email"
                    aria-label="Найти клиента для проверки"
                    onChange={(e) => setQuery(e.target.value)}
                  />
                  {findError ? <span className="err">{findError}</span> : null}
                  {found && found.length === 0 ? <span className="cap">Никого не нашли</span> : null}
                  {found && found.length > 0 ? (
                    <div className="bc-found">
                      {found.map((row) => (
                        <button key={row.client.id} type="button" className="hov" onClick={() => choose(row)} disabled={!row.telegram_id}>
                          <span>{displayName(row)}</span>
                          <span className="muted">{row.telegram_id ? handleOf(row) : 'нет Telegram'}</span>
                        </button>
                      ))}
                    </div>
                  ) : null}
                </>
              )}
              <span>
                <button type="button" className="btn-line hov" onClick={() => void test()} disabled={!target || !messageOk || busy !== null}>
                  {busy === 'test' ? 'Отправляем…' : 'Отправить в бот'}
                </button>
              </span>
              {testNote ? (
                <span role="status" className={`bc-result ${testNote.tone === 'plain' ? '' : testNote.tone}`}>
                  {testNote.text}
                </span>
              ) : null}
              {!options.bot ? <span className="warn-line">Бот не подключён: проверка не уйдёт, пока не задан токен</span> : null}
            </div>
          </div>
          ) : null}
          {hasEmail(channel) ? (
            <div className="row">
              <label className="rl" htmlFor="bc-mail-to">
                На почту, на адрес
              </label>
              <div className="rc">
                <input
                  id="bc-mail-to"
                  className="field"
                  type="email"
                  inputMode="email"
                  value={mailTo}
                  maxLength={254}
                  placeholder="you@example.com"
                  onChange={(e) => {
                    setMailTo(e.target.value);
                    setMailNote(null);
                  }}
                />
                <span>
                  <button
                    type="button"
                    className="btn-line hov"
                    onClick={() => void testMail()}
                    disabled={!mailTo.trim() || !contentOk || mailOff || busy !== null}
                  >
                    {busy === 'mail' ? 'Отправляем…' : 'Отправить письмо'}
                  </button>
                </span>
                {mailNote ? (
                  <span role="status" className={`bc-result ${mailNote.tone === 'plain' ? '' : mailNote.tone}`}>
                    {mailNote.text}
                  </span>
                ) : null}
              </div>
            </div>
          ) : null}
        </section>

        <section className="sec" aria-labelledby="bc-send-h">
          <div className="sec-head">
            <h2 id="bc-send-h" className="section-title">
              Отправка
            </h2>
            <span className="muted">
              {hasBot(channel) ? 'Telegram принимает около 25 сообщений в секунду: тысяча получателей — примерно минута. ' : ''}
              {hasEmail(channel) ? 'Письма уходят по два в секунду, чтобы почтовый сервис не начал отказывать: тысяча писем — около восьми минут.' : ''}
            </span>
          </div>
          <div className="row">
            <span className="rl">Когда</span>
            <div className="rc">
              <span className="segs" role="group" aria-label="Когда отправить">
                <button type="button" aria-pressed={when === 'now'} onClick={() => setWhen('now')}>
                  Сейчас
                </button>
                <button type="button" aria-pressed={when === 'later'} onClick={() => setWhen('later')}>
                  Запланировать
                </button>
              </span>
              {when === 'later' ? (
                <>
                  <input
                    className="field bc-when"
                    type="datetime-local"
                    value={later}
                    min={localInput(new Date(Date.now() + 120_000))}
                    max={localInput(new Date(Date.now() + limits.schedule_days * 86_400_000))}
                    aria-label="Дата и время отправки"
                    aria-invalid={!laterOk}
                    onChange={(e) => setLater(e.target.value)}
                  />
                  <span className={laterOk ? 'cap' : 'err'}>
                    {laterOk ? `Время этого устройства, ${zoneLabel()}` : `Выберите время позже текущего, не дальше ${limits.schedule_days} дней`}
                  </span>
                </>
              ) : null}
            </div>
          </div>
          {confirm === 'send' ? (
            <div className="bc-ask" role="group" aria-label="Подтверждение отправки">
              {when === 'now' ? (
                <p>
                  Сообщение уйдёт <strong>{recipients ?? 'получателям'}</strong> сразу после подтверждения.{' '}
                  <span className="muted">Отправку можно остановить, пока она идёт: у тех, кому уже ушло, сообщение останется.</span>
                </p>
              ) : (
                <p>
                  Сообщение уйдёт <strong>{at(laterDate.toISOString())}</strong>. Получатели соберутся в этот момент
                  {recipients ? `, сейчас оно ушло бы ${recipients}` : ''}. <span className="muted">До этого времени отправку можно отменить.</span>
                </p>
              )}
              <div className="bc-ask-btns">
                <button type="button" className="btn-main btn" onClick={() => void send()} disabled={busy !== null}>
                  {busy === 'send' ? 'Отправляем…' : when === 'now' ? 'Отправить' : 'Запланировать'}
                </button>
                <button type="button" className="btn-line hov" onClick={() => setConfirm(null)} disabled={busy !== null}>
                  Не отправлять
                </button>
              </div>
            </div>
          ) : confirm === 'delete' ? (
            <div className="bc-ask" role="group" aria-label="Подтверждение удаления">
              <p>Удалить черновик? Вернуть его не получится.</p>
              <div className="bc-ask-btns">
                <button type="button" className="btn-bad" onClick={() => void remove()} disabled={busy !== null}>
                  {busy === 'delete' ? 'Удаляем…' : 'Удалить'}
                </button>
                <button type="button" className="btn-line hov" onClick={() => setConfirm(null)} disabled={busy !== null}>
                  Оставить
                </button>
              </div>
            </div>
          ) : (
            <div className="bc-actions">
              <button
                type="button"
                className="btn-main btn"
                onClick={() => {
                  setTried(true);
                  if (canSend) {
                    setConfirm('send');
                  }
                }}
                disabled={busy !== null || (tried && !canSend)}
              >
                {sendLabel}
              </button>
              <button type="button" className="btn-line hov" onClick={() => void save()} disabled={busy !== null || (id !== null && !dirty)}>
                {busy === 'save' ? 'Сохраняем…' : 'Сохранить черновик'}
              </button>
              {id !== null ? (
                <button type="button" className="bc-link bad" onClick={() => setConfirm('delete')} disabled={busy !== null}>
                  Удалить черновик
                </button>
              ) : null}
              <span role="status" className={note?.tone === 'bad' ? 'bad' : undefined}>
                {note?.text ?? (when === 'now' && nobody ? 'В этой аудитории некому отправить' : id !== null && dirty ? 'Есть несохранённые изменения' : '')}
              </span>
            </div>
          )}
        </section>
      </div>
      <aside className="bc-side">{preview}</aside>
    </div>
  );
}
