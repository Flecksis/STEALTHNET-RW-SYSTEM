import { useCallback, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { cleanHtml, textLength } from '../../lib/cleanHtml';
import { count, dateLong } from '../../lib/format';
import type { Lang } from '../../lib/i18n';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import type { Theme } from '../../lib/theme';
import '../form.css';
import './documents.css';
import { DocumentPreviewFrame, PreviewTools, useSize, useViewportHeight, type Device, type PreviewData } from './DocumentPreviewFrame';
import { DOCS_API, DOCS_PAGE, hostOf, normalizePath, pathProblem, titleProblem, urlProblem, type DocFull, type DocKind, type DocsView } from './documentsModel';
import { RichText, type Body } from './RichText';

type Draft = {
  kind: DocKind;
  title: string;
  title_en: string;
  url: string;
  path: string;
  body_ru: Body;
  body_en: Body;
};

type Cleaned = { source: string; html: string; chars: number };


function fromDoc(doc: DocFull): Draft {
  return {
    kind: doc.kind,
    title: doc.title,
    title_en: doc.title_en ?? '',
    url: doc.url ?? '',
    path: (doc.path ?? '').replace(/^\/+/, ''),
    body_ru: { html: doc.body_ru ?? '', raw: false },
    body_en: { html: doc.body_en ?? '', raw: false },
  };
}

function blank(kind: DocKind): Draft {
  return { kind, title: '', title_en: '', url: '', path: '', body_ru: { html: '', raw: false }, body_en: { html: '', raw: false } };
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function SettingsDocument() {
  const { id = 'new' } = useParams();
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const phone = usePhone();
  const viewport = useViewportHeight();
  const fresh = id === 'new';
  const [meta, setMeta] = useState<DocsView | null>(null);
  const [saved, setSaved] = useState<DocFull | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [touched, setTouched] = useState(false);
  const [lang, setLang] = useState<Lang>('ru');
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [asking, setAsking] = useState(false);
  const [device, setDevice] = useState<Device>('phone');
  const [theme, setTheme] = useState<Theme>('light');
  const [sheet, setSheet] = useState(false);
  const [sheetBody, setSheetBody] = useState<HTMLDivElement | null>(null);
  const sheetSize = useSize(sheetBody);
  const [remote, setRemote] = useState<{ ru: Cleaned | null; en: Cleaned | null }>({ ru: null, en: null });

  const cleanRemote = useCallback(async (raw: string) => api<{ html: string; chars: number }>(`${DOCS_API}/clean`, { method: 'POST', body: { html: raw } }), []);
  const cleanHtmlRemote = useCallback(async (raw: string) => (await cleanRemote(raw)).html, [cleanRemote]);

  const rawRu = draft?.body_ru.raw ? draft.body_ru.html : null;
  const rawEn = draft?.body_en.raw ? draft.body_en.html : null;
  useEffect(() => {
    const jobs = (
      [
        ['ru', rawRu],
        ['en', rawEn],
      ] as const
    ).filter((job): job is readonly ['ru' | 'en', string] => job[1] !== null);
    if (!jobs.length) {
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      for (const [which, source] of jobs) {
        cleanRemote(source)
          .then((view) => {
            if (alive) {
              setRemote((current) => ({ ...current, [which]: { source, ...view } }));
            }
          })
          .catch(() => undefined);
      }
    }, 300);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [rawRu, rawEn, cleanRemote]);

  const resolve = useCallback(
    (body: Body, which: 'ru' | 'en'): { html: string; chars: number } | null => {
      if (!body.raw) {
        const html = cleanHtml(body.html);
        return { html, chars: textLength(html) };
      }
      const known = remote[which];
      return known ? { html: known.html, chars: known.chars } : null;
    },
    [remote],
  );

  const load = useCallback(async () => {
    try {
      const view = await api<DocsView>(DOCS_API);
      setMeta(view);
      if (fresh) {
        const kind: DocKind = search.get('kind') === 'link' ? 'link' : 'page';
        setSaved(null);
        setDraft(blank(kind));
      } else {
        const doc = await api<DocFull>(`${DOCS_API}/${encodeURIComponent(id)}`);
        setSaved(doc);
        setDraft(fromDoc(doc));
      }
      setTouched(false);
      setLoadError(null);
    } catch (err) {
      setLoadError(err instanceof ApiError && err.status === 404 ? 'Такого документа нет: возможно, его уже удалили' : failText(err, 'Документ не загрузился'));
    }
  }, [fresh, id, search]);

  useEffect(() => {
    void load();
  }, [load]);

  const preview: PreviewData | null = useMemo(() => {
    if (!draft || draft.kind !== 'page') {
      return null;
    }
    const en = resolve(draft.body_en, 'en');
    const english = lang === 'en' && en !== null && en.chars > 0;
    const title = english ? draft.title_en.trim() || draft.title : draft.title;
    return {
      title: title.trim() || 'Без названия',
      html: (english ? en?.html : resolve(draft.body_ru, 'ru')?.html) ?? '',
      lang: english ? 'en' : 'ru',
      path: draft.path ? `/${normalizePath(draft.path)}` : null,
      updatedAt: saved?.updated_at ?? null,
    };
  }, [draft, lang, saved, resolve]);

  if (loadError || !draft || !meta) {
    return (
      <div className="page-body dce">
        <Link to={DOCS_PAGE} className="dce-back">
          <Icon name="chevron-left" size={18} />
          Настройки · Документы
        </Link>
        {loadError ? (
          <p className="err" role="alert">
            {loadError}
          </p>
        ) : (
          <p className="muted">Загружаем документ…</p>
        )}
      </div>
    );
  }

  const page = draft.kind === 'page';
  const host = hostOf(meta.base_url) ?? window.location.host;
  const change = (patch: Partial<Draft>) => {
    setDraft({ ...draft, ...patch });
    setTouched(true);
    setStatus(null);
  };
  const bodyKey = lang === 'en' ? 'body_en' : 'body_ru';
  const ruClean = resolve(draft.body_ru, 'ru');
  const enClean = resolve(draft.body_en, 'en');
  const chars = (lang === 'en' ? enClean : ruClean)?.chars ?? null;
  const problems = {
    title: titleProblem(draft.title, meta.title_max, true),
    title_en: titleProblem(draft.title_en, meta.title_max, false),
    url: page ? null : urlProblem(draft.url),
    path: page ? pathProblem(draft.path, meta.path_max) : null,
    body: page && ruClean !== null && ruClean.chars === 0 ? 'добавьте текст на русском' : null,
  };
  const firstProblem = Object.values(problems).find(Boolean) ?? null;

  const save = async () => {
    if (busy) {
      return;
    }
    if (firstProblem) {
      setStatus({ text: `Исправьте: ${firstProblem}`, bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    const body = page
      ? {
          kind: 'page',
          title: draft.title.trim(),
          title_en: draft.title_en.trim() || null,
          path: normalizePath(draft.path),
          body_ru: draft.body_ru.raw ? draft.body_ru.html : cleanHtml(draft.body_ru.html),
          body_en: draft.body_en.raw ? draft.body_en.html : enClean && enClean.chars > 0 ? enClean.html : null,
        }
      : { kind: 'link', title: draft.title.trim(), title_en: draft.title_en.trim() || null, url: draft.url.trim() };
    try {
      const doc = await api<DocFull>(fresh ? DOCS_API : `${DOCS_API}/${encodeURIComponent(id)}`, { method: fresh ? 'POST' : 'PUT', body });
      setSaved(doc);
      setDraft(fromDoc(doc));
      setTouched(false);
      setStatus({ text: 'Сохранено', bad: false });
      if (fresh) {
        navigate(`${DOCS_PAGE}/${doc.id}`, { replace: true });
      }
    } catch (err) {
      setStatus({ text: failText(err, 'Документ не сохранился'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const cancel = () => {
    setDraft(saved ? fromDoc(saved) : blank(draft.kind));
    setTouched(false);
    setStatus(null);
  };

  const remove = async () => {
    setAsking(false);
    setBusy(true);
    try {
      await api<DocsView>(`${DOCS_API}/${encodeURIComponent(id)}`, { method: 'DELETE' });
      navigate(DOCS_PAGE);
    } catch (err) {
      setStatus({ text: failText(err, 'Документ не удалился'), bad: true });
      setBusy(false);
    }
  };

  const heading = saved ? saved.title : page ? 'Новая страница' : 'Новая ссылка';
  const openHref = saved && !touched ? (saved.kind === 'page' ? saved.path : saved.url) : null;
  const statusLine = status ? (
    <span role="status" className={status.bad ? 'bad' : undefined}>
      {status.text}
    </span>
  ) : (
    <span role="status">{touched ? 'Есть несохранённые изменения' : saved?.updated_at && page ? `Текст обновлён ${dateLong(saved.updated_at, true)}` : ''}</span>
  );

  const head = (
    <>
      <Link to={DOCS_PAGE} className="dce-back">
        <Icon name="chevron-left" size={18} />
        Настройки · Документы
      </Link>
      <div className="dce-head">
        <div className="head-stack">
          <h1 className="page-title tight">{heading}</h1>
          <span className="muted">{page ? 'Страница на вашем сайте. Изменения появятся там после сохранения' : 'Ссылка на документ на другом сайте'}</span>
        </div>
        {openHref ? (
          <a href={openHref} target="_blank" rel="noopener noreferrer" className="dce-open">
            {page ? 'Открыть на сайте' : 'Открыть ссылку'}
            <Icon name="external" size={16} />
          </a>
        ) : null}
      </div>
    </>
  );

  const langSwitch = (
    <div role="group" aria-label="Язык текста" className="segs dce-lang">
      <button type="button" aria-pressed={lang === 'ru'} onClick={() => setLang('ru')}>
        Русский
      </button>
      <button type="button" aria-pressed={lang === 'en'} onClick={() => setLang('en')}>
        English
      </button>
    </div>
  );

  const titleField = (
    <label className="dce-label">
      <span>{lang === 'en' ? 'Название на английском' : 'Название'}</span>
      {lang === 'en' ? (
        <input
          className="field"
          value={draft.title_en}
          placeholder={draft.title ? `Пусто — покажется «${draft.title}»` : 'По желанию'}
          maxLength={meta.title_max + 20}
          aria-invalid={problems.title_en !== null}
          onChange={(e) => change({ title_en: e.target.value })}
          disabled={busy}
        />
      ) : (
        <input
          className="field"
          value={draft.title}
          placeholder="Например, «Политика конфиденциальности»"
          maxLength={meta.title_max + 20}
          aria-invalid={problems.title !== null}
          onChange={(e) => change({ title: e.target.value })}
          disabled={busy}
        />
      )}
      {lang === 'en' ? problems.title_en ? <span className="err">{problems.title_en}</span> : null : problems.title && touched ? <span className="err">{problems.title}</span> : null}
    </label>
  );

  const pathField = (
    <label className="dce-label">
      <span>Адрес страницы</span>
      <span className="dce-addr" aria-invalid={touched && problems.path !== null}>
        <span className="dce-addr-host">{host}/</span>
        <input
          value={draft.path}
          placeholder="privacy-policy"
          spellCheck={false}
          autoCapitalize="off"
          maxLength={meta.path_max + 10}
          aria-label="Адрес страницы"
          onChange={(e) => change({ path: e.target.value })}
          disabled={busy}
        />
      </span>
      {touched && problems.path ? <span className="err">{problems.path}</span> : null}
    </label>
  );

  const editor = (
    <RichText
      key={`${saved?.id ?? 'new'}-${lang}-${saved?.updated_at ?? ''}`}
      value={draft[bodyKey]}
      onChange={(body) => change({ [bodyKey]: body } as Partial<Draft>)}
      clean={cleanHtmlRemote}
      label={lang === 'en' ? 'Текст страницы на английском' : 'Текст страницы'}
      placeholder={lang === 'en' ? 'Английский текст — по желанию. Пока его нет, на английском сайте покажется русский' : 'Текст документа. Можно вставить из Word или Google Docs'}
      disabled={busy}
    />
  );

  const editorFoot = (
    <div className="dce-foot">
      <span>{lang === 'en' ? 'Английский — по желанию. Пока его нет, на английском сайте покажется русский текст' : 'Вставка из Word и Google Docs сохраняет заголовки, списки, ссылки и таблицы'}</span>
      <span className="num">{chars === null ? 'Проверяем код…' : count(chars, 'знак', 'знака', 'знаков')}</span>
    </div>
  );

  const saveRow = (
    <div className="dce-save">
      <button type="button" className="btn-main" onClick={() => void save()} disabled={busy || (!touched && !fresh)}>
        {busy ? 'Сохраняем…' : fresh ? (page ? 'Создать страницу' : 'Добавить ссылку') : 'Сохранить'}
      </button>
      {touched && !fresh ? (
        <button type="button" className="btn-line hov" onClick={cancel} disabled={busy}>
          Отменить
        </button>
      ) : null}
      {page && phone ? (
        <button type="button" className="btn-line hov" onClick={() => setSheet(true)}>
          Превью
        </button>
      ) : null}
      {statusLine}
      <span className="grow" />
      {saved && !phone ? (
        <button type="button" className="dce-del" onClick={() => setAsking(true)} disabled={busy}>
          Удалить документ
        </button>
      ) : null}
    </div>
  );

  const dialog = asking ? (
    <div className="dialog-back" role="presentation" onClick={() => setAsking(false)}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="dce-del-h" onClick={(event) => event.stopPropagation()}>
        <h2 id="dce-del-h">Удалить «{saved?.title}»?</h2>
        <p className="muted">
          {page
            ? 'Страница пропадёт с сайта, а ссылки на неё — из бота, кабинета, мини-приложения и подвала лендинга. Текст удалится насовсем.'
            : 'Ссылка пропадёт из бота, кабинета, мини-приложения и подвала лендинга.'}
        </p>
        <div className="dialog-actions">
          <button type="button" className="btn-main" onClick={() => void remove()} autoFocus>
            Удалить
          </button>
          <button type="button" className="btn-line hov" onClick={() => setAsking(false)}>
            Отменить
          </button>
        </div>
      </div>
    </div>
  ) : null;

  const phoneDelete =
    saved && phone ? (
      <button type="button" className="dce-del dce-del-phone" onClick={() => setAsking(true)} disabled={busy}>
        Удалить документ
      </button>
    ) : null;

  if (!page) {
    return (
      <div className="page-body dce">
        {head}
        <section className="sec">
          <div className="dce-fields">
            <label className="dce-label">
              <span>Название</span>
              <input
                className="field"
                value={draft.title}
                placeholder="Например, «Правила возврата»"
                maxLength={meta.title_max + 20}
                aria-invalid={touched && problems.title !== null}
                onChange={(e) => change({ title: e.target.value })}
                disabled={busy}
              />
              {touched && problems.title ? <span className="err">{problems.title}</span> : null}
            </label>
            <label className="dce-label">
              <span>Название на английском</span>
              <input
                className="field"
                value={draft.title_en}
                placeholder="По желанию"
                maxLength={meta.title_max + 20}
                aria-invalid={problems.title_en !== null}
                onChange={(e) => change({ title_en: e.target.value })}
                disabled={busy}
              />
              {problems.title_en ? <span className="err">{problems.title_en}</span> : null}
            </label>
            <label className="dce-label wide">
              <span>Ссылка</span>
              <input
                className="field mono"
                value={draft.url}
                placeholder="https://example.com/refunds"
                inputMode="url"
                spellCheck={false}
                aria-invalid={touched && problems.url !== null}
                onChange={(e) => change({ url: e.target.value })}
                disabled={busy}
              />
              {touched && problems.url ? <span className="err">{problems.url}</span> : null}
            </label>
          </div>
        </section>
        {saveRow}
        {phoneDelete}
        {dialog}
      </div>
    );
  }

  const previewHeight = Math.max(460, Math.min(900, viewport - 220));

  return (
    <div className="page-body dce">
      {head}
      <div className={phone ? 'dce-grid single' : 'dce-grid'}>
        <section className="sec dce-main">
          <div className="dce-fields">
            {titleField}
            {pathField}
          </div>
          <div className="dce-bar">
            {langSwitch}
            <span className="cap">Английский — по желанию. Пока его нет, на английском сайте покажется русский текст</span>
          </div>
          {editor}
          {editorFoot}
        </section>
        {phone ? null : (
          <aside className="card dce-pv" aria-label="Превью страницы">
            <PreviewTools device={device} theme={theme} onDevice={setDevice} onTheme={setTheme} phoneFirst />
            {preview ? <DocumentPreviewFrame data={preview} device={device} theme={theme} height={previewHeight} /> : null}
          </aside>
        )}
      </div>
      {saveRow}
      {phoneDelete}
      {dialog}
      {sheet ? (
        <div className="dce-sheet" role="dialog" aria-modal="true" aria-label="Превью страницы">
          <div className="dce-sheet-head">
            <span className="dce-sheet-title">Превью</span>
            <button type="button" className="dce-icon hov" aria-label="Закрыть превью" onClick={() => setSheet(false)} autoFocus>
              <Icon name="close" size={20} />
            </button>
            <PreviewTools device={device} theme={theme} onDevice={setDevice} onTheme={setTheme} phoneFirst />
          </div>
          <div className="dce-sheet-body" ref={setSheetBody}>
            {sheetSize.height > 0 && preview ? <DocumentPreviewFrame data={preview} device={device} theme={theme} height={sheetSize.height} /> : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}
