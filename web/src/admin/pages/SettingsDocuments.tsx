import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { Switch } from '../ui';
import '../form.css';
import './documents.css';
import { DOCS_API, DOCS_PAGE, hostOf, listItem, type DocRow, type DocsView } from './documentsModel';
import { SettingsLayout } from './SettingsLayout';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function Where({ row, host }: { row: DocRow; host: string | null }) {
  if (row.kind === 'page') {
    return (
      <span className="dl-where">
        Страница <b>{`${host ?? ''}${row.path ?? ''}`}</b>
        {row.english ? ' · есть английский текст' : ' · только на русском'}
      </span>
    );
  }
  return (
    <span className="dl-where">
      Ссылка <b>{row.url}</b>
    </span>
  );
}

export function SettingsDocuments() {
  const [data, setData] = useState<DocsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api<DocsView>(DOCS_API));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Документы не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (!data) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем документы…</p>
        )}
      </SettingsLayout>
    );
  }

  const save = async (items: DocRow[], consent: boolean, done: string) => {
    setBusy(true);
    setStatus(null);
    try {
      setData(await api<DocsView>(DOCS_API, { method: 'PUT', body: { consent, items: items.map(listItem) } }));
      setStatus({ text: done, bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Документы не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const move = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= data.items.length) {
      return;
    }
    const next = [...data.items];
    const [moved] = next.splice(index, 1);
    if (moved) {
      next.splice(target, 0, moved);
    }
    void save(next, data.consent, 'Порядок сохранён');
  };

  const host = hostOf(data.base_url);
  const full = data.items.length >= data.max;
  const pagesWithoutHost = !host && data.items.some((row) => row.kind === 'page');

  return (
    <SettingsLayout>
      <section className="sec" aria-labelledby="doc-h">
        <div className="sec-head">
          <h2 id="doc-h" className="section-title">
            Документы
          </h2>
          <span className="muted">
            Оферта, политика конфиденциальности и другие правила. Документ — страница на вашем сайте или ссылка на другой сайт. Ссылки появятся в боте (кнопка «Документы»), внизу лендинга, в кабинете и в мини-приложении.
          </span>
        </div>
        {pagesWithoutHost ? (
          <div className="dl-note">
            <span className="warn-line">
              Не задан адрес сервиса («Настройки → Общие → Адрес сервиса»), поэтому бот не может дать ссылку на страницы. На сайте они работают.
            </span>
          </div>
        ) : null}
        {data.items.length === 0 ? (
          <div className="dl-note">
            <span className="cap">Документов пока нет.</span>
          </div>
        ) : null}
        {data.items.map((row, index) => (
          <div key={row.id} className="dl-row">
            <span className="dl-num num">{index + 1}</span>
            <span className="dl-main">
              <span className="dl-title">{row.title}</span>
              <Where row={row} host={host} />
            </span>
            <span className="dl-actions">
              <Link to={`${DOCS_PAGE}/${row.id}`} className="b34 hov dl-edit" aria-label={`Изменить «${row.title}»`}>
                Изменить
              </Link>
              <button type="button" className="dl-icon hov" aria-label={`«${row.title}» выше`} onClick={() => move(index, -1)} disabled={busy || index === 0}>
                <Icon name="arrow-up" size={18} />
              </button>
              <button
                type="button"
                className="dl-icon hov"
                aria-label={`«${row.title}» ниже`}
                onClick={() => move(index, 1)}
                disabled={busy || index === data.items.length - 1}
              >
                <Icon name="arrow-down" size={18} />
              </button>
            </span>
          </div>
        ))}
        <div className="dl-add">
          {full ? (
            <span className="cap">Документов уже {data.max}: чтобы добавить новый, удалите один из них</span>
          ) : (
            <>
              <Link to={`${DOCS_PAGE}/new?kind=page`} className="b34 hov dl-edit">
                Новая страница
              </Link>
              <Link to={`${DOCS_PAGE}/new?kind=link`} className="b34 hov dl-edit">
                Ссылка на другой сайт
              </Link>
              <span className="cap">До {data.max} документов</span>
            </>
          )}
        </div>
        <div className="toggle-row">
          <div>
            <strong>Спрашивать согласие в боте</strong>
            <span className="cap">
              Пока клиент не нажмёт «Принимаю», бот не откроет меню. Если изменить названия, адреса или порядок документов, согласие спросят заново. Правка текста страницы согласие не сбрасывает.
            </span>
          </div>
          <Switch
            on={data.consent}
            onToggle={() => void save(data.items, !data.consent, data.consent ? 'Согласие больше не спрашивается' : 'Согласие будет спрашиваться')}
            label="Спрашивать согласие в боте"
            disabled={busy || data.items.length === 0}
          />
        </div>
      </section>
      {status ? (
        <p role="status" className={status.bad ? 'err' : 'muted'}>
          {status.text}
        </p>
      ) : null}
    </SettingsLayout>
  );
}
