import { useEffect, useId, useRef, useState, type MouseEvent } from 'react';
import { useLocation } from 'react-router';
import { api } from '../../lib/api';
import { useMedia, usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { ROLE_LABEL, useSession } from '../session';
import { buildText, shortCommit, shortName, type BuildSummary } from '../version';
import { caughtLines, useCaught } from './errors';
import {
  ISSUE_FORM,
  ISSUES_REPO,
  NO_TITLE,
  REPORT_PATH,
  browserName,
  clean,
  composeBody,
  fitIssueUrl,
  pagePath,
  stamp,
  systemName,
  titleFrom,
} from './text';
import '../form.css';
import './report.css';

type Check = { version: string | null; supported: boolean; error: string | null };

function panelLine(build: BuildSummary): string {
  const commit = shortCommit(build.commit);
  const fresh = build.update ? `; вышла ${shortName(build.update.label, build.update.commit, build.update.built_at)}` : '';
  return `${buildText(build)}${commit ? `, ${commit}` : ''}${fresh}`;
}

async function remnaLine(canCheck: boolean): Promise<string> {
  const aim = await api<{ remnawave?: string }>('/api/version')
    .then((info) => (info.remnawave ? `сборка рассчитана на ${info.remnawave}` : null))
    .catch(() => null);
  if (!canCheck) {
    return ['нет данных: нужен доступ к настройкам', aim].filter(Boolean).join('; ');
  }
  try {
    const check = await api<Check>('/api/admin/settings/remnawave/check', { method: 'POST' });
    if (check.version) {
      return check.supported || !aim ? check.version : `${check.version}, ${aim}`;
    }
    return check.error ? `не ответила: ${check.error.slice(0, 160)}` : 'не ответила';
  } catch {
    return 'не удалось проверить';
  }
}

function errorsNote(total: number, shown: number): string {
  if (!total) {
    return 'В этой вкладке сбоев скриптов не было.';
  }
  const head = total > shown ? `Сбоев скриптов в этой вкладке: ${total}, в отчёт попадут последние ${shown}.` : `Сбоев скриптов в этой вкладке: ${total}.`;
  return `${head} Почты, ссылки и токены из них убраны.`;
}

function screenLine(): string {
  const ratio = Math.round(window.devicePixelRatio * 100) / 100;
  const scale = ratio && ratio !== 1 ? `, масштаб ${ratio}` : '';
  return `${window.screen.width}×${window.screen.height}, окно ${window.innerWidth}×${window.innerHeight}${scale}`;
}

async function copyText(value: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(value);
    return true;
  } catch {
    const area = document.createElement('textarea');
    area.value = value;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    const done = document.execCommand('copy');
    area.remove();
    return done;
  }
}

export function BugReport() {
  const { me, can, theme } = useSession();
  const phone = usePhone();
  const narrow = useMedia('(max-width: 760px)');
  const location = useLocation();
  const caught = useCaught();
  const whatRef = useRef<HTMLTextAreaElement>(null);
  const ids = { what: useId(), steps: useId(), expected: useId(), title: useId(), text: useId(), head: useId(), body: useId() };
  const [what, setWhat] = useState('');
  const [steps, setSteps] = useState('');
  const [expected, setExpected] = useState('');
  const [withErrors, setWithErrors] = useState(true);
  const [title, setTitle] = useState<string | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [missing, setMissing] = useState(false);
  const [copied, setCopied] = useState<'ok' | 'fail' | null>(null);
  const [panel, setPanel] = useState('загружаем…');
  const [remna, setRemna] = useState('проверяем…');
  const [opened] = useState(() => new Date());
  const canCheck = can('settings');

  useEffect(() => {
    let alive = true;
    api<BuildSummary>('/api/admin/system/summary')
      .then((build) => {
        if (alive) {
          setPanel(panelLine(build));
        }
      })
      .catch(() => {
        if (alive) {
          setPanel('нет данных');
        }
      });
    void remnaLine(canCheck).then((line) => {
      if (alive) {
        setRemna(line);
      }
    });
    return () => {
      alive = false;
    };
  }, [canCheck]);

  const raw = (location.state as { from?: unknown } | null)?.from;
  const from = typeof raw === 'string' && raw.startsWith('/admin') && raw !== REPORT_PATH ? raw : null;
  const back = from ?? '/admin';
  const page = from ? pagePath(from) : null;
  const layout = phone ? 'телефонная версия' : narrow || me.appearance.admin !== 'side' ? 'меню сверху' : 'меню слева';
  const ua = navigator.userAgent;
  const browser = [
    `${browserName(ua)}, ${systemName(ua)}`,
    `экран ${screenLine()}`,
    `язык ${navigator.language || 'не указан'}`,
    `время ${stamp(opened)}`,
    `админка: ${layout}, ${theme === 'dark' ? 'тёмная' : 'светлая'} тема, роль «${ROLE_LABEL[me.admin.role]}»`,
  ].join('; ');
  const errors = caughtLines(caught);
  const attach = withErrors && errors.length > 0;
  const body =
    text ??
    composeBody({ what, steps, expected, errors: attach ? errors : null, version: panel, remnawave: remna, page: page ?? 'не указана', browser });
  const subject = title ?? titleFrom(what);
  const link = fitIssueUrl({ repo: ISSUES_REPO, form: ISSUE_FORM }, clean(subject).trim() || NO_TITLE, clean(body));
  const empty = !what.trim();

  const open = (event: MouseEvent<HTMLAnchorElement>) => {
    if (empty) {
      event.preventDefault();
      setMissing(true);
      whatRef.current?.focus();
    }
  };

  const copy = async () => {
    const done = await copyText(`${clean(subject).trim() || NO_TITLE}\n\n${clean(body)}`);
    setCopied(done ? 'ok' : 'fail');
    window.setTimeout(() => setCopied(null), 2400);
  };

  return (
    <>
      {phone ? <PhoneHeader title="Сообщить об ошибке" back={{ to: back, label: 'Назад' }} /> : <h1 className="page-title tight">Сообщить об ошибке</h1>}
      <p className="muted rep-lead">
        Отчёт откроется на GitHub новой задачей в открытом репозитории, его увидят все. Не пишите имена клиентов: почты, ссылки, адреса серверов и токены
        панель сама заменит на [скрыто].
      </p>
      <div className="rep">
        <section className="sec" aria-labelledby={ids.head}>
          <div className="sec-head">
            <h2 id={ids.head} className="section-title">
              Описание
            </h2>
          </div>
          <div className="row">
            <label className="rl" htmlFor={ids.what}>
              Что случилось
            </label>
            <span className="rc">
              <textarea
                ref={whatRef}
                id={ids.what}
                className="field"
                rows={4}
                value={what}
                required
                aria-invalid={missing && empty}
                placeholder="Например: тариф не сохраняется, появляется «Ошибка 500»"
                onChange={(e) => setWhat(e.target.value)}
              />
              {missing && empty ? (
                <span className="err" role="alert">
                  Опишите, что случилось
                </span>
              ) : null}
            </span>
          </div>
          <div className="row">
            <label className="rl" htmlFor={ids.steps}>
              Как повторить
            </label>
            <span className="rc">
              <textarea
                id={ids.steps}
                className="field"
                rows={4}
                value={steps}
                placeholder={'1. Открыть «Тарифы»\n2. Изменить цену\n3. Нажать «Сохранить»'}
                onChange={(e) => setSteps(e.target.value)}
              />
            </span>
          </div>
          <div className="row">
            <label className="rl" htmlFor={ids.expected}>
              Что ожидали
            </label>
            <span className="rc">
              <textarea
                id={ids.expected}
                className="field"
                rows={2}
                value={expected}
                placeholder="Тариф сохранится с новой ценой"
                onChange={(e) => setExpected(e.target.value)}
              />
            </span>
          </div>
          <div className="row">
            <span className="rl">Ошибки интерфейса</span>
            <span className="rc">
              <label className={errors.length ? 'rep-check' : 'rep-check off'}>
                <input type="checkbox" checked={attach} disabled={!errors.length} onChange={(e) => setWithErrors(e.target.checked)} />
                <span>
                  Приложить к отчёту
                  <span className="cap">{errorsNote(caught.length, errors.length)}</span>
                </span>
              </label>
            </span>
          </div>
        </section>
        <section className="sec" aria-labelledby={ids.body}>
          <div className="sec-head">
            <h2 id={ids.body} className="section-title">
              Текст задачи
            </h2>
            <span className="muted">Сведения о панели, странице и браузере добавлены сами. Перед отправкой текст можно поправить.</span>
          </div>
          <div className="row">
            <label className="rl" htmlFor={ids.title}>
              Заголовок
            </label>
            <span className="rc">
              <input
                id={ids.title}
                className="field"
                value={subject}
                maxLength={200}
                onChange={(e) => setTitle(e.target.value)}
                onBlur={() => {
                  if (title !== null) {
                    setTitle(clean(title));
                  }
                }}
              />
            </span>
          </div>
          <div className="row">
            <label className="rl" htmlFor={ids.text}>
              Текст
            </label>
            <span className="rc">
              <textarea
                id={ids.text}
                className="field mono rep-text"
                rows={18}
                value={body}
                spellCheck={false}
                onChange={(e) => setText(e.target.value)}
                onBlur={() => {
                  if (text !== null) {
                    setText(clean(text));
                  }
                }}
              />
              {text !== null ? (
                <span className="cap">
                  Текст изменён вручную, поля выше на него больше не влияют.{' '}
                  <button type="button" className="rep-link" onClick={() => setText(null)}>
                    Собрать заново
                  </button>
                </span>
              ) : null}
              {link.cut ? (
                <span className="warn-line">Текст длиннее, чем вмещает ссылка: на GitHub попадёт начало, конец обрежется с пометкой. Полный текст можно скопировать.</span>
              ) : null}
            </span>
          </div>
          <div className="row">
            <span className="rc">
              <span className="rep-actions">
                <a className="btn-main btn rep-go" href={link.url} target="_blank" rel="noreferrer" onClick={open}>
                  Открыть на GitHub
                </a>
                <button type="button" className="btn-line hov" onClick={() => void copy()}>
                  {copied === 'ok' ? 'Скопировано' : 'Скопировать текст'}
                </button>
              </span>
              {copied === 'fail' ? (
                <span className="err" role="alert">
                  Не скопировалось. Выделите текст и скопируйте вручную.
                </span>
              ) : null}
              <span className="cap">
                Нужен аккаунт на GitHub. Если страница не открылась, скопируйте текст и создайте задачу вручную: github.com/{ISSUES_REPO}
              </span>
            </span>
          </div>
        </section>
      </div>
    </>
  );
}
