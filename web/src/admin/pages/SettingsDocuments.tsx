import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { Switch } from '../ui';
import '../form.css';
import './bot.css';
import { SettingsLayout } from './SettingsLayout';

type Document = { title: string; url: string };
type Documents = { consent: boolean; items: Document[]; max: number; title_max: number };
type Problem = { title: string | null; url: string | null };

const NONE: Problem = { title: null, url: null };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function urlProblem(url: string): string | null {
  const value = url.trim();
  if (!value) {
    return 'нужна ссылка';
  }
  if (!/^https:\/\/[^\s"'<>]+$/.test(value) || value.length <= 'https://'.length) {
    return 'ссылка вида https://…';
  }
  return null;
}

export function SettingsDocuments() {
  const [data, setData] = useState<Documents | null>(null);
  const [items, setItems] = useState<Document[]>([]);
  const [consent, setConsent] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);

  const apply = useCallback((next: Documents) => {
    setData(next);
    setItems(next.items.map((d) => ({ ...d })));
    setConsent(next.consent);
  }, []);

  const load = useCallback(async () => {
    try {
      apply(await api<Documents>('/api/admin/settings/documents'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Документы не загрузились'));
    }
  }, [apply]);

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

  const dirty = consent !== data.consent || JSON.stringify(items) !== JSON.stringify(data.items);
  const problems = items.map((item) => ({
    title: item.title.trim() ? (item.title.trim().length > data.title_max ? `не длиннее ${data.title_max} символов` : null) : 'нужно название',
    url: urlProblem(item.url),
  }));
  const invalid = problems.some((p) => p.title || p.url);

  const change = (index: number, patch: Partial<Document>) => {
    setItems(items.map((item, i) => (i === index ? { ...item, ...patch } : item)));
    setStatus(null);
  };

  const move = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= items.length) {
      return;
    }
    const next = [...items];
    const [moved] = next.splice(index, 1);
    if (moved) {
      next.splice(target, 0, moved);
    }
    setItems(next);
    setStatus(null);
  };

  const save = async () => {
    if (!dirty || busy) {
      return;
    }
    if (invalid) {
      setStatus({ text: 'Исправьте поля, отмеченные красным', bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      apply(
        await api<Documents>('/api/admin/settings/documents', {
          method: 'PUT',
          body: { consent, items: items.map((d) => ({ title: d.title.trim(), url: d.url.trim() })) },
        }),
      );
      setStatus({ text: 'Сохранено', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Документы не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsLayout>
      <section className="sec" aria-labelledby="doc-h">
        <div className="sec-head">
          <h2 id="doc-h" className="section-title">
            Документы
          </h2>
          <span className="muted">Оферта, политика конфиденциальности и другие правила. Ссылки появятся в боте (кнопка «Документы»), на сайте и в кабинете внизу страницы.</span>
        </div>
        {items.length === 0 ? (
          <div className="row">
            <span className="cap">Документов пока нет.</span>
          </div>
        ) : null}
        {items.map((item, index) => (
          <div key={index} className="row doc-row">
            <span className="doc-num num">{index + 1}</span>
            <span className="doc-fields">
              <input
                className="field"
                value={item.title}
                placeholder="Название, например «Публичная оферта»"
                maxLength={data.title_max + 20}
                aria-label={`Название документа ${index + 1}`}
                aria-invalid={(problems[index] ?? NONE).title !== null}
                onChange={(e) => change(index, { title: e.target.value })}
                disabled={busy}
              />
              <input
                className="field mono"
                value={item.url}
                placeholder="https://example.com/offer"
                inputMode="url"
                aria-label={`Ссылка на документ ${index + 1}`}
                aria-invalid={(problems[index] ?? NONE).url !== null}
                onChange={(e) => change(index, { url: e.target.value })}
                disabled={busy}
              />
              {(problems[index] ?? NONE).title || (problems[index] ?? NONE).url ? (
                <span className="err">{[(problems[index] ?? NONE).title, (problems[index] ?? NONE).url].filter(Boolean).join('; ')}</span>
              ) : null}
            </span>
            <span className="doc-actions">
              <button type="button" className="doc-icon hov" aria-label="Выше" onClick={() => move(index, -1)} disabled={busy || index === 0}>
                <Icon name="arrow-up" size={18} />
              </button>
              <button
                type="button"
                className="doc-icon hov"
                aria-label="Ниже"
                onClick={() => move(index, 1)}
                disabled={busy || index === items.length - 1}
              >
                <Icon name="arrow-down" size={18} />
              </button>
              <button
                type="button"
                className="doc-icon hov"
                aria-label={`Удалить документ ${index + 1}`}
                onClick={() => {
                  setItems(items.filter((_, i) => i !== index));
                  setStatus(null);
                }}
                disabled={busy}
              >
                <Icon name="close" size={18} />
              </button>
            </span>
          </div>
        ))}
        <div className="row">
          <button
            type="button"
            className="b34 hov"
            onClick={() => {
              setItems([...items, { title: '', url: '' }]);
              setStatus(null);
            }}
            disabled={busy || items.length >= data.max}
          >
            Добавить документ
          </button>
          {items.length >= data.max ? <span className="cap">Не больше {data.max} документов</span> : null}
        </div>
        <div className="toggle-row">
          <div>
            <strong>Спрашивать согласие в боте</strong>
            <span className="cap">
              Пока клиент не нажмёт «Принимаю», бот не откроет меню. Если изменить список документов, согласие спросят заново.
            </span>
          </div>
          <Switch on={consent} onToggle={() => setConsent(!consent)} label="Спрашивать согласие в боте" disabled={busy || items.length === 0} />
        </div>
      </section>
      <div className="save-row">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || !dirty}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" disabled={busy || !dirty} onClick={() => apply(data)}>
          Отменить
        </button>
        <span role="status" className={status?.bad ? 'bad' : undefined}>
          {status?.text ?? (dirty ? 'Есть несохранённые изменения' : '')}
        </span>
      </div>
    </SettingsLayout>
  );
}
