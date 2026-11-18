import { useRef, useState, type ChangeEvent } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { CredentialsBlock } from './ExtCredentials';
import { extLink, MANIFEST_LIMIT, type ExtEvent, type ExtPermission, type InstallResult, type Preview, type PreviewResult, type Problem } from './ext';
import '../form.css';
import '../pages/admins.css';
import './modules.css';
import './ext.css';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function readText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : '');
    reader.onerror = () => reject(reader.error);
    reader.readAsText(file);
  });
}

function problemText(p: Problem): string {
  return p.path ? `${p.path}: ${p.message}` : p.message;
}

function ChangeMark({ change }: { change: ExtEvent['change'] }) {
  if (change === 'added') {
    return <span className="ext-new">новое</span>;
  }
  return null;
}

function EventList({ events }: { events: ExtEvent[] }) {
  const kept = events.filter((e) => e.change !== 'removed');
  const gone = events.filter((e) => e.change === 'removed');
  return (
    <div className="rc">
      {kept.length === 0 ? <span className="cap">Нет: модуль не подписан на события.</span> : null}
      <ul className="ext-list">
        {kept.map((e) => (
          <li key={e.kind}>
            <span className="ext-item">
              {e.title} <ChangeMark change={e.change} />
            </span>
            <span className="cap">{e.about}</span>
          </li>
        ))}
      </ul>
      {gone.length ? <span className="cap">Больше не будет приходить: {gone.map((e) => e.title).join(', ')}.</span> : null}
    </div>
  );
}

function PermissionList({ permissions }: { permissions: ExtPermission[] }) {
  const kept = permissions.filter((p) => p.change !== 'removed');
  const gone = permissions.filter((p) => p.change === 'removed');
  return (
    <div className="rc">
      {kept.length === 0 ? <span className="cap">Нет: ключ модуля сможет только читать свои настройки.</span> : null}
      <ul className="ext-list">
        {kept.map((p) => (
          <li key={p.key}>
            <span className="ext-item">
              {p.title} <ChangeMark change={p.change} />
            </span>
            <span className="cap">{p.about}</span>
          </li>
        ))}
      </ul>
      {gone.length ? <span className="cap">Модуль теряет права: {gone.map((p) => p.title).join(', ')}.</span> : null}
    </div>
  );
}

function PreviewCard({ preview }: { preview: Preview }) {
  return (
    <section className="sec ext-facts" aria-labelledby="ext-what-h">
      <div className="sec-head">
        <h2 id="ext-what-h" className="section-title">
          Что получит модуль
        </h2>
        {preview.sensitive ? (
          <span className="md-warn">
            Модуль сможет менять данные и работать с деньгами через API, как администратор с этими правами. Ставьте модули только от тех, кому доверяете.
          </span>
        ) : (
          <span className="cap">Права ключа API модуля не шире ваших. Изменить их можно только новым файлом описания и повторным подтверждением.</span>
        )}
      </div>
      <div className="row">
        <span className="rl">События</span>
        <EventList events={preview.events} />
      </div>
      <div className="row">
        <span className="rl">Права ключа API</span>
        <PermissionList permissions={preview.permissions} />
      </div>
      <div className="row">
        <span className="rl">Адрес для событий</span>
        <span className="rc">
          {preview.webhook ? <span className="mono ext-url">{preview.webhook}</span> : <span className="cap">Не указан: события не приходят.</span>}
        </span>
      </div>
      <div className="row">
        <span className="rl">Страница в админке</span>
        <span className="rc">
          {preview.page ? (
            <>
              <span>«{preview.page.title}»</span>
              <span className="mono ext-url">{preview.page.url}</span>
              <span className="cap">Открывается во встроенном окне у тех, у кого есть право «{preview.page.permission_title}».</span>
            </>
          ) : (
            <span className="cap">Нет.</span>
          )}
        </span>
      </div>
      <div className="row">
        <span className="rl">Настройки</span>
        <span className="rc">
          {preview.settings.length ? <span>{preview.settings.join(', ')}</span> : <span className="cap">Нет.</span>}
        </span>
      </div>
    </section>
  );
}

export function ExtInstall() {
  const [params] = useSearchParams();
  const update = params.get('update');
  const navigate = useNavigate();
  const input = useRef<HTMLInputElement>(null);
  const [fileName, setFileName] = useState<string | null>(null);
  const [source, setSource] = useState<string | null>(null);
  const [result, setResult] = useState<PreviewResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<InstallResult | null>(null);

  const reset = () => {
    setFileName(null);
    setSource(null);
    setResult(null);
    setError(null);
    if (input.current) {
      input.current.value = '';
    }
  };

  const pick = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (!file) {
      return;
    }
    setResult(null);
    setError(null);
    setDone(null);
    setFileName(file.name);
    if (file.size > MANIFEST_LIMIT) {
      setSource(null);
      setError('Файл больше 64 КБ — это не похоже на описание модуля');
      return;
    }
    setBusy(true);
    try {
      const text = await readText(file);
      setSource(text);
      setResult(await api<PreviewResult>('/api/admin/ext-modules/preview', { method: 'POST', body: { manifest: text, update } }));
    } catch (err) {
      setError(failText(err, 'Файл не проверился'));
    } finally {
      setBusy(false);
    }
  };

  const confirm = async () => {
    const preview = result?.preview;
    if (!preview || source === null) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const installed = await api<InstallResult>('/api/admin/ext-modules', {
        method: 'POST',
        body: {
          manifest: source,
          update,
          permissions: preview.permissions.filter((p) => p.change !== 'removed').map((p) => p.key),
          events: preview.events.filter((e) => e.change !== 'removed').map((e) => e.kind),
        },
      });
      if (installed.credentials) {
        setDone(installed);
      } else {
        navigate(extLink(installed.module.id), { state: { note: `Модуль обновлён до версии ${installed.module.version}` } });
      }
    } catch (err) {
      setError(failText(err, 'Модуль не установился'));
    } finally {
      setBusy(false);
    }
  };

  const preview = result?.preview ?? null;
  const updating = Boolean(update || preview?.installed);

  if (done?.credentials) {
    return (
      <div className="md-page">
        <Link to="/admin/modules" className="md-back">
          Все модули
        </Link>
        <div className="md-title">
          <h1 className="page-title tight">Модуль «{done.module.name}» установлен</h1>
          <p>Он уже включён: события начнут уходить на его адрес. Передайте модулю ключ и секрет — без них он не сможет проверить подпись и обратиться к API.</p>
        </div>
        <CredentialsBlock credentials={done.credentials} onDone={() => navigate(extLink(done.module.id))} />
        <Link to={extLink(done.module.id)} className="md-back">
          Открыть карточку модуля
        </Link>
      </div>
    );
  }

  return (
    <div className="md-page">
      <Link to={update ? extLink(update) : '/admin/modules'} className="md-back">
        {update ? 'Карточка модуля' : 'Все модули'}
      </Link>
      <div className="md-title">
        <h1 className="page-title tight">{update ? 'Обновление модуля' : 'Установка модуля'}</h1>
        <p>
          Внешний модуль — отдельная программа на вашем сервере или где угодно. Панель не запускает его код: она шлёт модулю события с подписью и выдаёт ключ API с
          правами, которые вы подтвердите. Как описать модуль — в docs/MODULES.md.
        </p>
      </div>
      <section className="sec" aria-labelledby="ext-file-h">
        <div className="sec-head">
          <h2 id="ext-file-h" className="section-title">
            Файл описания
          </h2>
          <span className="cap">JSON до 64 КБ от автора модуля: имя, адрес для событий, нужные права и события.</span>
        </div>
        <div className="row adm-row">
          <span className="adm-who">
            <span className="adm-email">{fileName ?? 'Файл не выбран'}</span>
            {busy && !preview ? <span className="adm-status">Проверяем файл…</span> : null}
          </span>
          <span className="adm-acts">
            <input ref={input} id="ext-file" type="file" accept=".json,application/json" className="sr-only" onChange={(e) => void pick(e)} disabled={busy} />
            <label htmlFor="ext-file" className="b34 hov ext-pick" aria-disabled={busy || undefined}>
              {fileName ? 'Выбрать другой' : 'Выбрать файл'}
            </label>
          </span>
        </div>
        {result && result.errors.length ? (
          <div className="row">
            <span className="rc">
              <span className="err" role="alert">
                В файле {result.errors.length === 1 ? 'ошибка' : 'ошибки'}, модуль не установить:
              </span>
              <ul className="ext-problems">
                {result.errors.map((p, i) => (
                  <li key={`${p.path}-${i}`}>{problemText(p)}</li>
                ))}
              </ul>
            </span>
          </div>
        ) : null}
      </section>
      {preview ? (
        <>
          <section className="card md-state" aria-label="Модуль из файла">
            <div className="md-text">
              <span className="md-name">
                {preview.name} <span className="muted">{preview.version}</span>
              </span>
              {preview.description ? <span className="md-desc">{preview.description}</span> : null}
              <span className="md-where">
                Автор: {preview.author}
                {preview.site ? (
                  <>
                    {' · '}
                    <a href={preview.site} target="_blank" rel="noreferrer noopener" className="md-link-inline">
                      сайт модуля
                    </a>
                  </>
                ) : null}
              </span>
              {preview.installed ? (
                <span className="md-note">
                  Уже установлена версия {preview.installed.version}: файл обновит модуль, ключ и секрет останутся прежними.
                </span>
              ) : null}
            </div>
          </section>
          <PreviewCard preview={preview} />
          {error ? (
            <p className="err" role="alert">
              {error}
            </p>
          ) : null}
          <div className="md-actions">
            <button type="button" className="btn-main btn" onClick={() => void confirm()} disabled={busy}>
              {busy ? 'Сохраняем…' : updating ? 'Подтвердить и обновить' : 'Подтвердить и установить'}
            </button>
            <button type="button" className="btn-line hov" onClick={reset} disabled={busy}>
              Отменить
            </button>
          </div>
        </>
      ) : error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
