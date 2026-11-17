import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { api, ApiError } from '../../lib/api';
import { dateLong, dateTime, NBSP } from '../../lib/format';
import { Switch } from '../ui';
import '../form.css';
import './updates.css';
import { SettingsLayout } from './SettingsLayout';

type StepKey = 'check' | 'download' | 'backup' | 'migrate' | 'start' | 'verify';
type Step = { key: StepKey; state: 'pending' | 'running' | 'done' | 'failed'; seconds: number | null };
type Update = {
  id: string;
  origin: string;
  state: 'requested' | 'waiting' | 'running' | 'done' | 'failed' | 'rolled_back' | 'attention';
  from: string | null;
  to: string | null;
  steps: Step[];
  requested_at: string | null;
  started_at: string | null;
  finished_at: string | null;
  error: string | null;
  downtime: number | null;
  backup: string | null;
  by: string | null;
};
type History = {
  id: string;
  origin: string;
  result: string;
  error: string | null;
  from: string | null;
  to: string | null;
  started_at: string | null;
  finished_at: string | null;
  seconds: number | null;
  by: string | null;
};
type System = {
  current: { version: string; commit: string | null; built_at: string | null; arch: string; source_url: string | null };
  components: { remnawave: string; postgres: string | null };
  check: { enabled: boolean; allowed: boolean; state: 'never' | 'ok' | 'unreachable' | 'invalid' | 'no_build'; last_checked_at: string | null; last_ok_at: string | null };
  latest: { commit: string; built_at: string; source_url: string } | null;
  helper: { installed: boolean; panel_updates: boolean; blocked: string | null };
  update: Update | null;
  history: History[];
  can_update: boolean;
  owner: boolean;
  totp_required: boolean;
};

const STEPS: Array<{ key: StepKey; label: string }> = [
  { key: 'check', label: 'Проверка сборки' },
  { key: 'download', label: 'Загрузка' },
  { key: 'backup', label: 'Резервная копия' },
  { key: 'migrate', label: 'Обновление базы' },
  { key: 'start', label: 'Запуск новой версии' },
  { key: 'verify', label: 'Проверка работы' },
];

const ACTIVE = ['requested', 'waiting', 'running'];
const STATUS_CMD = 'sudo stealthnet status';
const UPDATE_CMD = 'sudo stealthnet update';
const HIDDEN_KEY = 'sn.admin.updates.hidden';

type Retry = 'yes' | 'no' | 'check';
type Explain = { what: string; todo: string | null; command?: string; retry: Retry };

function short(commit: string | null | undefined): string {
  return commit ? commit.slice(0, 7) : '';
}

function seconds(n: number): string {
  return `${n}${NBSP}с`;
}

function when(iso: string | null | undefined): string {
  return dateTime(iso).replace(/^Сегодня/, 'сегодня').replace(/^Вчера/, 'вчера');
}

function explain(code: string | null, downtime: number | null): Explain {
  const down = downtime !== null && downtime > 0 ? ` Панель была недоступна ${seconds(downtime)}.` : '';
  switch (code) {
    case 'download_failed':
      return { what: 'Не удалось скачать сборку: реестр сборок не ответил. Установка не началась.', todo: 'Повторите через несколько минут.', retry: 'yes' };
    case 'wrong_arch':
      return { what: 'Готовой сборки для процессора этого сервера нет. Установка не началась.', todo: 'Обновите командой на сервере — она соберёт систему сама:', command: UPDATE_CMD, retry: 'no' };
    case 'no_space':
      return { what: 'На диске не хватает места для копии базы. Установка не началась.', todo: 'Освободите место на сервере и повторите.', retry: 'yes' };
    case 'docker_unavailable':
      return { what: 'Docker на сервере не отвечает. Установка не началась.', todo: 'Что с сервером, покажет команда:', command: STATUS_CMD, retry: 'yes' };
    case 'stale_request':
      return { what: 'Пока вы подтверждали, вышла ещё более новая сборка.', todo: 'Проверьте обновления ещё раз.', retry: 'check' };
    case 'expired':
      return { what: 'Заявка на обновление пролежала слишком долго.', todo: 'Проверьте обновления ещё раз.', retry: 'check' };
    case 'no_update':
      return { what: 'Новой сборки нет: установлена последняя.', todo: null, retry: 'no' };
    case 'rate_limited':
      return { what: 'Обновление уже запускалось недавно и не прошло.', todo: 'Повторить можно через 10 минут.', retry: 'yes' };
    case 'busy':
      return { what: 'Сервер был занят другим обновлением. Установка не началась.', todo: 'Повторите через несколько минут.', retry: 'yes' };
    case 'blocked':
      return { what: 'Эта сборка уже не подошла этому серверу.', todo: 'Дождитесь следующей сборки — она появится здесь же.', retry: 'no' };
    case 'helper_silent':
      return { what: 'Сервер не начал обновление за 30 секунд. Заявка отменена.', todo: 'Проверьте сервер командой — она покажет, включено ли обновление из админки:', command: STATUS_CMD, retry: 'yes' };
    case 'invalid_request':
      return { what: 'Сервер не принял заявку на обновление.', todo: 'Повторите один раз. Если повторится, посмотрите состояние командой:', command: STATUS_CMD, retry: 'yes' };
    case 'backup_failed':
      return { what: `Не удалось сделать копию базы, поэтому обновление не началось.${down}`, todo: 'Чаще всего не хватает места на диске. Освободите место и повторите.', retry: 'yes' };
    case 'migrate_failed':
      return { what: `Не применилось обновление базы.${down}`, todo: 'Повторять не нужно: ошибка в самой сборке. Дождитесь исправленной — она появится здесь же.', retry: 'no' };
    case 'start_failed':
    case 'unhealthy':
      return { what: 'Новая сборка запустилась, но за 90 секунд не заработала как надо. Система ничего не откатывала сама, чтобы не потерять оплаты, которые уже могли прийти.', todo: 'Зайдите на сервер и выполните команду — она покажет, что произошло:', command: STATUS_CMD, retry: 'no' };
    case 'interrupted':
      return { what: `Обновление прервалось: сервер перезагрузился или обновление остановили.${down}`, todo: 'Можно повторить.', retry: 'yes' };
    default:
      return { what: `Обновление остановилось из-за ошибки на сервере.${down}`, todo: 'Повторите один раз. Если повторится, посмотрите состояние командой:', command: STATUS_CMD, retry: 'yes' };
  }
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function readHidden(): string | null {
  try {
    return window.sessionStorage.getItem(HIDDEN_KEY);
  } catch {
    return null;
  }
}

function writeHidden(id: string) {
  try {
    window.sessionStorage.setItem(HIDDEN_KEY, id);
  } catch {
    return;
  }
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
    <span className="upd-cmd">
      <code className="mono">{text}</code>
      <button type="button" className="btn-line hov upd-copy" onClick={() => void copy()}>
        {copied ? 'Скопировано' : 'Скопировать'}
      </button>
    </span>
  );
}

function Steps({ update, now }: { update: Update; now: number }) {
  return (
    <ol className="upd-steps">
      {STEPS.map((step) => {
        const st = update.steps.find((s) => s.key === step.key);
        const state = st?.state ?? 'pending';
        const started = update.started_at ? Date.parse(update.started_at) : null;
        const done = update.steps.filter((s) => s.state === 'done').reduce((sum, s) => sum + (s.seconds ?? 0), 0);
        const running = state === 'running' && started ? Math.max(0, Math.round((now - started) / 1000) - done) : null;
        return (
          <li key={step.key} className={`upd-step ${state}`}>
            <span className="upd-step-name">
              {state === 'running' ? <span className="upd-dot" aria-hidden="true" /> : null}
              {step.label}
            </span>
            <span className="upd-step-time num">
              {state === 'done' && st?.seconds !== null && st?.seconds !== undefined ? seconds(st.seconds) : null}
              {state === 'running' ? `идёт${running !== null ? `, ${seconds(running)}` : ''}` : null}
              {state === 'failed' ? 'не прошёл' : null}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

export function SettingsUpdates() {
  const [data, setData] = useState<System | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [offline, setOffline] = useState<number | null>(null);
  const [checking, setChecking] = useState(false);
  const [note, setNote] = useState<{ text: string; bad: boolean } | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [password, setPassword] = useState('');
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [formError, setFormError] = useState<{ text: string; field: 'password' | 'code' | null } | null>(null);
  const [hidden, setHidden] = useState<string | null>(() => readHidden());
  const [now, setNow] = useState(() => Date.now());
  const startCommit = useRef<string | null | undefined>(undefined);
  const watching = useRef(false);

  const load = useCallback(async () => {
    try {
      const next = await api<System>('/api/admin/system');
      if (startCommit.current === undefined) {
        startCommit.current = next.current.commit;
      }
      setData(next);
      setError(null);
      setOffline(null);
      watching.current = !!next.update && ACTIVE.includes(next.update.state);
    } catch (err) {
      if (watching.current && !(err instanceof ApiError && err.status < 500)) {
        setOffline((value) => value ?? Date.now());
        return;
      }
      setError(failText(err, 'Сведения о версии не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const active = (!!data?.update && ACTIVE.includes(data.update.state)) || offline !== null;
  useEffect(() => {
    if (!active) {
      return;
    }
    const poll = window.setInterval(() => void load(), 2000);
    const tick = window.setInterval(() => setNow(Date.now()), 1000);
    return () => {
      window.clearInterval(poll);
      window.clearInterval(tick);
    };
  }, [active, load]);

  if (!data) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем сведения о версии…</p>
        )}
      </SettingsLayout>
    );
  }

  const { current, check, latest, helper, owner } = data;
  const update = data.update && data.update.id !== hidden ? data.update : null;
  const running = !!update && ACTIVE.includes(update.state);
  const panelHelper = helper.installed && helper.panel_updates;
  const blocked = !!latest && helper.blocked === latest.commit;
  const upgraded = !!update && update.state === 'done' && !!update.to && update.to === current.commit;
  const reloadNeeded = upgraded && startCommit.current !== undefined && startCommit.current !== current.commit;

  const checkNow = async () => {
    setChecking(true);
    setNote(null);
    try {
      setData(await api<System>('/api/admin/system/check', { method: 'POST', body: {} }));
    } catch (err) {
      setNote({ text: failText(err, 'Проверить не получилось'), bad: true });
    } finally {
      setChecking(false);
    }
  };

  const toggleCheck = async () => {
    try {
      setData(await api<System>('/api/admin/system/settings', { method: 'PUT', body: { check_enabled: !check.enabled } }));
    } catch (err) {
      setNote({ text: failText(err, 'Настройка не сохранилась'), bad: true });
    }
  };

  const openConfirm = () => {
    setConfirm(true);
    setPassword('');
    setCode('');
    setFormError(null);
  };

  const start = async () => {
    if (!latest || busy) {
      return;
    }
    if (!password) {
      setFormError({ text: 'Введите пароль', field: 'password' });
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      await api('/api/admin/system/update', {
        method: 'POST',
        body: { target: latest.commit, password, code: data.totp_required ? code : undefined },
      });
      setConfirm(false);
      setPassword('');
      setCode('');
      watching.current = true;
      await load();
    } catch (err) {
      if (err instanceof ApiError && err.code === 'auth.wrong_password') {
        setFormError({ text: 'Пароль не подошёл', field: 'password' });
      } else if (err instanceof ApiError && (err.code === 'auth.totp_invalid' || err.code === 'auth.totp_required')) {
        setFormError({ text: err.code === 'auth.totp_required' ? 'Введите код из приложения' : 'Код не подошёл', field: 'code' });
      } else {
        setFormError({ text: failText(err, 'Обновление не запустилось'), field: null });
        await load();
      }
    } finally {
      setBusy(false);
    }
  };

  const cancel = async () => {
    try {
      await api('/api/admin/system/update', { method: 'DELETE' });
    } catch (err) {
      setNote({ text: failText(err, 'Отменить не получилось'), bad: true });
    }
    await load();
  };

  const hide = () => {
    if (update) {
      writeHidden(update.id);
      setHidden(update.id);
    }
  };

  const failed = !!update && ['failed', 'rolled_back', 'attention'].includes(update.state);
  const why = failed && update ? explain(update.error, update.downtime) : null;
  const target = update?.to ?? latest?.commit ?? null;
  const elapsed = update?.started_at ? Math.max(0, Math.round((now - Date.parse(update.started_at)) / 1000)) : null;
  const restarting = offline !== null ? Math.max(0, Math.round((now - offline) / 1000)) : null;
  const doneSteps = update ? new Set(update.steps.filter((s) => s.state === 'done').map((s) => s.key)) : new Set<string>();

  let status: ReactNode;
  if (restarting !== null && update) {
    status = (
      <>
        <p className="upd-status">
          <span className="upd-dot" aria-hidden="true" />
          Панель перезапускается · прошло {seconds(restarting)}
        </p>
        {restarting > 300 ? (
          <div className="upd-why">
            <p className="err">Панель не отвечает больше 5 минут.</p>
            <p>Зайдите на сервер и выполните команду — она покажет, что произошло и что делать:</p>
            <Command text={STATUS_CMD} />
          </div>
        ) : (
          <p className="cap">Страница сама продолжит, когда панель ответит. Закрывать её не нужно.</p>
        )}
      </>
    );
  } else if (running && update) {
    status = (
      <>
        <p className="upd-status strong">
          {update.state === 'requested' ? `Обновление до сборки ${short(update.to)} запрошено` : `Обновляем до сборки ${short(update.to)}`}
          {elapsed !== null ? ` · прошло ${seconds(elapsed)}` : ''}
        </p>
        {update.state === 'waiting' ? (
          <p className="upd-status">
            <span className="upd-dot" aria-hidden="true" />
            Ждём окончания другой операции на сервере
          </p>
        ) : null}
        <Steps update={update} now={now} />
        {update.state === 'requested' ? (
          <div className="upd-actions">
            <span className="cap">Сервер вот-вот заберёт заявку. Пока он этого не сделал, её можно отменить.</span>
            <button type="button" className="btn-line hov" onClick={() => void cancel()}>
              Отменить
            </button>
          </div>
        ) : doneSteps.has('migrate') ? (
          <p className="cap">Новая версия запускается. Проверяем, что работают панель, бот и фоновые задачи.</p>
        ) : doneSteps.has('download') ? (
          <p className="cap">Панель остановлена примерно на минуту. Страница сама продолжит, когда панель ответит.</p>
        ) : (
          <p className="cap">Пока панель работает как обычно. Она остановится примерно на минуту на шаге «Резервная копия».</p>
        )}
      </>
    );
  } else if (failed && update && why) {
    status = (
      <>
        {update.state === 'attention' ? (
          <p className="upd-status bad">Сборка {short(update.to)} запущена, но работает с ошибками.</p>
        ) : (
          <p className="upd-status">
            <span className="bad">{target ? `Сборка ${short(target)} не установлена.` : 'Обновление не установлено.'}</span>{' '}
            <span className="ink2">
              {update.downtime ? 'Работает прежняя версия, данные не изменились.' : 'Работает прежняя версия, панель не останавливалась.'}
            </span>
          </p>
        )}
        <dl className="upd-why">
          <dt>Что случилось</dt>
          <dd>{why.what}</dd>
          {why.todo ? (
            <>
              <dt>Что делать</dt>
              <dd>
                {why.todo}
                {why.command ? <Command text={why.command} /> : null}
              </dd>
            </>
          ) : null}
        </dl>
        {owner ? (
          <div className="upd-actions">
            {why.retry === 'yes' && data.can_update ? (
              <button type="button" className="btn-line hov" onClick={openConfirm}>
                Попробовать ещё раз
              </button>
            ) : null}
            {why.retry === 'check' ? (
              <button type="button" className="btn-line hov" onClick={() => void checkNow()} disabled={checking || !check.allowed}>
                {checking ? 'Проверяю…' : 'Проверить сейчас'}
              </button>
            ) : null}
            <button type="button" className="btn-line hov" onClick={hide}>
              Скрыть
            </button>
          </div>
        ) : null}
      </>
    );
  } else if (latest && owner) {
    status = (
      <>
        <p className="upd-status">
          <span className="strong">
            Вышла новая сборка {short(latest.commit)} от {dateLong(latest.built_at)}
          </span>
          {check.last_checked_at ? <span className="muted"> · проверено {when(check.last_checked_at)}</span> : null}
        </p>
        <p className="cap">
          Номер сборки — это коммит в репозитории. Сборку проверили автоматические тесты, прежде чем выложить.{' '}
          <a href={latest.source_url} target="_blank" rel="noreferrer">
            Исходный код новой сборки
          </a>
        </p>
        {blocked ? (
          <p className="ink2">Сборка {short(latest.commit)} не подошла этому серверу: ошибка в самом обновлении. Кнопка появится, когда выйдет следующая сборка.</p>
        ) : !panelHelper ? (
          <div className="upd-why">
            <p>{helper.installed ? 'Обновление из админки на этом сервере выключено.' : 'Обновление из админки на этом сервере не установлено.'} Обновить можно командой на сервере:</p>
            <Command text={UPDATE_CMD} />
          </div>
        ) : (
          <p className="cap">
            Панель и бот будут недоступны около минуты. Перед установкой сохраним копию базы. Если новая версия не пройдёт проверку перед запуском, вернём прежнюю: данные при этом не изменятся.
          </p>
        )}
      </>
    );
  } else {
    let line: ReactNode;
    if (!check.allowed) {
      line = <p className="upd-status muted">Проверка обновлений выключена на сервере</p>;
    } else if (check.state === 'unreachable' || check.state === 'invalid') {
      line = (
        <>
          <p className="upd-status">
            <span className="warn">Не удалось проверить обновления: реестр сборок не отвечает</span>
            {check.last_ok_at ? <span className="muted"> · последняя удачная проверка — {when(check.last_ok_at)}</span> : null}
          </p>
          <p className="cap">На работу панели это не влияет. Проверка повторится сама.</p>
        </>
      );
    } else if (check.state === 'no_build') {
      line = (
        <div className="upd-why">
          <p className="ink2">Готовых сборок для процессора этого сервера нет. Обновлять можно командой на сервере:</p>
          <Command text={UPDATE_CMD} />
        </div>
      );
    } else if (check.state === 'never') {
      line = <p className="upd-status muted">{check.enabled ? 'Обновления ещё не проверялись' : 'Проверка обновлений выключена'}</p>;
    } else if (!current.built_at || !current.commit) {
      line = <p className="upd-status muted">Эта версия собрана вручную, поэтому сравнить её с опубликованными сборками нельзя</p>;
    } else {
      line = (
        <p className="upd-status ink2">
          Установлена последняя сборка{check.last_checked_at ? ` · проверено ${when(check.last_checked_at)}` : ''}
        </p>
      );
    }
    status = line;
  }

  const showButtons = owner && !running && restarting === null && !failed;
  const ago = check.last_checked_at ? now - Date.parse(check.last_checked_at) : Infinity;

  return (
    <SettingsLayout>
      {upgraded && update ? (
        <div role="status" className="upd-banner">
          <span>
            <strong>Обновлено до сборки {short(update.to)}</strong> {when(update.finished_at)}
            {update.started_at && update.finished_at ? ` за ${seconds(Math.round((Date.parse(update.finished_at) - Date.parse(update.started_at)) / 1000))}` : ''}. Копия базы перед обновлением сохранена.
          </span>
          {reloadNeeded ? (
            <button type="button" className="btn-line hov" onClick={() => window.location.reload()}>
              Обновить страницу
            </button>
          ) : (
            <button type="button" className="btn-line hov" onClick={hide}>
              Скрыть
            </button>
          )}
        </div>
      ) : null}

      <section className="sec" aria-labelledby="upd-h">
        <div className="upd-head">
          <div className="upd-title">
            <h2 id="upd-h" className="upd-version">
              STEALTHNET {current.version}
            </h2>
            {status}
          </div>
          {showButtons ? (
            <div className="upd-buttons">
              {latest && data.can_update && !confirm ? (
                <button type="button" className="btn-main btn" onClick={openConfirm}>
                  Обновить до {short(latest.commit)}
                </button>
              ) : null}
              <button type="button" className="btn-line hov" onClick={() => void checkNow()} disabled={checking || !check.allowed || ago < 60_000}>
                {checking ? 'Проверяю…' : 'Проверить сейчас'}
              </button>
            </div>
          ) : null}
        </div>
        {confirm && latest && owner ? (
          <div className="upd-confirm">
            <p className="strong">Обновить до сборки {short(latest.commit)}?</p>
            <div className="upd-fields">
              <label className="upd-field">
                <span>Пароль владельца</span>
                <input
                  className="field"
                  type="password"
                  autoComplete="current-password"
                  value={password}
                  aria-invalid={formError?.field === 'password'}
                  onChange={(e) => setPassword(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      void start();
                    }
                  }}
                  disabled={busy}
                  autoFocus
                />
              </label>
              {data.totp_required ? (
                <label className="upd-field">
                  <span>Код из приложения</span>
                  <input
                    className="field num"
                    inputMode="numeric"
                    autoComplete="one-time-code"
                    value={code}
                    aria-invalid={formError?.field === 'code'}
                    onChange={(e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
                    disabled={busy}
                  />
                </label>
              ) : null}
            </div>
            {data.totp_required ? <p className="cap">Код, которым вы только что входили, второй раз не подойдёт — дождитесь следующего.</p> : null}
            {formError ? (
              <p className="err" role="alert">
                {formError.text}
              </p>
            ) : null}
            <div className="upd-actions">
              <button type="button" className="btn-main btn" onClick={() => void start()} disabled={busy}>
                {busy ? 'Запускаем…' : 'Обновить'}
              </button>
              <button type="button" className="btn-line hov" onClick={() => setConfirm(false)} disabled={busy}>
                Отменить
              </button>
            </div>
          </div>
        ) : null}
        {!owner ? <p className="upd-owner muted">Обновляет владелец</p> : null}
        {note ? (
          <p className={note.bad ? 'err upd-note' : 'cap upd-note'} role="status">
            {note.text}
          </p>
        ) : null}
        <div className="row">
          <span className="rl">Сборка</span>
          <div className="rc upd-value">
            {current.commit ? (
              <span>
                <code className="mono">{short(current.commit)}</code>
                {current.built_at ? ` · ${dateLong(current.built_at, true)}` : ''}
                {current.source_url ? (
                  <>
                    {' · '}
                    <a href={current.source_url} target="_blank" rel="noreferrer">
                      Исходный код этой сборки
                    </a>
                  </>
                ) : null}
              </span>
            ) : (
              <span className="muted">собрана вручную, номер неизвестен</span>
            )}
          </div>
        </div>
        <div className="row">
          <span className="rl">Remnawave</span>
          <div className="rc upd-value">
            <span className="ink2">эта версия проверена с Remnawave {data.components.remnawave}</span>
          </div>
        </div>
        {data.components.postgres ? (
          <div className="row">
            <span className="rl">База данных</span>
            <div className="rc upd-value">
              <span>PostgreSQL {data.components.postgres}</span>
            </div>
          </div>
        ) : null}
        <div className="row">
          <span className="rl">Обновление из админки</span>
          <div className="rc upd-value">
            <span className={panelHelper ? 'ink2' : 'muted'}>
              {panelHelper ? 'включено' : helper.installed ? 'выключено на сервере' : 'не установлено'}
            </span>
          </div>
        </div>
        <div className="row">
          <span className="rl" id="upd-check-l">
            Проверять обновления
          </span>
          <div className="rc">
            {owner ? (
              <span className="upd-switch">
                <Switch on={check.enabled} onToggle={() => void toggleCheck()} label="Проверять обновления" describedBy="upd-check-hint" disabled={!check.allowed} />
              </span>
            ) : (
              <span className="upd-value">{check.enabled ? 'включено' : 'выключено'}</span>
            )}
            <span id="upd-check-hint" className="cap">
              Раз в сутки панель спрашивает у реестра сборок GitHub, какая сборка последняя. О вашей установке в запросе ничего нет, но адрес сервера реестру виден, как при любом обращении в интернет.
            </span>
          </div>
        </div>
      </section>

      <section className="sec" aria-labelledby="upd-b-h">
        <div className="sec-head">
          <h2 id="upd-b-h" className="section-title">
            Резервные копии
          </h2>
          <span className="muted">
            Перед каждым обновлением сервер сохраняет копию базы и файла с ключами. Хранятся три последние копии.
          </span>
        </div>
        <div className="row">
          <span className="rl">Где лежат</span>
          <div className="rc upd-value">
            <span>
              в папке <code className="mono">deploy/backups</code> рядом с установкой
            </span>
          </div>
        </div>
        <p className="upd-foot cap">
          Скачать копию отсюда нельзя: с ключами из неё расшифровываются токены платёжных систем. Храните копию не только на этом сервере и никому её не отправляйте — в том числе тем, кто назвался поддержкой.
        </p>
      </section>

      {data.history.length ? (
        <section className="sec" aria-labelledby="upd-h-h">
          <div className="sec-head">
            <h2 id="upd-h-h" className="section-title">
              История обновлений
            </h2>
          </div>
          <ul className="upd-history">
            {data.history.map((item) => (
              <li key={item.id} className="upd-hrow">
                <span className="num upd-hwhen">{when(item.started_at)}</span>
                <span className="mono">
                  {short(item.from) || '—'} → {short(item.to) || '—'}
                </span>
                <span className={item.result === 'done' ? 'ink2' : 'bad'}>
                  {item.result === 'done'
                    ? `установлено${item.seconds ? ` за ${seconds(item.seconds)}` : ''}`
                    : item.result === 'rolled_back'
                      ? 'не установлено, версия возвращена'
                      : item.result === 'attention'
                        ? 'запущено с ошибками'
                        : 'не началось'}
                </span>
                <span className="muted">{item.origin === 'panel' ? (item.by ?? 'из админки') : 'командой на сервере'}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <p className="cap">
        Панель просит выполнять на сервере только две команды: <code className="mono">{STATUS_CMD}</code> и <code className="mono">{UPDATE_CMD}</code>. Другие команды от имени панели не выполняйте.
      </p>
    </SettingsLayout>
  );
}
