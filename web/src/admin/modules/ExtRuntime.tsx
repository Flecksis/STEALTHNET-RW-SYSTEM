import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { pathPart } from '../../lib/apiPath';
import type { ExtCard, LogsView, RuntimeJob, RuntimeView } from './ext';
import {
  RUNTIME_POLL_MS,
  capital,
  dataLine,
  healthProblem,
  jobActive,
  jobFailure,
  memoryLine,
  momentText,
  runningNote,
  sizeText,
  stateTitle,
  stepTitle,
  volumesText,
} from './package';

export type Confirm = { password: string; code: string };

export type ConfirmError = { text: string; field: 'password' | 'code' | null };

export const NO_CONFIRM: Confirm = { password: '', code: '' };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function confirmError(err: unknown, fallback: string): ConfirmError {
  if (err instanceof ApiError && err.code === 'auth.wrong_password') {
    return { text: 'Пароль не подошёл', field: 'password' };
  }
  if (err instanceof ApiError && err.code === 'auth.totp_required') {
    return { text: 'Введите код из приложения', field: 'code' };
  }
  if (err instanceof ApiError && err.code === 'auth.totp_invalid') {
    return { text: 'Код не подошёл. Код, которым вы только что входили, второй раз не подойдёт — дождитесь следующего', field: 'code' };
  }
  return { text: failText(err, fallback), field: null };
}

export function useRuntimeWatch(id: string | null, runtime: RuntimeView | null, setRuntime: (next: RuntimeView) => void): boolean {
  const active = jobActive(runtime?.job);
  useEffect(() => {
    if (!id || !active) {
      return;
    }
    let alive = true;
    let timer = 0;
    const tick = async () => {
      try {
        const next = await api<RuntimeView>(`/api/admin/ext-modules/${pathPart(id)}/runtime`);
        if (alive) {
          setRuntime(next);
        }
      } catch (err) {
        if (err instanceof ApiError && err.status === 404) {
          return;
        }
      }
      if (alive) {
        timer = window.setTimeout(() => void tick(), RUNTIME_POLL_MS);
      }
    };
    timer = window.setTimeout(() => void tick(), RUNTIME_POLL_MS);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [id, active, setRuntime]);
  return active;
}

export function JobSteps({ job }: { job: RuntimeJob }) {
  return (
    <ol className="upd-steps">
      {job.steps.map((step) => (
        <li key={step.key} className={`upd-step ${step.state}`}>
          <span className="upd-step-name">
            {step.state === 'running' ? <span className="upd-dot" aria-hidden="true" /> : null}
            {stepTitle(step.key)}
          </span>
          <span className="upd-step-time num">
            {step.state === 'done' && step.seconds !== null ? (step.seconds < 1 ? '<1 с' : `${step.seconds} с`) : null}
            {step.state === 'failed' ? 'не прошёл' : null}
          </span>
        </li>
      ))}
    </ol>
  );
}

export function OwnerFields({
  value,
  onChange,
  totp,
  invalid,
  busy,
  onSubmit,
}: {
  value: Confirm;
  onChange: (next: Confirm) => void;
  totp: boolean;
  invalid: ConfirmError['field'];
  busy: boolean;
  onSubmit: () => void;
}) {
  const enter = (key: string) => {
    if (key === 'Enter') {
      onSubmit();
    }
  };
  return (
    <>
      <label className="upd-field">
        <span>Пароль владельца</span>
        <input
          className="field"
          type="password"
          autoComplete="current-password"
          value={value.password}
          aria-invalid={invalid === 'password'}
          onChange={(e) => onChange({ ...value, password: e.target.value })}
          onKeyDown={(e) => enter(e.key)}
          disabled={busy}
        />
      </label>
      {totp ? (
        <label className="upd-field">
          <span>Код из приложения</span>
          <input
            className="field num"
            inputMode="numeric"
            autoComplete="one-time-code"
            value={value.code}
            aria-invalid={invalid === 'code'}
            onChange={(e) => onChange({ ...value, code: e.target.value.replace(/\D/g, '').slice(0, 6) })}
            onKeyDown={(e) => enter(e.key)}
            disabled={busy}
          />
        </label>
      ) : null}
    </>
  );
}

function LogsRow({ id, runtime, onRuntime }: { id: string; runtime: RuntimeView; onRuntime: (next: RuntimeView) => void }) {
  const [logs, setLogs] = useState<LogsView | null>(null);
  const [waiting, setWaiting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const box = useRef<HTMLPreElement>(null);
  const active = jobActive(runtime.job);

  const load = useCallback(async () => {
    try {
      setLogs(await api<LogsView>(`/api/admin/ext-modules/${pathPart(id)}/logs`));
    } catch (err) {
      setError(failText(err, 'Журнал не загрузился'));
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!waiting || active) {
      return;
    }
    setWaiting(false);
    const job = runtime.job;
    if (job && job.action === 'module-logs' && job.state !== 'done') {
      setError(jobFailure(job) ?? 'Журнал не обновился');
    }
    void load();
  }, [waiting, active, runtime.job, load]);

  useEffect(() => {
    if (box.current) {
      box.current.scrollTop = box.current.scrollHeight;
    }
  }, [logs]);

  const refresh = async () => {
    setBusy(true);
    setError(null);
    try {
      onRuntime(await api<RuntimeView>(`/api/admin/ext-modules/${pathPart(id)}/logs`, { method: 'POST', body: {} }));
      setWaiting(true);
    } catch (err) {
      setError(failText(err, 'Журнал не обновился'));
    } finally {
      setBusy(false);
    }
  };

  const lines = logs?.lines ?? [];
  const when = logs?.at ? `последние строки, обновлено ${momentText(logs.at, new Date())}` : 'последние строки модуля';
  return (
    <div className="row">
      <div className="rc">
        <div className="mz-split">
          <span className="adm-who">
            <span className="adm-email">Журнал</span>
            <span className="adm-status">{when}</span>
          </span>
          <span className="adm-acts">
            <button
              type="button"
              className="b34 hov"
              onClick={() => void refresh()}
              disabled={busy || waiting || active || runtime.helper.state !== 'ready'}
            >
              {busy || waiting ? 'Обновляем…' : 'Обновить'}
            </button>
          </span>
        </div>
        {error ? (
          <span className="err" role="alert">
            {error}
          </span>
        ) : null}
        {lines.length ? (
          <pre ref={box} className="mz-log mono">
            {lines.join('\n')}
          </pre>
        ) : logs ? (
          <span className="cap">
            {logs.at ? 'Модуль пока ничего не написал в журнал.' : 'Журнала ещё нет: нажмите «Обновить», и сервер пришлёт последние строки.'}
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function RuntimeSection({ id, runtime, onRuntime }: { id: string; runtime: RuntimeView; onRuntime: (next: RuntimeView) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const now = new Date();
  const job = runtime.job;
  const active = jobActive(job);
  const stepsJob = active && job && job.action !== 'module-logs' ? job : null;
  const ready = runtime.helper.state === 'ready';
  const note = runningNote(runtime.state, runtime.container, runtime.health, now);
  const sick = healthProblem(runtime.state, runtime.health, now);
  const failure = job && job.action !== 'module-logs' && (job.state === 'rolled_back' || runtime.state !== 'running') ? jobFailure(job) : null;
  const canRestart = ready && runtime.enabled && runtime.state !== 'missing' && !stepsJob;
  const checkedAt = runtime.container?.checked_at ?? null;
  const measured = checkedAt ? `замер ${momentText(checkedAt, now)}` : null;

  const restart = async () => {
    setBusy(true);
    setError(null);
    try {
      onRuntime(await api<RuntimeView>(`/api/admin/ext-modules/${pathPart(id)}/restart`, { method: 'POST', body: {} }));
    } catch (err) {
      setError(failText(err, 'Модуль не перезапустился'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="sec" aria-labelledby="ext-run-h">
      <div className="sec-head">
        <h2 id="ext-run-h" className="section-title">
          На сервере
        </h2>
      </div>
      <div className="row adm-row">
        <span className="adm-who" aria-live="polite">
          <span className={runtime.state === 'running' ? 'mz-state ok' : 'mz-state'}>{stateTitle(runtime.state)}</span>
          {note ? <span className="adm-status">{note}</span> : null}
          {sick ? <span className="adm-status bad">{sick}</span> : null}
          {failure ? <span className="adm-status bad">{failure}</span> : null}
          {runtime.state === 'missing' ? <span className="adm-status">На этом сервере модуль не запущен — загрузите архив заново.</span> : null}
          {!ready ? <span className="adm-status">Управлять модулем отсюда сейчас нельзя: {runtime.reason}.</span> : null}
        </span>
        {canRestart ? (
          <span className="adm-acts">
            <button type="button" className="b34 hov" onClick={() => void restart()} disabled={busy || active}>
              {busy ? 'Перезапускаем…' : 'Перезапустить'}
            </button>
          </span>
        ) : null}
      </div>
      {stepsJob ? (
        <div className="row">
          <div className="rc">
            <JobSteps job={stepsJob} />
            {stepsJob.state === 'waiting' ? <span className="cap">Ждём, когда сервер возьмёт заявку.</span> : null}
          </div>
        </div>
      ) : null}
      {error ? (
        <div className="row">
          <span className="err" role="alert">
            {error}
          </span>
        </div>
      ) : null}
      {runtime.run ? (
        <>
          <div className="row adm-row">
            <span className="adm-who">
              <span className="adm-email">Память</span>
              <span className="adm-status">
                {memoryLine(runtime.run, runtime.container)}
                {measured && runtime.container?.memory_bytes != null ? ` · ${measured}` : null}
              </span>
            </span>
          </div>
          <div className="row adm-row">
            <span className="adm-who">
              <span className="adm-email">Данные</span>
              <span className="adm-status">
                {dataLine(runtime.run, runtime.container)}
                {measured && runtime.container?.data_bytes != null && runtime.run.volumes.length ? ` · ${measured}` : null}
              </span>
            </span>
          </div>
        </>
      ) : null}
      {runtime.can_logs ? <LogsRow id={id} runtime={runtime} onRuntime={onRuntime} /> : null}
    </section>
  );
}

export function ManagedManage({ card, runtime, owner }: { card: ExtCard; runtime: RuntimeView | null; owner: boolean }) {
  const navigate = useNavigate();
  const [asking, setAsking] = useState(false);
  const [confirm, setConfirm] = useState<Confirm>(NO_CONFIRM);
  const [erase, setErase] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ConfirmError | null>(null);
  const volumes = runtime?.run?.volumes ?? [];
  const folders = volumesText(volumes);
  const dataSize = runtime?.container?.data_bytes;
  const totp = runtime?.totp_required ?? false;

  const open = () => {
    setAsking(!asking);
    setConfirm(NO_CONFIRM);
    setErase(false);
    setError(null);
  };

  const remove = async () => {
    if (busy) {
      return;
    }
    if (!confirm.password) {
      setError({ text: 'Введите пароль', field: 'password' });
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api(`/api/admin/ext-modules/${pathPart(card.id)}`, {
        method: 'DELETE',
        body: { password: confirm.password, code: totp ? confirm.code : undefined, erase_data: erase },
      });
      navigate('/admin/modules');
    } catch (err) {
      setError(confirmError(err, 'Модуль не удалился'));
      setBusy(false);
    }
  };

  return (
    <section className="sec" aria-labelledby="ext-manage-h">
      <div className="sec-head">
        <h2 id="ext-manage-h" className="section-title">
          Обновить или удалить
        </h2>
      </div>
      <div className="row adm-row">
        <span className="adm-who">
          <span className="ext-cred-label">Новая версия</span>
          <span className="adm-status">
            Загрузите архив новой версии: панель покажет, что изменится, и запустит её с теми же данными. Если новая версия не заработает, вернётся {card.version}.
          </span>
        </span>
        <span className="adm-acts">
          {owner ? (
            <Link to={`/admin/modules/install?update=${encodeURIComponent(card.id)}`} className="b34 hov">
              Обновить из архива
            </Link>
          ) : (
            <span className="adm-status">Обновляет владелец</span>
          )}
        </span>
      </div>
      <div className="row adm-row">
        <span className="adm-who">
          <span className="ext-cred-label">Удаление</span>
          <span className="adm-status">Модуль остановится, ключ API отзовётся. Данные модуля можно оставить на сервере или стереть.</span>
        </span>
        <span className="adm-acts">
          {owner ? (
            <button type="button" className="b34 bad hov" aria-expanded={asking} onClick={open} disabled={busy}>
              Удалить модуль
            </button>
          ) : (
            <span className="adm-status">Удаляет владелец</span>
          )}
        </span>
      </div>
      {asking && owner ? (
        <div className="mz-confirm-row">
          {folders ? (
            <div className="mz-erase">
              <label className="mz-check">
                <input type="checkbox" checked={erase} onChange={(e) => setErase(e.target.checked)} disabled={busy} />
                <span>Стереть данные модуля</span>
              </label>
              <span className="cap">
                {capital(folders)}
                {dataSize !== null && dataSize !== undefined ? `, ${sizeText(dataSize)}` : ''}. Стёртые данные не вернуть. Без галочки они останутся на сервере, и
                повторная установка их подхватит.
              </span>
            </div>
          ) : null}
          <div className="mz-fields">
            <OwnerFields value={confirm} onChange={setConfirm} totp={totp} invalid={error?.field ?? null} busy={busy} onSubmit={() => void remove()} />
          </div>
          {error ? (
            <p className="err" role="alert">
              {error.text}
            </p>
          ) : null}
          <div className="md-actions">
            <button type="button" className="btn-bad" onClick={() => void remove()} disabled={busy}>
              {busy ? 'Удаляем…' : erase ? 'Удалить модуль и данные' : 'Удалить модуль'}
            </button>
            <button type="button" className="btn-line hov" onClick={() => setAsking(false)} disabled={busy}>
              Отменить
            </button>
          </div>
        </div>
      ) : null}
    </section>
  );
}
