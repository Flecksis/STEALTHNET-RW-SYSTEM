import { Fragment, useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { Link, useLocation, useNavigate } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, dateLong, dateTime, dayLabel, groupDigits, NBSP, timeHM } from '../../lib/format';
import { Switch } from '../ui';
import '../form.css';
import './backups.css';
import { SettingsLayout } from './SettingsLayout';

type Schedule =
  | { every: 'off' }
  | { every: 'hours'; hours: number }
  | { every: 'day'; at: string }
  | { every: 'week'; weekday: number; at: string };
type Telegram = { to: 'off' } | { to: 'alerts' } | { to: 'chat'; chat_id: number };
type Delivery = { state: 'sent' | 'failed'; error: string | null; parts: number | null; key: string | null; at: string | null };
type Counts = { clients: number; subscriptions: number; active_subscriptions: number | null; payments: number };
type Kind = 'auto' | 'manual' | 'before_restore' | 'restore';
type Copy = {
  name: string;
  size: number;
  created_at: string;
  kind: Kind;
  sealed: boolean;
  key_included: boolean | null;
  counts: Counts | null;
  app: string | null;
  telegram: Delivery | null;
  s3: Delivery | null;
  by: string | null;
};
type Job = {
  id: string;
  kind: 'backup' | 'restore';
  state: 'running' | 'done' | 'failed';
  stage: string;
  detail: string | null;
  started_at: string;
  finished_at: string | null;
  error: string | null;
  result: Record<string, unknown> | null;
};
type Run = {
  id: number;
  kind: Kind;
  state: 'running' | 'done' | 'failed';
  file: string | null;
  error: string | null;
  started_at: string;
  finished_at: string | null;
  details: Record<string, unknown>;
  by: string | null;
};
type S3View = {
  on: boolean;
  endpoint: string;
  region: string;
  bucket: string;
  prefix: string;
  access_key: string;
  virtual_host: boolean;
  secret_set: boolean;
};
type View = {
  config: { schedule: Schedule; keep: number; telegram: Telegram; s3: S3View | null; password_set: boolean; password_set_at: string | null };
  storage: { configured: boolean; writable: boolean; host_dir: string | null; used_bytes: number; free_bytes: number | null; problem: string | null };
  secret_key: boolean;
  bot: boolean;
  group: { title: string | null; forum: boolean; topic: string | null } | null;
  copies: Copy[];
  next_at: string | null;
  job: Job | null;
  last: Run | null;
  restores: Run[];
  offset_minutes: number;
  remna_sync_path: string;
  totp_required: boolean;
};
type Keys = 'same' | 'from_copy' | 'lost' | 'none' | 'no_server_key';
type Preview = {
  source: string;
  size: number;
  sealed: boolean;
  created_at: string;
  kind: Kind;
  app: string;
  current_app: string;
  same_version: boolean;
  migrations_to_apply: number;
  counts: Counts;
  tables: number;
  rows: number;
  remnawave: { source: string; users: number; linked: number; missing: number; error: string | null } | null;
  brand: string | null;
  public_url: string | null;
  keys: Keys;
  secrets_in_copy: number;
  warnings: string[];
};
type Upload = { id: string; size: number; sealed: boolean; name: string };
type Source = { kind: 'file'; name: string } | { kind: 'upload'; id: string };
type Tab = 'copies' | 'settings' | 'restore';

const TABS: Array<{ key: Tab; label: string }> = [
  { key: 'copies', label: 'Копии' },
  { key: 'settings', label: 'Настройки' },
  { key: 'restore', label: 'Восстановление' },
];
const KIND_TITLE: Record<Kind, string> = {
  auto: 'По расписанию',
  manual: 'Вручную',
  before_restore: 'Перед восстановлением',
  restore: 'Восстановление',
};
const WEEKDAYS = ['понедельник', 'вторник', 'среду', 'четверг', 'пятницу', 'субботу', 'воскресенье'];
const WEEKDAY_EVERY = ['каждый понедельник', 'каждый вторник', 'каждую среду', 'каждый четверг', 'каждую пятницу', 'каждую субботу', 'каждое воскресенье'];
const HOURS = [1, 2, 3, 4, 6, 8, 12];
const SHOWN = 12;
const UPLOAD_MAX = 16 * 1024 * 1024 * 1024;
const BACKUP_STAGES: Record<string, string> = {
  remnawave: 'Снимаем подписки из Remnawave',
  dump: 'Копируем базу',
  verify: 'Проверяем файл',
  telegram: 'Отправляем в Telegram',
  s3: 'Отправляем в S3',
};
const RESTORE_STEPS: Array<{ key: string; label: string }> = [
  { key: 'check', label: 'Проверка файла' },
  { key: 'pause', label: 'Остановка бота и приёма оплат' },
  { key: 'before', label: 'Копия текущей базы' },
  { key: 'load', label: 'Загрузка данных из копии' },
  { key: 'migrate', label: 'Обновление базы до этой версии' },
  { key: 'apply', label: 'Замена данных' },
  { key: 'keys', label: 'Ключи и секреты' },
  { key: 'finish', label: 'Запуск' },
];

function size(bytes: number): string {
  const kb = 1024;
  if (bytes < kb * kb) {
    return `${Math.max(1, Math.round(bytes / kb))}${NBSP}КБ`;
  }
  if (bytes < kb * kb * kb) {
    return `${(bytes / kb / kb).toFixed(1).replace('.', ',')}${NBSP}МБ`;
  }
  return `${(bytes / kb / kb / kb).toFixed(2).replace('.', ',')}${NBSP}ГБ`;
}

function utc(minutes: number): string {
  if (minutes === 0) {
    return 'UTC';
  }
  const abs = Math.abs(minutes);
  const tail = abs % 60 ? `:${String(abs % 60).padStart(2, '0')}` : '';
  return `UTC${minutes < 0 ? '−' : '+'}${Math.floor(abs / 60)}${tail}`;
}

const MONTHS = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];

function panelTime(iso: string, minutes: number): string {
  const at = new Date(Date.parse(iso) + minutes * 60_000);
  const now = new Date(Date.now() + minutes * 60_000);
  const day = (d: Date) => Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
  const diff = Math.round((day(at) - day(now)) / 86_400_000);
  const time = `${String(at.getUTCHours()).padStart(2, '0')}:${String(at.getUTCMinutes()).padStart(2, '0')}`;
  if (diff === 0) {
    return `сегодня в ${time}`;
  }
  if (diff === 1) {
    return `завтра в ${time}`;
  }
  return `${at.getUTCDate()}${NBSP}${MONTHS[at.getUTCMonth()] ?? ''} в ${time}`;
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function countsLine(c: Counts): string {
  return `${count(c.clients, 'клиент', 'клиента', 'клиентов')}, ${count(c.subscriptions, 'подписка', 'подписки', 'подписок')}, ${count(c.payments, 'оплата', 'оплаты', 'оплат')}`;
}

function scheduleText(s: Schedule): string {
  switch (s.every) {
    case 'off':
      return 'Копии по расписанию выключены';
    case 'hours':
      return s.hours === 1 ? 'Копия делается каждый час' : `Копия делается каждые ${count(s.hours, 'час', 'часа', 'часов')}`;
    case 'day':
      return `Копия делается каждый день в ${s.at}`;
    case 'week':
      return `Копия делается ${WEEKDAY_EVERY[s.weekday - 1] ?? 'раз в неделю'} в ${s.at}`;
  }
}

function joinAnd(parts: string[]): string {
  if (parts.length <= 1) {
    return parts.join('');
  }
  return `${parts.slice(0, -1).join(', ')} и ${parts[parts.length - 1]}`;
}

function deliveryLine(copy: Copy): { text: string; problems: string[] } {
  const places = ['на сервере'];
  const problems: string[] = [];
  if (copy.telegram?.state === 'sent') {
    places.push(copy.telegram.parts && copy.telegram.parts > 1 ? `в Telegram, ${count(copy.telegram.parts, 'часть', 'части', 'частей')}` : 'в Telegram');
  } else if (copy.telegram?.state === 'failed') {
    problems.push(`В Telegram не ушла: ${copy.telegram.error ?? 'ошибка'}`);
  }
  if (copy.s3?.state === 'sent') {
    places.push('в S3');
  } else if (copy.s3?.state === 'failed') {
    problems.push(`В S3 не легла: ${copy.s3.error ?? 'ошибка'}`);
  }
  const text = places.length === 1 ? 'Только на сервере' : joinAnd(places);
  return { text: text.charAt(0).toUpperCase() + text.slice(1), problems };
}

function Command({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  };
  return (
    <span className="bk-cmd">
      <code className="mono">{text}</code>
      <button type="button" className="bk-link" onClick={() => void copy()}>
        {copied ? 'Скопировано' : 'Скопировать'}
      </button>
    </span>
  );
}

type Confirm = { password: string; code: string };

function ConfirmFields({
  totp,
  value,
  onChange,
  busy,
  label = 'Ваш пароль',
}: {
  totp: boolean;
  value: Confirm;
  onChange: (next: Confirm) => void;
  busy: boolean;
  label?: string;
}) {
  return (
    <div className="bk-fields">
      <label>
        <span>{label}</span>
        <input
          className="field"
          type="password"
          autoComplete="current-password"
          value={value.password}
          onChange={(e) => onChange({ ...value, password: e.target.value })}
          disabled={busy}
        />
      </label>
      {totp ? (
        <label>
          <span>Код из приложения</span>
          <input
            className="field num"
            inputMode="numeric"
            autoComplete="one-time-code"
            value={value.code}
            onChange={(e) => onChange({ ...value, code: e.target.value.replace(/\D/g, '').slice(0, 6) })}
            disabled={busy}
          />
        </label>
      ) : null}
    </div>
  );
}

function confirmBody(c: Confirm, totp: boolean) {
  return { password: c.password, code: totp ? c.code : undefined };
}

function Lead({ view, onSettings }: { view: View; onSettings: () => void }) {
  const { config, copies, last } = view;
  const places: string[] = [];
  if (config.telegram.to !== 'off') {
    places.push('Telegram');
  }
  if (config.s3?.on) {
    places.push('S3');
  }
  const head = scheduleText(config.schedule);
  const sent = places.length && config.schedule.every !== 'off' ? ` и уходит в ${joinAnd(places)}` : '';
  const newest = copies[0];
  const failedLast = last && last.state === 'failed' && (!newest || Date.parse(last.started_at) > Date.parse(newest.created_at));
  return (
    <p className="bk-lead">
      {head}
      {sent}. На сервере хранится до {count(config.keep, 'копии', 'копий', 'копий')}
      {newest ? `, последняя — ${dateTime(newest.created_at).toLowerCase()}, проверена` : ', копий пока нет'}.{' '}
      {failedLast ? <span className="bad">Последняя попытка {dateTime(last.started_at).toLowerCase()} не удалась: {last.error}. </span> : null}
      <button type="button" className="bk-link" onClick={onSettings}>
        Изменить
      </button>
    </p>
  );
}

function JobLine({ job, view }: { job: Job | null; view: View }) {
  if (!job || job.kind !== 'backup') {
    return null;
  }
  if (job.state !== 'running' && (!job.finished_at || Date.now() - Date.parse(job.finished_at) > 10 * 60_000)) {
    return null;
  }
  if (job.state === 'running') {
    return (
      <span className="bk-status" role="status">
        <span className="bk-spin" aria-hidden="true" />
        {BACKUP_STAGES[job.stage] ?? 'Копия делается'}…
      </span>
    );
  }
  if (job.state === 'failed') {
    return (
      <span className="bk-status bad" role="status">
        Копия не сделана: {job.error}
      </span>
    );
  }
  const file = typeof job.result?.file === 'string' ? job.result.file : null;
  const copy = file ? view.copies.find((c) => c.name === file) : null;
  const problems = copy ? deliveryLine(copy).problems : [];
  return (
    <span className={problems.length ? 'bk-status warn' : 'bk-status'} role="status">
      {problems.length ? `Копия готова, но ${problems.join('; ').toLowerCase()}` : 'Копия готова и проверена'}
    </span>
  );
}

function CopiesTab({
  view,
  busy,
  onRestore,
  onDelete,
  onResend,
}: {
  view: View;
  busy: string | null;
  onRestore: (name: string) => void;
  onDelete: (name: string) => void;
  onResend: (name: string) => void;
}) {
  const [all, setAll] = useState(false);
  const [asking, setAsking] = useState<string | null>(null);
  const shown = all ? view.copies : view.copies.slice(0, SHOWN);
  if (!view.copies.length) {
    return (
      <div className="bk-empty">
        <p>Копий на сервере пока нет.</p>
        <p className="cap">
          {view.next_at ? `Первая копия по расписанию — ${panelTime(view.next_at, view.offset_minutes)} (${utc(view.offset_minutes)}). ` : ''}
          Сделать копию сразу можно кнопкой «Сделать копию сейчас».
        </p>
      </div>
    );
  }
  const groups: Array<{ day: string; items: Copy[] }> = [];
  for (const copy of shown) {
    const day = dayLabel(copy.created_at);
    const group = groups[groups.length - 1];
    if (group && group.day === day) {
      group.items.push(copy);
    } else {
      groups.push({ day, items: [copy] });
    }
  }
  return (
    <div className="bk-list">
      {groups.map((group) => (
        <div key={group.day} className="bk-group">
          <h3 className="bk-day">{group.day}</h3>
          {group.items.map((copy) => {
            const where = deliveryLine(copy);
            const what = [
              copy.by ? `${KIND_TITLE[copy.kind]} — ${copy.by}` : KIND_TITLE[copy.kind],
              size(copy.size),
              copy.sealed ? 'с паролем' : 'без пароля',
            ].join(', ');
            return (
              <div key={copy.name} className="bk-line">
                <span className="bk-time num">{timeHM(copy.created_at)}</span>
                <span className="bk-desc">
                  <span>{what}</span>
                  <span className="cap">
                    {where.text}
                    {copy.counts ? ` · ${countsLine(copy.counts)}` : ''}
                  </span>
                  {where.problems.map((p) => (
                    <span key={p} className="cap bad">
                      {p}
                    </span>
                  ))}
                  {asking === copy.name ? (
                    <span className="bk-ask">
                      <span>Удалить копию с сервера? Копии в Telegram и S3 останутся.</span>
                      <button type="button" className="bk-link bad" disabled={busy !== null} onClick={() => onDelete(copy.name)}>
                        Удалить
                      </button>
                      <button type="button" className="bk-link" onClick={() => setAsking(null)}>
                        Не удалять
                      </button>
                    </span>
                  ) : null}
                </span>
                <span className="bk-acts">
                  <a className="bk-link" href={`/api/admin/backups/files/${encodeURIComponent(copy.name)}`} download={copy.name}>
                    Скачать
                  </a>
                  <button type="button" className="bk-link" disabled={busy !== null} onClick={() => onRestore(copy.name)}>
                    Восстановить
                  </button>
                  {where.problems.length ? (
                    <button type="button" className="bk-link" disabled={busy !== null} onClick={() => onResend(copy.name)}>
                      {busy === `send:${copy.name}` ? 'Отправляем…' : 'Отправить ещё раз'}
                    </button>
                  ) : null}
                  <button type="button" className="bk-link muted" disabled={busy !== null} onClick={() => setAsking(asking === copy.name ? null : copy.name)}>
                    Удалить
                  </button>
                </span>
              </div>
            );
          })}
        </div>
      ))}
      {view.copies.length > SHOWN && !all ? (
        <div className="bk-foot">
          <button type="button" className="bk-link" onClick={() => setAll(true)}>
            Показать ещё {view.copies.length - SHOWN}
          </button>
        </div>
      ) : null}
    </div>
  );
}

function ScheduleRows({ view, onSaved }: { view: View; onSaved: (v: View) => void }) {
  const [schedule, setSchedule] = useState<Schedule>(view.config.schedule);
  const [keep, setKeep] = useState(String(view.config.keep));
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ text: string; bad: boolean } | null>(null);
  const changed = JSON.stringify(schedule) !== JSON.stringify(view.config.schedule) || keep !== String(view.config.keep);
  const at = schedule.every === 'day' || schedule.every === 'week' ? schedule.at : '04:00';
  const pick = (every: Schedule['every']) => {
    if (every === 'off') {
      setSchedule({ every: 'off' });
    } else if (every === 'hours') {
      setSchedule({ every: 'hours', hours: 6 });
    } else if (every === 'day') {
      setSchedule({ every: 'day', at });
    } else {
      setSchedule({ every: 'week', weekday: 7, at });
    }
  };
  const save = async () => {
    const n = Number(keep);
    if (!Number.isInteger(n) || n < 1 || n > 100) {
      setNote({ text: 'Хранить можно от 1 до 100 копий', bad: true });
      return;
    }
    setBusy(true);
    setNote(null);
    try {
      onSaved(await api<View>('/api/admin/backups/settings', { method: 'PUT', body: { schedule, keep: n } }));
      setNote({ text: 'Сохранено', bad: false });
    } catch (err) {
      setNote({ text: failText(err, 'Не сохранилось'), bad: true });
    } finally {
      setBusy(false);
    }
  };
  const { storage } = view;
  return (
    <>
      <div className="row">
        <span className="rl">Расписание</span>
        <div className="rc">
          <span className="bk-inline">
            <select className="field bk-select" value={schedule.every} onChange={(e) => pick(e.target.value as Schedule['every'])} aria-label="Как часто">
              <option value="off">Выключено</option>
              <option value="hours">Каждые несколько часов</option>
              <option value="day">Каждый день</option>
              <option value="week">Раз в неделю</option>
            </select>
            {schedule.every === 'hours' ? (
              <select
                className="field bk-short"
                value={schedule.hours}
                onChange={(e) => setSchedule({ every: 'hours', hours: Number(e.target.value) })}
                aria-label="Через сколько часов"
              >
                {HOURS.map((h) => (
                  <option key={h} value={h}>
                    {count(h, 'час', 'часа', 'часов')}
                  </option>
                ))}
              </select>
            ) : null}
            {schedule.every === 'week' ? (
              <select
                className="field bk-short"
                value={schedule.weekday}
                onChange={(e) => setSchedule({ ...schedule, weekday: Number(e.target.value) })}
                aria-label="День недели"
              >
                {WEEKDAYS.map((d, i) => (
                  <option key={d} value={i + 1}>
                    в {d}
                  </option>
                ))}
              </select>
            ) : null}
            {schedule.every === 'day' || schedule.every === 'week' ? (
              <>
                <span>в</span>
                <input
                  className="field num bk-time-in"
                  type="time"
                  value={schedule.at}
                  onChange={(e) => setSchedule({ ...schedule, at: e.target.value })}
                  aria-label="Время копии"
                />
              </>
            ) : null}
          </span>
          <span className="cap">
            Время — по часовому поясу панели, {utc(view.offset_minutes)}. Копия делается без остановки панели и бота.
            {view.next_at ? ` Следующая — ${panelTime(view.next_at, view.offset_minutes)}.` : ''}
          </span>
        </div>
      </div>
      <div className="row">
        <span className="rl">Хранить на сервере</span>
        <div className="rc">
          <span className="bk-inline">
            <input
              className="fld num"
              inputMode="numeric"
              value={keep}
              onChange={(e) => setKeep(e.target.value.replace(/\D/g, '').slice(0, 3))}
              aria-label="Сколько копий хранить"
            />
            <span>
              {count(Number(keep) || 0, 'последнюю копию', 'последние копии', 'последних копий').replace(/^\d+\s/, '')}
              {storage.configured ? ` · занято ${size(storage.used_bytes)}${storage.free_bytes !== null ? `, свободно ${size(storage.free_bytes)}` : ''}` : ''}
            </span>
          </span>
          <span className="cap">
            {storage.host_dir ? `Папка ${storage.host_dir} на сервере. ` : 'Папка deploy/backups/panel рядом с установкой. '}
            Старая копия удаляется только после новой удачной.
          </span>
        </div>
      </div>
      <div className="row bk-save">
        <span className="rl" />
        <div className="rc">
          <span className="bk-inline">
            <button type="button" className="btn-main btn" disabled={!changed || busy} onClick={() => void save()}>
              {busy ? 'Сохраняем…' : 'Сохранить'}
            </button>
            {note ? <span className={note.bad ? 'err' : 'cap'}>{note.text}</span> : null}
          </span>
        </div>
      </div>
    </>
  );
}

function PasswordRow({ view, onSaved }: { view: View; onSaved: (v: View) => void }) {
  const [mode, setMode] = useState<'set' | 'remove' | null>(null);
  const [first, setFirst] = useState('');
  const [second, setSecond] = useState('');
  const [confirm, setConfirm] = useState<Confirm>({ password: '', code: '' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => {
    setMode(null);
    setFirst('');
    setSecond('');
    setConfirm({ password: '', code: '' });
    setError(null);
  };
  const save = async () => {
    if (mode === 'set') {
      if ([...first].length < 10) {
        setError('Пароль копий — не короче 10 символов');
        return;
      }
      if (first !== second) {
        setError('Пароли не совпадают');
        return;
      }
    }
    if (!confirm.password) {
      setError('Введите свой пароль');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      onSaved(
        await api<View>('/api/admin/backups/password', {
          method: 'PUT',
          body: { password: mode === 'set' ? first : null, confirm: confirmBody(confirm, view.totp_required) },
        }),
      );
      close();
    } catch (err) {
      setError(failText(err, 'Не сохранилось'));
    } finally {
      setBusy(false);
    }
  };
  const set = view.config.password_set;
  return (
    <div className="row">
      <span className="rl">Пароль копий</span>
      <div className="rc">
        {!view.secret_key ? (
          <span className="warn">На сервере не задан ключ шифрования SN_SECRET_KEY: пароль копий сохранить нельзя</span>
        ) : (
          <span className="bk-inline">
            <span className={set ? 'ink2' : 'muted'}>
              {set ? (view.config.password_set_at ? `задан ${dateLong(view.config.password_set_at)}` : 'задан') : 'не задан'}
            </span>
            {mode === null ? (
              <>
                <button type="button" className="btn-line hov" onClick={() => setMode('set')}>
                  {set ? 'Сменить' : 'Задать пароль'}
                </button>
                {set ? (
                  <button type="button" className="btn-line hov" onClick={() => setMode('remove')}>
                    Убрать
                  </button>
                ) : null}
              </>
            ) : null}
          </span>
        )}
        {mode ? (
          <div className="bk-form">
            {mode === 'set' ? (
              <div className="bk-fields">
                <label>
                  <span>Новый пароль копий</span>
                  <input className="field" type="password" autoComplete="new-password" value={first} onChange={(e) => setFirst(e.target.value)} disabled={busy} />
                </label>
                <label>
                  <span>Ещё раз</span>
                  <input className="field" type="password" autoComplete="new-password" value={second} onChange={(e) => setSecond(e.target.value)} disabled={busy} />
                </label>
              </div>
            ) : (
              <p>Новые копии будут без пароля, и ключа шифрования в них не будет.</p>
            )}
            <ConfirmFields totp={view.totp_required} value={confirm} onChange={setConfirm} busy={busy} />
            {error ? (
              <p className="err" role="alert">
                {error}
              </p>
            ) : null}
            <span className="bk-inline">
              <button type="button" className="btn-main btn" disabled={busy} onClick={() => void save()}>
                {busy ? 'Сохраняем…' : mode === 'set' ? 'Сохранить пароль' : 'Убрать пароль'}
              </button>
              <button type="button" className="btn-line hov" disabled={busy} onClick={close}>
                Отменить
              </button>
            </span>
          </div>
        ) : null}
        <span className="cap">
          С паролем копия зашифрована, и в неё кладётся ключ шифрования секретов: на другом сервере ключи платёжек и токен бота восстановятся. Без пароля ключа в
          копии нет — их придётся ввести заново. Забытый пароль не восстановить: запишите его там, где храните другие пароли.
        </span>
      </div>
    </div>
  );
}

function TelegramRow({ view, onSaved }: { view: View; onSaved: (v: View) => void }) {
  const current = view.config.telegram;
  const [to, setTo] = useState<Telegram['to']>(current.to);
  const [chat, setChat] = useState(current.to === 'chat' ? String(current.chat_id) : '');
  const [confirm, setConfirm] = useState<Confirm>({ password: '', code: '' });
  const [busy, setBusy] = useState<'save' | 'check' | null>(null);
  const [note, setNote] = useState<{ text: string; bad: boolean } | null>(null);
  const target: Telegram = to === 'chat' ? { to: 'chat', chat_id: Number(chat) } : to === 'alerts' ? { to: 'alerts' } : { to: 'off' };
  const changed = JSON.stringify(target) !== JSON.stringify(current);
  const needsConfirm = changed && to !== 'off';
  const save = async () => {
    if (to === 'chat' && !/^-?\d{5,20}$/.test(chat.trim())) {
      setNote({ text: 'Номер чата — число, у групп и каналов он начинается с -100', bad: true });
      return;
    }
    if (needsConfirm && !confirm.password) {
      setNote({ text: 'Введите свой пароль', bad: true });
      return;
    }
    setBusy('save');
    setNote(null);
    try {
      onSaved(
        await api<View>('/api/admin/backups/telegram', {
          method: 'PUT',
          body: { telegram: target, confirm: needsConfirm ? confirmBody(confirm, view.totp_required) : undefined },
        }),
      );
      setConfirm({ password: '', code: '' });
      setNote({ text: 'Сохранено', bad: false });
    } catch (err) {
      setNote({ text: failText(err, 'Не сохранилось'), bad: true });
    } finally {
      setBusy(null);
    }
  };
  const check = async () => {
    setBusy('check');
    setNote(null);
    try {
      const result = await api<{ message: string }>('/api/admin/backups/telegram/check', { method: 'POST', body: {} });
      setNote({ text: result.message, bad: false });
    } catch (err) {
      setNote({ text: failText(err, 'Проверка не прошла'), bad: true });
    } finally {
      setBusy(null);
    }
  };
  const group = view.group;
  return (
    <div className="row">
      <span className="rl">Отправлять в Telegram</span>
      <div className="rc">
        <div className="bk-radio" role="radiogroup" aria-label="Куда отправлять копии">
          <label>
            <input type="radio" name="bk-tg" checked={to === 'alerts'} onChange={() => setTo('alerts')} />
            <span>
              В группу уведомлений{group?.forum ? `, тема «${group.topic ?? 'Резервные копии'}»` : ''}
              <span className="cap">
                {group ? (
                  `Группа «${group.title ?? 'без названия'}» подключена в разделе «Уведомления в Telegram»`
                ) : (
                  <>
                    Группа не подключена: <Link to="/admin/settings/alerts">подключить в «Уведомлениях в Telegram»</Link>
                  </>
                )}
              </span>
            </span>
          </label>
          <label>
            <input type="radio" name="bk-tg" checked={to === 'chat'} onChange={() => setTo('chat')} />
            <span>В другой чат или канал</span>
          </label>
          {to === 'chat' ? (
            <label className="bk-chat">
              <span className="cap">Номер чата. Добавьте бота в чат или канал администратором, иначе он не сможет отправить файл</span>
              <input className="field mono" inputMode="numeric" placeholder="-1001234567890" value={chat} onChange={(e) => setChat(e.target.value.trim())} />
            </label>
          ) : null}
          <label>
            <input type="radio" name="bk-tg" checked={to === 'off'} onChange={() => setTo('off')} />
            <span>Не отправлять</span>
          </label>
        </div>
        {needsConfirm ? <ConfirmFields totp={view.totp_required} value={confirm} onChange={setConfirm} busy={busy !== null} /> : null}
        <span className="bk-inline">
          {changed ? (
            <button type="button" className="btn-main btn" disabled={busy !== null} onClick={() => void save()}>
              {busy === 'save' ? 'Сохраняем…' : 'Сохранить'}
            </button>
          ) : current.to !== 'off' ? (
            <button type="button" className="btn-line hov" disabled={busy !== null} onClick={() => void check()}>
              {busy === 'check' ? 'Отправляем…' : 'Отправить проверочное сообщение'}
            </button>
          ) : null}
          {note ? <span className={note.bad ? 'err' : 'cap'}>{note.text}</span> : null}
        </span>
        {!view.bot ? <span className="warn">Бот не подключён: копии в Telegram не уйдут, пока его не подключить в «Настройки → Бот»</span> : null}
        <span className="cap">Telegram принимает файлы до 50{NBSP}МБ: копию больше бот пришлёт частями, в подписи будет сказано, как их собрать.</span>
      </div>
    </div>
  );
}

type S3Form = { on: boolean; endpoint: string; region: string; bucket: string; prefix: string; access_key: string; secret_key: string; virtual_host: boolean };

function S3Row({ view, onSaved }: { view: View; onSaved: (v: View) => void }) {
  const saved = view.config.s3;
  const initial: S3Form = {
    on: saved?.on ?? false,
    endpoint: saved?.endpoint ?? '',
    region: saved?.region ?? '',
    bucket: saved?.bucket ?? '',
    prefix: saved?.prefix ?? '',
    access_key: saved?.access_key ?? '',
    secret_key: '',
    virtual_host: saved?.virtual_host ?? false,
  };
  const [form, setForm] = useState<S3Form>(initial);
  const [confirm, setConfirm] = useState<Confirm>({ password: '', code: '' });
  const [busy, setBusy] = useState<'save' | 'check' | null>(null);
  const [note, setNote] = useState<{ text: string; bad: boolean } | null>(null);
  const changed = JSON.stringify({ ...form, secret_key: '' }) !== JSON.stringify(initial) || form.secret_key !== '';
  const turningOff = !!saved?.on && !form.on;
  const set = (patch: Partial<S3Form>) => setForm((f) => ({ ...f, ...patch }));
  const body = () => ({
    on: form.on,
    endpoint: form.endpoint,
    region: form.region,
    bucket: form.bucket,
    prefix: form.prefix,
    access_key: form.access_key,
    secret_key: form.secret_key || undefined,
    virtual_host: form.virtual_host,
  });
  const save = async () => {
    if (!turningOff && !confirm.password) {
      setNote({ text: 'Введите свой пароль', bad: true });
      return;
    }
    setBusy('save');
    setNote(null);
    try {
      const next = await api<View>('/api/admin/backups/s3', {
        method: 'PUT',
        body: turningOff ? { s3: null } : { s3: body(), confirm: confirmBody(confirm, view.totp_required) },
      });
      onSaved(next);
      setForm((f) => ({ ...f, secret_key: '', on: next.config.s3?.on ?? false }));
      setConfirm({ password: '', code: '' });
      setNote({ text: 'Сохранено', bad: false });
    } catch (err) {
      setNote({ text: failText(err, 'Не сохранилось'), bad: true });
    } finally {
      setBusy(null);
    }
  };
  const check = async () => {
    setBusy('check');
    setNote(null);
    try {
      const result = await api<{ message: string }>('/api/admin/backups/s3/check', { method: 'POST', body: { s3: body() } });
      setNote({ text: result.message, bad: false });
    } catch (err) {
      setNote({ text: failText(err, 'Проверка не прошла'), bad: true });
    } finally {
      setBusy(null);
    }
  };
  const field = (key: keyof S3Form, label: string, placeholder = '', mono = true) => (
    <label>
      <span>{label}</span>
      <input
        className={mono ? 'field mono' : 'field'}
        value={String(form[key])}
        placeholder={placeholder}
        onChange={(e) => set({ [key]: e.target.value } as Partial<S3Form>)}
        disabled={busy !== null}
      />
    </label>
  );
  return (
    <div className="row">
      <span className="rl">S3-хранилище</span>
      <div className="rc">
        <span className="bk-inline">
          <Switch on={form.on} onToggle={() => set({ on: !form.on })} label="Отправлять копии в S3" />
          <span>{form.on ? 'Отправлять копии в S3' : 'Не отправлять'}</span>
        </span>
        {form.on || saved ? (
          <>
            <div className="bk-grid2">
              {field('endpoint', 'Адрес хранилища', 'https://s3.example.com')}
              {field('region', 'Регион', 'us-east-1')}
              {field('bucket', 'Бакет', 'stealthnet-backups')}
              {field('prefix', 'Папка в бакете', 'panel/')}
              {field('access_key', 'Ключ доступа')}
              <label>
                <span>Секретный ключ</span>
                <input
                  className="field"
                  type="password"
                  autoComplete="off"
                  value={form.secret_key}
                  placeholder={saved?.secret_set ? 'сохранён, введите, чтобы заменить' : ''}
                  onChange={(e) => set({ secret_key: e.target.value })}
                  disabled={busy !== null}
                />
              </label>
            </div>
            <label className="bk-check-line">
              <input type="checkbox" checked={form.virtual_host} onChange={(e) => set({ virtual_host: e.target.checked })} />
              <span>Бакет в имени домена (bucket.s3.example.com) — для AWS и тех, кто не принимает бакет в пути</span>
            </label>
          </>
        ) : null}
        {changed && !turningOff ? <ConfirmFields totp={view.totp_required} value={confirm} onChange={setConfirm} busy={busy !== null} /> : null}
        <span className="bk-inline">
          {changed ? (
            <button type="button" className="btn-main btn" disabled={busy !== null} onClick={() => void save()}>
              {busy === 'save' ? 'Сохраняем…' : 'Сохранить'}
            </button>
          ) : null}
          {form.on || saved ? (
            <button type="button" className="btn-line hov" disabled={busy !== null || !form.endpoint || !form.bucket} onClick={() => void check()}>
              {busy === 'check' ? 'Проверяем…' : 'Проверить подключение'}
            </button>
          ) : null}
          {note ? <span className={note.bad ? 'err' : 'cap'}>{note.text}</span> : null}
        </span>
        <span className="cap">
          Подойдёт любое S3-совместимое хранилище: Backblaze B2, Cloudflare R2, Яндекс, Selectel, MinIO. Старые копии в хранилище панель не удаляет — настройте срок
          хранения правилом жизненного цикла бакета.
        </span>
      </div>
    </div>
  );
}

function SettingsTab({ view, onSaved }: { view: View; onSaved: (v: View) => void }) {
  return (
    <div className="bk-settings">
      <ScheduleRows view={view} onSaved={onSaved} />
      <PasswordRow view={view} onSaved={onSaved} />
      <TelegramRow view={view} onSaved={onSaved} />
      <S3Row view={view} onSaved={onSaved} />
    </div>
  );
}

function keysText(keys: Keys): { text: string; tone: 'ok' | 'warn' | 'bad' | '' } {
  switch (keys) {
    case 'same':
      return { text: 'ключ шифрования тот же: ключи платёжек и токен бота сохранятся', tone: 'ok' };
    case 'from_copy':
      return { text: 'в копии есть ключ шифрования: ключи платёжек и токен бота восстановятся', tone: 'ok' };
    case 'none':
      return { text: 'секретов в копии нет', tone: '' };
    case 'no_server_key':
      return { text: 'на этом сервере не задан SN_SECRET_KEY: секреты из копии сохранить не получится', tone: 'bad' };
    default:
      return { text: 'ключа шифрования этого сервера в копии нет: ключи платёжек, токен бота и другие секреты придётся ввести заново', tone: 'warn' };
  }
}

type Uploading = { name: string; sent: number; total: number; part: number; parts: number };

function Steps({ job }: { job: Job }) {
  const at = RESTORE_STEPS.findIndex((s) => s.key === job.stage);
  const done = job.state === 'done';
  return (
    <ol className="bk-steps">
      {RESTORE_STEPS.map((step, i) => {
        const state = done || i < at ? 'done' : i === at ? (job.state === 'failed' ? 'failed' : 'running') : 'pending';
        return (
          <li key={step.key} className={`bk-step ${state}`}>
            <span>{step.label}</span>
            <span className="cap num">
              {state === 'done' ? 'готово' : null}
              {state === 'running' ? (job.detail ?? 'идёт') : null}
              {state === 'failed' ? 'не прошло' : null}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

function RestoreTab({
  view,
  chosen,
  clearChosen,
  onFinished,
}: {
  view: View;
  chosen: string | null;
  clearChosen: () => void;
  onFinished: () => void;
}) {
  const [source, setSource] = useState<Source | null>(null);
  const [label, setLabel] = useState('');
  const [sealed, setSealed] = useState(false);
  const [password, setPassword] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [confirm, setConfirm] = useState<Confirm>({ password: '', code: '' });
  const [busy, setBusy] = useState<'check' | 'start' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState<Uploading | null>(null);
  const [watch, setWatch] = useState<string | null>(null);
  const [job, setJob] = useState<Job | null>(null);
  const [over, setOver] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const xhr = useRef<XMLHttpRequest | null>(null);

  const reset = useCallback(() => {
    setSource(null);
    setLabel('');
    setSealed(false);
    setPassword('');
    setPreview(null);
    setConfirm({ password: '', code: '' });
    setError(null);
  }, []);

  const check = useCallback(async (src: Source, pass: string | null) => {
    setBusy('check');
    setError(null);
    try {
      setPreview(await api<Preview>('/api/admin/backups/preview', { method: 'POST', body: { source: src, password: pass || undefined } }));
    } catch (err) {
      if (err instanceof ApiError && (err.code === 'backups.password_needed' || err.code === 'backups.wrong_password')) {
        setSealed(true);
      }
      setError(failText(err, 'Копия не проверилась'));
    } finally {
      setBusy(null);
    }
  }, []);

  useEffect(() => {
    if (!chosen) {
      return;
    }
    const copy = view.copies.find((c) => c.name === chosen);
    clearChosen();
    reset();
    const src: Source = { kind: 'file', name: chosen };
    setSource(src);
    setLabel(copy ? `${KIND_TITLE[copy.kind].toLowerCase()}, ${dateTime(copy.created_at).toLowerCase()}` : chosen);
    setSealed(copy?.sealed ?? false);
    void check(src, null);
  }, [chosen, check, clearChosen, reset, view.copies]);

  useEffect(() => {
    if (!watch) {
      return;
    }
    let alive = true;
    const tick = async () => {
      try {
        const next = await api<Job>(`/api/admin/backups/restore/status?watch=${encodeURIComponent(watch)}`);
        if (alive) {
          setJob(next);
        }
      } catch {
        return;
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 1500);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [watch]);

  useEffect(() => {
    if (job && job.state !== 'running') {
      setWatch(null);
      if (job.state === 'done') {
        onFinished();
      }
    }
  }, [job, onFinished]);

  const sendPart = (file: File, url: string, part: number, parts: number) =>
    new Promise<Upload>((resolve, reject) => {
      const request = new XMLHttpRequest();
      xhr.current = request;
      request.open('POST', url);
      request.setRequestHeader('accept', 'application/json');
      request.upload.onprogress = (e) => {
        if (e.lengthComputable) {
          setUploading({ name: file.name, sent: e.loaded, total: e.total, part, parts });
        }
      };
      request.onload = () => {
        xhr.current = null;
        let data: unknown = null;
        try {
          data = JSON.parse(request.responseText);
        } catch {
          data = null;
        }
        if (request.status >= 200 && request.status < 300 && data) {
          resolve(data as Upload);
        } else {
          const message = (data as { error?: { message?: string } } | null)?.error?.message;
          reject(new Error(message ?? `Файл не загрузился (ошибка ${request.status})`));
        }
      };
      request.onerror = () => {
        xhr.current = null;
        reject(new Error('Загрузка оборвалась. Проверьте интернет и попробуйте ещё раз'));
      };
      request.onabort = () => {
        xhr.current = null;
        reject(new Error('Загрузка отменена'));
      };
      request.send(file);
    });

  const upload = async (list: FileList | null) => {
    const files = Array.from(list ?? []).sort((a, b) => a.name.localeCompare(b.name));
    const first = files[0];
    if (!first) {
      return;
    }
    const total = files.reduce((sum, f) => sum + f.size, 0);
    if (total === 0) {
      setError('Файл пустой');
      return;
    }
    if (total > UPLOAD_MAX) {
      setError('Файл больше 16 ГБ');
      return;
    }
    reset();
    const name = first.name.replace(/\.part\d{3}$/, '');
    let done: Upload | null = null;
    try {
      for (const [i, file] of files.entries()) {
        const url = done
          ? `/api/admin/backups/upload/${done.id}?name=${encodeURIComponent(name)}`
          : `/api/admin/backups/upload?name=${encodeURIComponent(name)}`;
        done = await sendPart(file, url, i + 1, files.length);
      }
    } catch (err) {
      setUploading(null);
      setError(err instanceof Error ? err.message : 'Файл не загрузился');
      return;
    }
    setUploading(null);
    if (!done) {
      return;
    }
    const src: Source = { kind: 'upload', id: done.id };
    setSource(src);
    setLabel(name);
    setSealed(done.sealed);
    if (!done.sealed) {
      void check(src, null);
    }
  };

  const cancel = async () => {
    if (source?.kind === 'upload') {
      try {
        await api(`/api/admin/backups/upload/${source.id}`, { method: 'DELETE' });
      } catch {
        return reset();
      }
    }
    reset();
  };

  const start = async () => {
    if (!source || !preview) {
      return;
    }
    if (!confirm.password) {
      setError('Введите свой пароль');
      return;
    }
    setBusy('start');
    setError(null);
    try {
      const result = await api<{ watch: string }>('/api/admin/backups/restore', {
        method: 'POST',
        body: { source, password: password || undefined, confirm: confirmBody(confirm, view.totp_required) },
      });
      setJob(null);
      setWatch(result.watch);
    } catch (err) {
      setError(failText(err, 'Восстановление не запустилось'));
    } finally {
      setBusy(null);
    }
  };

  if (job) {
    const result = job.result as {
      before?: string | null;
      warnings?: string[];
      created_at?: string;
      remna_sync_path?: string;
    } | null;
    return (
      <div className="bk-pane">
        {job.state === 'running' ? (
          <p className="strong" role="status">
            Идёт восстановление. Панель отвечает «идёт восстановление», бот и приём оплат стоят. Страницу можно не закрывать.
          </p>
        ) : job.state === 'done' ? (
          <p className="strong" role="status">
            Готово: база восстановлена{result?.created_at ? ` из копии от ${dateTime(result.created_at).toLowerCase()}` : ''}.
          </p>
        ) : (
          <p className="strong bad" role="alert">
            Восстановление не прошло, база осталась прежней: {job.error}
          </p>
        )}
        <Steps job={job} />
        {job.state === 'done' ? (
          <div className="bk-after">
            {result?.before ? <p className="cap">Копия базы до восстановления: {result.before}. Она есть в списке копий.</p> : null}
            {(result?.warnings ?? []).map((w) => (
              <p key={w} className="warn">
                {w}
              </p>
            ))}
            <p>
              Пароли администраторов теперь такие, как были в копии: если панель попросит войти, входите с ними. Если Remnawave потеряла пользователей или сроки в ней
              расходятся с панелью, перенесите клиентов и подписки в Remnawave.
            </p>
            <span className="bk-inline">
              <Link className="btn-main btn bk-go" to={result?.remna_sync_path ?? view.remna_sync_path}>
                Перенести клиентов и подписки в Remnawave
              </Link>
              <button type="button" className="btn-line hov" onClick={() => window.location.reload()}>
                Обновить страницу
              </button>
            </span>
          </div>
        ) : null}
        {job.state === 'failed' ? (
          <span className="bk-inline">
            <button
              type="button"
              className="btn-line hov"
              onClick={() => {
                setJob(null);
                reset();
              }}
            >
              Выбрать копию заново
            </button>
          </span>
        ) : null}
      </div>
    );
  }

  const keys = preview ? keysText(preview.keys) : null;
  return (
    <div className="bk-pane">
      {!source && !uploading ? (
        <div
          className={over ? 'bk-drop is-over' : 'bk-drop'}
          onDragOver={(e) => {
            e.preventDefault();
            setOver(true);
          }}
          onDragLeave={() => setOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setOver(false);
            void upload(e.dataTransfer.files);
          }}
        >
          <span className="strong">Перетащите файл копии сюда</span>
          <span className="cap">
            или выберите копию на вкладке «Копии». Подойдут файлы .tar.gz и .tar.gz.enc до 16{NBSP}ГБ, в том числе все части из Telegram сразу
          </span>
          <button type="button" className="btn-line hov" onClick={() => input.current?.click()}>
            Выбрать файл
          </button>
          <input
            ref={input}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              void upload(e.target.files);
              e.target.value = '';
            }}
          />
        </div>
      ) : null}
      {uploading ? (
        <div className="bk-drop is-busy">
          <span className="strong">{uploading.name}</span>
          <div
            className="bk-bar"
            role="progressbar"
            aria-valuenow={Math.floor((uploading.sent / Math.max(1, uploading.total)) * 100)}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-label="Загрузка файла"
          >
            <span style={{ width: `${Math.min(100, Math.floor((uploading.sent / Math.max(1, uploading.total)) * 100))}%` }} />
          </div>
          <span className="bk-inline">
            <span className="cap">
              {uploading.parts > 1 ? `Часть ${uploading.part} из ${uploading.parts} · ` : ''}
              {size(uploading.sent)} из {size(uploading.total)}
            </span>
            <button type="button" className="bk-link" onClick={() => xhr.current?.abort()}>
              Отменить
            </button>
          </span>
        </div>
      ) : null}
      {source && !preview ? (
        <div className="bk-check">
          <p className="strong">Копия: {label}</p>
          {sealed ? (
            <div className="bk-fields">
              <label>
                <span>Пароль копии</span>
                <input
                  className="field"
                  type="password"
                  autoComplete="off"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && password) {
                      void check(source, password);
                    }
                  }}
                  disabled={busy !== null}
                />
              </label>
            </div>
          ) : null}
          {busy === 'check' ? (
            <p className="bk-status" role="status">
              <span className="bk-spin" aria-hidden="true" />
              Проверяем копию: читаем файл целиком и сверяем контрольные суммы…
            </p>
          ) : null}
          {error ? (
            <p className="err" role="alert">
              {error}
            </p>
          ) : null}
          <span className="bk-inline">
            {sealed ? (
              <button type="button" className="btn-main btn" disabled={busy !== null || !password} onClick={() => void check(source, password)}>
                Проверить копию
              </button>
            ) : null}
            <button type="button" className="btn-line hov" disabled={busy !== null} onClick={() => void cancel()}>
              Отменить
            </button>
          </span>
        </div>
      ) : null}
      {preview && source ? (
        <div className="bk-check">
          <p className="strong">Копия {label} — файл целый</p>
          <dl className="bk-facts">
            <dt>Сделана</dt>
            <dd>
              {dateLong(preview.created_at, true)}, {timeHM(preview.created_at)}, версия {preview.app}.{' '}
              {preview.same_version ? 'Версия базы совпадает с этой панелью' : `После восстановления база обновится до ${preview.current_app}`}
            </dd>
            {preview.brand || preview.public_url ? (
              <>
                <dt>Панель</dt>
                <dd>{[preview.brand, preview.public_url].filter(Boolean).join(', ')}</dd>
              </>
            ) : null}
            <dt>Внутри</dt>
            <dd>
              {countsLine(preview.counts)}, настройки и файлы сайта · {groupDigits(preview.rows)} строк в {count(preview.tables, 'таблице', 'таблицах', 'таблицах')}
            </dd>
            {preview.remnawave ? (
              <>
                <dt>Снимок подписок</dt>
                <dd>
                  {preview.remnawave.source === 'remnawave'
                    ? `${count(preview.remnawave.linked, 'подписка', 'подписки', 'подписок')} с пользователями Remnawave: сроки, лимиты, сквады`
                    : `${count(preview.remnawave.linked, 'подписка', 'подписки', 'подписок')} по данным панели`}
                </dd>
              </>
            ) : null}
            <dt>Ключи платёжек</dt>
            <dd className={keys?.tone ?? ''}>{keys?.text}</dd>
          </dl>
          {preview.warnings
            .filter((w) => preview.keys !== 'lost' || !w.includes('придётся ввести заново'))
            .map((w) => (
              <p key={w} className="warn">
                {w}
              </p>
            ))}
          <p className="bk-note">
            Всё, что изменилось после копии ({dateTime(preview.created_at).toLowerCase()}), пропадёт: новые клиенты, оплаты и продления. Перед заменой панель
            сохранит копию текущей базы. Пока идёт восстановление, панель отвечает «идёт восстановление», бот и приём оплат не работают — обычно несколько минут.
          </p>
          <ConfirmFields totp={view.totp_required} value={confirm} onChange={setConfirm} busy={busy !== null} label="Ваш пароль для подтверждения" />
          {error ? (
            <p className="err" role="alert">
              {error}
            </p>
          ) : null}
          <span className="bk-inline">
            <button type="button" className="btn-main btn" disabled={busy !== null} onClick={() => void start()}>
              {busy === 'start' ? 'Запускаем…' : 'Восстановить'}
            </button>
            <button type="button" className="btn-line hov" disabled={busy !== null} onClick={() => void cancel()}>
              Отменить
            </button>
          </span>
        </div>
      ) : null}
      {view.restores.length ? (
        <div className="bk-history">
          <h3 className="bk-sub">Прошлые восстановления</h3>
          {view.restores.map((run) => (
            <div key={run.id} className="bk-hrow">
              <span className="num">{dateTime(run.started_at)}</span>
              <span className="bk-hwhat">{run.file ?? 'файл'}</span>
              <span className={run.state === 'done' ? 'ink2' : 'bad'}>{run.state === 'done' ? 'восстановлено' : `не прошло: ${run.error ?? 'ошибка'}`}</span>
              <span className="muted">{run.by ?? 'командой на сервере'}</span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

export function SettingsBackups() {
  const location = useLocation();
  const navigate = useNavigate();
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<{ text: string; bad: boolean } | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const hash = location.hash.replace('#', '');
  const tab: Tab = hash === 'settings' || hash === 'restore' ? hash : 'copies';
  const go = (next: Tab) => navigate({ hash: next === 'copies' ? '' : next }, { replace: true });

  const load = useCallback(async () => {
    try {
      setView(await api<View>('/api/admin/backups'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Сведения о копиях не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const running = view?.job?.kind === 'backup' && view.job.state === 'running';
  useEffect(() => {
    if (!running) {
      return;
    }
    const timer = window.setInterval(() => void load(), 2000);
    return () => window.clearInterval(timer);
  }, [running, load]);

  const clearChosen = useCallback(() => setChosen(null), []);
  const finished = useCallback(() => void load(), [load]);

  if (!view) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем сведения о копиях…</p>
        )}
      </SettingsLayout>
    );
  }

  const makeNow = async () => {
    setBusy('run');
    setNote(null);
    try {
      await api('/api/admin/backups/run', { method: 'POST', body: {} });
      await load();
    } catch (err) {
      setNote({ text: failText(err, 'Копия не запустилась'), bad: true });
    } finally {
      setBusy(null);
    }
  };

  const remove = async (name: string) => {
    setBusy(`delete:${name}`);
    setNote(null);
    try {
      setView(await api<View>(`/api/admin/backups/files/${encodeURIComponent(name)}`, { method: 'DELETE' }));
    } catch (err) {
      setNote({ text: failText(err, 'Копия не удалилась'), bad: true });
    } finally {
      setBusy(null);
    }
  };

  const resend = async (name: string) => {
    setBusy(`send:${name}`);
    setNote(null);
    try {
      setView(await api<View>(`/api/admin/backups/files/${encodeURIComponent(name)}/send`, { method: 'POST', body: {} }));
    } catch (err) {
      setNote({ text: failText(err, 'Не отправилось'), bad: true });
    } finally {
      setBusy(null);
    }
  };

  const problem: ReactNode = view.storage.problem ? (
    <div className="bk-problem" role="alert">
      <p>
        {view.storage.problem
          .replace(/\. Выполните на сервере: sudo stealthnet update$/, '.')
          .split('SN_BACKUP_DIR')
          .map((part, index) => (
            <Fragment key={index}>
              {index > 0 ? <code>SN_BACKUP_DIR</code> : null}
              {part}
            </Fragment>
          ))}
      </p>
      {view.storage.problem.includes('sudo stealthnet update') ? (
        <p className="cap">
          Выполните на сервере: <Command text="sudo stealthnet update" />
        </p>
      ) : null}
    </div>
  ) : null;

  return (
    <SettingsLayout>
      <div className="bk-top">
        <Lead view={view} onSettings={() => go('settings')} />
        {problem}
        <div className="bk-now">
          <button type="button" className="btn-main btn" disabled={running || busy !== null || !view.storage.configured} onClick={() => void makeNow()}>
            {running ? 'Копия делается…' : 'Сделать копию сейчас'}
          </button>
          <JobLine job={view.job} view={view} />
          {note ? <span className={note.bad ? 'err' : 'cap'}>{note.text}</span> : null}
        </div>
      </div>
      <section className="sec bk-card" aria-label="Резервные копии">
        <div className="bk-tabs" role="tablist">
          {TABS.map((t) => (
            <button key={t.key} type="button" role="tab" className="bk-tab" aria-selected={tab === t.key} onClick={() => go(t.key)}>
              {t.label}
            </button>
          ))}
        </div>
        {tab === 'copies' ? (
          <CopiesTab
            view={view}
            busy={busy}
            onRestore={(name) => {
              setChosen(name);
              go('restore');
            }}
            onDelete={(name) => void remove(name)}
            onResend={(name) => void resend(name)}
          />
        ) : null}
        {tab === 'settings' ? <SettingsTab view={view} onSaved={setView} /> : null}
        {tab === 'restore' ? <RestoreTab view={view} chosen={chosen} clearChosen={clearChosen} onFinished={finished} /> : null}
      </section>
      <p className="cap">
        Копии и восстановление доступны владельцу и администраторам с правом «Резервные копии». Каждое скачивание записывается в журнал действий.
      </p>
    </SettingsLayout>
  );
}
