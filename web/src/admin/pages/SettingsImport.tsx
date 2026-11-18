import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, dateTime, groupDigits, money, NBSP } from '../../lib/format';
import { usePhone } from '../../lib/media';
import '../form.css';
import './import.css';
import { ACCEPT, ACTIVE, COUNT_LABELS, MAX_UPLOAD, SOURCES, STAGES, type ImportView, type Run, type Source } from './importModel';
import { SettingsLayout } from './SettingsLayout';

function size(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) {
    return `${(bytes / 1024 / 1024 / 1024).toFixed(1).replace('.', ',')}${NBSP}ГБ`;
  }
  if (bytes >= 1024 * 1024) {
    return `${(bytes / 1024 / 1024).toFixed(bytes >= 100 * 1024 * 1024 ? 0 : 1).replace('.', ',')}${NBSP}МБ`;
  }
  return `${Math.max(1, Math.round(bytes / 1024))}${NBSP}КБ`;
}

function when(iso: string): string {
  return dateTime(iso).replace(/^Сегодня/, 'сегодня').replace(/^Вчера/, 'вчера');
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

type Uploading = { name: string; sent: number; total: number };

type Ctx = {
  view: ImportView;
  run: Run | null;
  source: Source | null;
  picked: boolean;
  pick: (source: Source) => void;
  next: () => void;
  back: () => void;
  uploading: Uploading | null;
  upload: (file: File) => void;
  cancelUpload: () => void;
  password: (value: string) => Promise<void>;
  start: () => Promise<void>;
  recheck: () => Promise<void>;
  discard: () => Promise<void>;
  again: () => void;
  busy: boolean;
  error: string | null;
};

function HowTo({ source }: { source: Source }) {
  if (source === 'stealthnet') {
    return (
      <ol className="imp-how">
        <li>
          Откройте админку старой панели, раздел <b>«Бэкапы»</b> (группа «Инфраструктура» в меню).
        </li>
        <li>
          В карточке «Создать бэкап» нажмите <b>«Создать и скачать»</b>. Скачается файл <span className="mono">stealthnet-backup-….sql</span>.
        </li>
        <li>Перетащите этот файл сюда. Подойдёт и автоматическая копия из списка «Сохранённые на сервере» — файл .sql.gz.</li>
      </ol>
    );
  }
  return (
    <ol className="imp-how">
      <li>
        В старом боте откройте <b>«Админ-панель» → «Система» → «Резервные копии»</b> и нажмите <b>«Создать бекап»</b>.
      </li>
      <li>
        Бот пришлёт файл <span className="mono">backup_….tar.gz</span> в чат для копий. Если файл не пришёл: «Админ-панель» → «Настройки» →
        «Конфигурация бота» → «Резервные копии», включите Backup Send Enabled и впишите свой Telegram ID в Backup Send Chat Id.
      </li>
      <li>Скачайте файл из Telegram и перетащите сюда. Если копия защищена паролем (файл .zip), спросим пароль.</li>
    </ol>
  );
}

function SourcePick({ value, onPick }: { value: Source | null; onPick: (s: Source) => void }) {
  return (
    <div className="imp-sources" role="radiogroup" aria-label="Откуда переносим">
      {SOURCES.map((s) => (
        <button
          key={s.id}
          type="button"
          role="radio"
          aria-checked={value === s.id}
          className="imp-source hov"
          onClick={() => onPick(s.id)}
        >
          <span className="imp-radio" aria-hidden="true" />
          <span className="imp-source-text">
            <span className="strong">{s.name}</span>
            <span className="cap">{s.what}</span>
          </span>
        </button>
      ))}
    </div>
  );
}

function DropZone({ ctx }: { ctx: Ctx }) {
  const input = useRef<HTMLInputElement>(null);
  const [over, setOver] = useState(false);
  const source = ctx.source ?? 'stealthnet';
  if (ctx.uploading) {
    const up = ctx.uploading;
    const pct = up.total > 0 ? Math.min(100, Math.floor((up.sent / up.total) * 100)) : 0;
    return (
      <div className="imp-drop is-busy">
        <span className="strong imp-name">{up.name}</span>
        <div className="imp-bar" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label="Загрузка файла">
          <span style={{ width: `${pct}%` }} />
        </div>
        <span className="imp-drop-row">
          <span className="cap">{pct < 100 ? `${size(up.sent)} из ${size(up.total)} · ${pct}%` : 'Файл загружен, сервер принимает его'}</span>
          {pct < 100 ? (
            <button type="button" className="imp-link" onClick={ctx.cancelUpload}>
              Отменить
            </button>
          ) : null}
        </span>
      </div>
    );
  }
  const take = (files: FileList | null) => {
    const file = files?.[0];
    if (file) {
      ctx.upload(file);
    }
  };
  return (
    <div
      className={over ? 'imp-drop is-over' : 'imp-drop'}
      onDragOver={(e) => {
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setOver(false);
        take(e.dataTransfer.files);
      }}
    >
      <span className="strong">Перетащите файл выгрузки сюда</span>
      <span className="cap">{source === 'stealthnet' ? 'Файл .sql или .sql.gz' : 'Файл .tar.gz, .tar или .zip'} до 2{NBSP}ГБ</span>
      <button type="button" className="btn-line hov" onClick={() => input.current?.click()}>
        Выбрать файл
      </button>
      <input
        ref={input}
        type="file"
        accept={ACCEPT[source]}
        hidden
        onChange={(e) => {
          take(e.target.files);
          e.target.value = '';
        }}
      />
    </div>
  );
}

function FileLine({ run }: { run: Run }) {
  return (
    <span className="imp-file">
      <span className="mono">{run.file.name}</span>
      <span className="muted">
        {run.file.size > 0 ? size(run.file.size) : ''}
        {run.preview?.dumped_at ? ` · копия от ${when(run.preview.dumped_at)}` : ''}
      </span>
    </span>
  );
}

function PasswordForm({ ctx }: { ctx: Ctx }) {
  const [value, setValue] = useState('');
  const wrong = ctx.run?.error;
  return (
    <form
      className="imp-password"
      onSubmit={(e) => {
        e.preventDefault();
        if (value.trim()) {
          void ctx.password(value);
        }
      }}
    >
      <label className="imp-field">
        <span className="strong">Архив защищён паролем</span>
        <span className="cap">
          Пароль задан в старом боте: «Конфигурация бота» → «Резервные копии», Backup Archive Password.
        </span>
        <input
          className="field"
          type="password"
          autoComplete="off"
          value={value}
          aria-invalid={Boolean(wrong)}
          onChange={(e) => setValue(e.target.value)}
          autoFocus
        />
      </label>
      {wrong ? (
        <p className="err" role="alert">
          {wrong}
        </p>
      ) : null}
      <div className="imp-actions-inline">
        <button type="submit" className="btn-main btn" disabled={ctx.busy || !value.trim()}>
          Открыть архив
        </button>
        <button type="button" className="btn-line hov" onClick={() => void ctx.discard()} disabled={ctx.busy}>
          Другой файл
        </button>
      </div>
    </form>
  );
}

function CheckView({ ctx, run }: { ctx: Ctx; run: Run }) {
  const p = run.preview;
  if (!p) {
    return null;
  }
  const total = p.remna.found + p.remna.missing + p.remna.ambiguous + (p.remna.shared ?? 0);
  return (
    <div className="imp-check">
      {p.detected !== ctx.source && ctx.source ? (
        <p className="imp-note">Файл — выгрузка {p.detected === 'bedolaga' ? 'бота Бедолаги' : 'STEALTHNET'}. Переносим из неё.</p>
      ) : null}
      {!p.remna.connected ? (
        <div className="imp-block" role="alert">
          <span className="strong">Сначала подключите Remnawave</span>
          <span>
            {p.remna.error
              ? `Панель не ответила: ${p.remna.error}. `
              : ''}
            Подключите ту же панель, что у старого бота, в{' '}
            <Link to="/admin/settings/remnawave">«Настройки → Remnawave»</Link>. Без неё не узнать, какие подписки уже есть, — а новые пользователи
            при переносе не создаются.
          </span>
          {ctx.view.remna ? (
            <button type="button" className="btn-line hov" onClick={() => void ctx.recheck()} disabled={ctx.busy}>
              Панель подключена — проверить снова
            </button>
          ) : null}
        </div>
      ) : null}
      <dl className="imp-counts">
        {COUNT_LABELS.map((c) => (
          <div key={c.key}>
            <dt>{c.label}</dt>
            <dd>{groupDigits(p.counts[c.key])}</dd>
          </div>
        ))}
      </dl>
      <ul className="imp-facts">
        <li>
          {p.balances.clients > 0
            ? `На балансах ${money({ minor: p.balances.total_minor, currency: p.balances.currency })} у ${count(p.balances.clients, 'клиента', 'клиентов', 'клиентов')}. Перенесутся одной записью на каждого клиента.`
            : 'Денег на балансах нет.'}
        </li>
        {p.balances.large_clients && p.balances.large_from_minor ? (
          <li className="imp-warn-line">
            Проверьте до запуска: у {count(p.balances.large_clients, 'клиента', 'клиентов', 'клиентов')} баланс больше{' '}
            {money({ minor: p.balances.large_from_minor, currency: p.balances.currency })} —{' '}
            {p.balances.large_by_tariff ? 'в десять раз дороже самого дорогого тарифа' : 'в десять раз больше, чем у 99% клиентов'}, всего{' '}
            {money({ minor: p.balances.large_total_minor ?? 0, currency: p.balances.currency })}. Если это ошибка, исправьте балансы в старой панели
            и загрузите копию заново, иначе они перенесутся как есть. Список — в отчёте.
          </li>
        ) : null}
        {p.remna.connected && p.remna.found === 0 && total > 0 ? (
          <li className="imp-warn-line">
            {p.remna.users === 0
              ? 'В подключённой Remnawave нет ни одного пользователя'
              : `Ни одна подписка не нашлась в подключённой Remnawave (пользователей в ней: ${groupDigits(p.remna.users)})`}{' '}
            — похоже, это не та панель, что у старого бота. Проверьте <Link to="/admin/settings/remnawave">«Настройки → Remnawave»</Link>: если
            перенести сейчас, клиенты перейдут без подписок.
          </li>
        ) : p.remna.connected ? (
          <li>
            Подписки привязываются к пользователям Remnawave, которые уже есть: нашлись {groupDigits(p.remna.found)} из {groupDigits(total)}. Ссылки у
            клиентов не меняются.
          </li>
        ) : null}
        {p.existing.matched > 0 ? (
          <li>
            {count(p.existing.matched, 'клиент уже есть', 'клиента уже есть', 'клиентов уже есть')} в этой установке — совпали по Telegram или почте.
            Их не задвоим: добавим подписки и баланс.
          </li>
        ) : null}
        {p.existing.again > 0 ? (
          <li>
            {count(p.existing.again, 'клиент уже переносился', 'клиента уже переносились', 'клиентов уже переносились')} раньше. Перенесётся только
            новое, балансы обновятся.
          </li>
        ) : null}
        {p.currency.switch ? (
          <li>
            Старый бот вёл деньги в {p.currency.source === 'RUB' ? 'рублях' : 'долларах'}. Валюта этой установки станет «
            {p.currency.source === 'RUB' ? 'Рубли' : 'Доллары'}» — в другой валюте здесь ещё ничего не продавали.
          </li>
        ) : null}
        {p.currency.blocked ? <li className="imp-warn-line">{p.currency.blocked}</li> : null}
        {p.tariff_mode === 'classic' ? (
          <li>
            В старом боте продавали без тарифов. Подписки разложим по тарифам «серверы и трафик», они будут сняты с продажи — цены задайте сами в
            «Тарифах».
          </li>
        ) : null}
      </ul>
      {p.skipped.length ? (
        <div className="imp-skips">
          <h3 className="strong">Что не перенесётся</h3>
          <ul>
            {p.skipped.map((s) => (
              <li key={s.code}>
                <span className="num">{groupDigits(s.count)}</span> {s.text}
              </li>
            ))}
          </ul>
          <a className="imp-link" href={`/api/admin/import/${run.id}/report`} download>
            Скачать список
          </a>
        </div>
      ) : null}
    </div>
  );
}

function RunningView({ run }: { run: Run }) {
  const at = STAGES.findIndex((s) => s.key === run.progress?.stage);
  return (
    <div className="imp-run">
      <ol className="imp-stages">
        {STAGES.map((s, i) => (
          <li key={s.key} className={i < at ? 'is-done' : i === at ? 'is-now' : undefined}>
            <span>{s.label}</span>
            {i === at && run.progress && run.progress.total > 1 ? (
              <span className="num muted">
                {groupDigits(run.progress.done)} из {groupDigits(run.progress.total)}
              </span>
            ) : i < at ? (
              <span className="muted">готово</span>
            ) : null}
          </li>
        ))}
      </ol>
      <p className="cap">Перенос идёт на сервере. Страницу можно закрыть — ход будет виден, когда вернётесь.</p>
    </div>
  );
}

function NextSteps({ ctx }: { ctx: Ctx }) {
  const bot = ctx.view.bot;
  return (
    <ol className="imp-next">
      <li>
        <span className="strong">Подключите того же Telegram-бота.</span>
        {bot === 'ok' ? (
          <span>Бот подключён и работает. Клиенты пишут тому же боту, что и раньше.</span>
        ) : (
          <span>
            В <Link to="/admin/settings/bot">«Настройки → Бот»</Link> вставьте токен старого бота. Клиенты продолжат писать тому же боту.
          </span>
        )}
        {bot === 'conflict' ? (
          <span className="imp-warn">
            Старый бот ещё работает: Telegram отдаёт сообщения ему, а новому отвечает отказом 409. Пока старый бот не выключен, этот работать не
            будет.
          </span>
        ) : null}
        {bot === 'webhook' ? (
          <span className="imp-warn">
            Бот привязан к сайту старого бота. Нажмите «Забрать бота сюда» в «Настройки → Бот», когда выключите старого.
          </span>
        ) : null}
      </li>
      <li>
        <span className="strong">Выключите старого бота.</span>
        <span>Остановите его на старом сервере. Данные там останутся — перенос их не трогал.</span>
      </li>
      <li>
        <span className="strong">Проверьте оплату.</span>
        <span>
          В <Link to="/admin/settings/payments">«Настройки → Оплата»</Link> подключите платёжные системы и проведите пробную оплату.
        </span>
      </li>
    </ol>
  );
}

function DoneView({ ctx, run }: { ctx: Ctx; run: Run }) {
  const o = run.outcome;
  if (!o) {
    return null;
  }
  return (
    <div className="imp-check">
      <dl className="imp-counts">
        {COUNT_LABELS.map((c) => (
          <div key={c.key}>
            <dt>{c.label}</dt>
            <dd>{groupDigits(o.counts[c.key])}</dd>
          </div>
        ))}
      </dl>
      <ul className="imp-facts">
        {o.merged > 0 ? <li>{count(o.merged, 'клиент объединён', 'клиента объединены', 'клиентов объединены')} с теми, кто уже был в установке.</li> : null}
        {o.updated > 0 ? <li>{count(o.updated, 'клиент обновлён', 'клиента обновлены', 'клиентов обновлены')} после прошлого переноса.</li> : null}
        {o.currency_switched ? <li>Валюта установки сменилась на валюту старого бота.</li> : null}
        <li>
          {run.issues > 0 ? (
            <>
              {count(run.issues, 'запись требует внимания', 'записи требуют внимания', 'записей требуют внимания')} — причины в отчёте.{' '}
              <a className="imp-link" href={`/api/admin/import/${run.id}/report`} download>
                Скачать отчёт
              </a>
            </>
          ) : (
            'Всё перенеслось без проблем.'
          )}
        </li>
      </ul>
      <h3 className="strong">Что сделать дальше</h3>
      <NextSteps ctx={ctx} />
      <p className="cap">Перед самым переключением можно загрузить свежую выгрузку: перенесётся только новое, дублей не будет.</p>
    </div>
  );
}

function Problem({ run }: { run: Run }) {
  if (run.state === 'interrupted' || (run.state === 'failed' && run.preview)) {
    return (
      <div className="imp-block" role="alert">
        <span className="strong">{run.state === 'interrupted' ? 'Перенос прервался' : 'Перенос остановился'}</span>
        <span>{run.error ?? 'Сервер перезапускался во время переноса.'} Уже перенесённое сохранено — запустите ещё раз, дублей не будет.</span>
      </div>
    );
  }
  return null;
}

type StepKey = 'source' | 'file' | 'check' | 'run' | 'done';

const STEP_TITLES: Record<StepKey, string> = {
  source: 'Откуда переносим',
  file: 'Файл выгрузки',
  check: 'Проверка',
  run: 'Перенос',
  done: 'Готово',
};

function stepOf(ctx: Ctx): StepKey {
  const run = ctx.run;
  if (ctx.uploading) {
    return 'file';
  }
  if (!run) {
    return ctx.source && ctx.picked ? 'file' : 'source';
  }
  switch (run.state) {
    case 'running':
      return 'run';
    case 'done':
      return 'done';
    case 'ready':
    case 'interrupted':
      return 'check';
    case 'failed':
      return run.preview ? 'check' : 'file';
    default:
      return 'file';
  }
}

function FileBody({ ctx }: { ctx: Ctx }) {
  const run = ctx.run;
  if (!ctx.uploading && run?.state === 'checking') {
    return (
      <div className="imp-drop is-busy">
        <FileLine run={run} />
        <div className="imp-bar is-wait" aria-hidden="true">
          <span />
        </div>
        <span className="cap">Проверяем файл: читаем клиентов и ищем их подписки в Remnawave. Это займёт до пары минут, страницу можно закрыть.</span>
      </div>
    );
  }
  if (!ctx.uploading && run?.state === 'needs_password') {
    return (
      <>
        <FileLine run={run} />
        <PasswordForm ctx={ctx} />
      </>
    );
  }
  return (
    <>
      {!ctx.uploading && run?.state === 'failed' && !run.preview ? (
        <p className="err" role="alert">
          {run.error ?? 'Файл не подошёл.'}
        </p>
      ) : null}
      <DropZone ctx={ctx} />
      {ctx.source ? (
        <details className="imp-hint" open={!run}>
          <summary>Где взять файл</summary>
          <HowTo source={ctx.source} />
        </details>
      ) : null}
    </>
  );
}

function canStart(run: Run | null): boolean {
  return Boolean(
    run?.preview?.remna.connected && (run.state === 'ready' || run.state === 'interrupted' || (run.state === 'failed' && run.preview)),
  );
}

function Intro() {
  return (
    <div className="head-stack">
      <h2 className="section-title">Перенос из другого бота</h2>
      <p className="muted">
        Клиенты, подписки, балансы и история оплат из старой системы. Старый бот при этом не меняется, клиентам ничего не приходит.
      </p>
    </div>
  );
}

function StartButton({ ctx }: { ctx: Ctx }) {
  const run = ctx.run;
  const label = run?.state === 'ready' ? 'Перенести' : 'Продолжить перенос';
  return (
    <button type="button" className="btn-main btn" onClick={() => void ctx.start()} disabled={ctx.busy || !canStart(run)}>
      {label}
    </button>
  );
}

const ORDER: StepKey[] = ['source', 'file', 'check', 'run', 'done'];

function Wizard({ ctx, phone }: { ctx: Ctx; phone: boolean }) {
  const step = stepOf(ctx);
  const at = ORDER.indexOf(step);
  const run = ctx.run;
  let body: ReactNode = null;
  let primary: ReactNode = null;
  let back: (() => void) | null = null;
  let note: string | null = null;
  if (step === 'source') {
    body = (
      <>
        <SourcePick value={ctx.source} onPick={ctx.pick} />
        {ctx.source ? <HowTo source={ctx.source} /> : null}
      </>
    );
    primary = (
      <button type="button" className="btn-main btn" disabled={!ctx.source} onClick={ctx.next}>
        Дальше
      </button>
    );
  } else if (step === 'file') {
    body = <FileBody ctx={ctx} />;
    if (!run && !ctx.uploading) {
      back = ctx.back;
    }
  } else if (step === 'check' && run) {
    body = (
      <>
        <FileLine run={run} />
        <Problem run={run} />
        <CheckView ctx={ctx} run={run} />
      </>
    );
    primary = <StartButton ctx={ctx} />;
    if (run.state === 'ready') {
      back = () => void ctx.discard();
    }
    note = 'Перенос можно повторить: дублей не будет.';
  } else if (step === 'run' && run) {
    body = <RunningView run={run} />;
  } else if (step === 'done' && run) {
    body = <DoneView ctx={ctx} run={run} />;
    primary = (
      <button type="button" className="btn-line hov" onClick={ctx.again}>
        Загрузить свежую выгрузку
      </button>
    );
  }
  return (
    <>
      <Intro />
      {phone ? (
        <p className="imp-phone-step">
          Шаг {at + 1} из {ORDER.length} · <span className="strong">{STEP_TITLES[step]}</span>
        </p>
      ) : (
        <ol className="imp-wizard" aria-label="Шаги переноса">
          {ORDER.map((key, i) => (
            <li key={key} className={i < at ? 'is-done' : i === at ? 'is-now' : undefined} aria-current={i === at ? 'step' : undefined}>
              <span className="imp-num" aria-hidden="true">
                {i + 1}
              </span>
              <span>{STEP_TITLES[key]}</span>
            </li>
          ))}
        </ol>
      )}
      <section className="sec imp-card" aria-label={STEP_TITLES[step]}>
        {phone ? <h3 className="imp-step-title">{STEP_TITLES[step]}</h3> : null}
        <div className="imp-step-body">{body}</div>
        {back || primary ? (
          <div className="imp-actions">
            {back ? (
              <button type="button" className="btn-line hov" onClick={back} disabled={ctx.busy}>
                Назад
              </button>
            ) : null}
            {primary}
            {note ? <span className="cap">{note}</span> : null}
          </div>
        ) : null}
      </section>
    </>
  );
}

export function SettingsImport() {
  const phone = usePhone();
  const [view, setView] = useState<ImportView | null>(null);
  const [picked, setPicked] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [source, setSource] = useState<Source | null>(null);
  const [uploading, setUploading] = useState<Uploading | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fresh, setFresh] = useState(false);
  const xhr = useRef<XMLHttpRequest | null>(null);

  const load = useCallback(async () => {
    try {
      const next = await api<ImportView>('/api/admin/import');
      setView(next);
      setLoadError(null);
    } catch (err) {
      setLoadError(failText(err, 'Не удалось загрузить состояние переноса'));
    }
  }, []);

  useEffect(() => {
    void load();
    return () => xhr.current?.abort();
  }, [load]);

  const state = view?.run?.state;
  useEffect(() => {
    if (!state || !ACTIVE.includes(state)) {
      return;
    }
    const timer = window.setInterval(() => void load(), 1500);
    return () => window.clearInterval(timer);
  }, [state, load]);

  const run = view && !(fresh && view.run?.state === 'done') ? view.run : null;
  const chosen = source ?? run?.source ?? null;

  const act = async (work: () => Promise<Run | void>, fallback: string) => {
    setBusy(true);
    setError(null);
    try {
      const next = await work();
      if (next) {
        setView((v) => (v ? { ...v, run: next } : v));
      }
      await load();
    } catch (err) {
      setError(failText(err, fallback));
    } finally {
      setBusy(false);
    }
  };

  const upload = (file: File) => {
    if (file.size > MAX_UPLOAD) {
      setError('Файл больше 2 ГБ. Нужна выгрузка базы, а не архив сервера целиком');
      return;
    }
    if (file.size === 0) {
      setError('Файл пустой');
      return;
    }
    const target = chosen ?? 'stealthnet';
    setError(null);
    setUploading({ name: file.name, sent: 0, total: file.size });
    const request = new XMLHttpRequest();
    xhr.current = request;
    request.open('POST', `/api/admin/import/upload?source=${target}&name=${encodeURIComponent(file.name)}`);
    request.setRequestHeader('accept', 'application/json');
    request.upload.onprogress = (e) => {
      if (e.lengthComputable) {
        setUploading({ name: file.name, sent: e.loaded, total: e.total });
      }
    };
    request.onload = () => {
      xhr.current = null;
      setUploading(null);
      let data: unknown = null;
      try {
        data = JSON.parse(request.responseText);
      } catch {
        data = null;
      }
      if (request.status >= 200 && request.status < 300 && data) {
        setFresh(false);
        setView((v) => (v ? { ...v, run: data as Run } : v));
        void load();
      } else {
        const message = (data as { error?: { message?: string } } | null)?.error?.message;
        setError(message ?? `Файл не загрузился (ошибка ${request.status})`);
      }
    };
    request.onerror = () => {
      xhr.current = null;
      setUploading(null);
      setError('Загрузка оборвалась. Проверьте интернет и попробуйте ещё раз');
    };
    request.onabort = () => {
      xhr.current = null;
      setUploading(null);
    };
    request.send(file);
  };

  const ctx: Ctx | null = view
    ? {
        view,
        run,
        source: chosen,
        picked,
        pick: (s) => {
          setSource(s);
          setError(null);
        },
        next: () => setPicked(true),
        back: () => {
          setPicked(false);
          setError(null);
        },
        uploading,
        upload,
        cancelUpload: () => xhr.current?.abort(),
        password: (value) =>
          act(() => api<Run>(`/api/admin/import/${run?.id}/password`, { method: 'POST', body: { password: value } }), 'Пароль не отправился'),
        start: () => act(() => api<Run>(`/api/admin/import/${run?.id}/start`, { method: 'POST', body: {} }), 'Перенос не запустился'),
        recheck: () => act(() => api<Run>(`/api/admin/import/${run?.id}/check`, { method: 'POST', body: {} }), 'Проверка не запустилась'),
        discard: async () => {
          if (run && run.state !== 'done') {
            await act(() => api<void>(`/api/admin/import/${run.id}`, { method: 'DELETE' }), 'Файл не убрался');
          }
          setFresh(false);
        },
        again: () => {
          setSource(view.run?.source ?? null);
          setPicked(true);
          setFresh(true);
          setError(null);
        },
        busy,
        error,
      }
    : null;

  return (
    <SettingsLayout>
      {loadError && !view ? (
        <section className="sec card-pad" role="alert">
          <h2 className="section-title">Перенос не загрузился</h2>
          <p className="muted">{loadError}</p>
        </section>
      ) : null}
      {ctx ? (
        <>
          <Wizard ctx={ctx} phone={phone} />
          {error ? (
            <p className="err imp-error" role="alert">
              {error}
            </p>
          ) : null}
        </>
      ) : null}
    </SettingsLayout>
  );
}
