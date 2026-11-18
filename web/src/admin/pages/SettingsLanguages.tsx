import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { Switch } from '../ui';
import '../form.css';
import './admins.css';
import { SettingsLayout } from './SettingsLayout';

type Lang = 'ru' | 'en';
type Languages = { english: boolean; default: Lang };

const NAMES: Record<Lang, string> = { ru: 'Русский', en: 'Английский' };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function SettingsLanguages() {
  const [languages, setLanguages] = useState<Languages | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setLanguages(await api<Languages>('/api/admin/settings/languages'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Настройки не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async (next: Languages) => {
    if (busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setLanguages(await api<Languages>('/api/admin/settings/languages', { method: 'PUT', body: next }));
    } catch (err) {
      setError(failText(err, 'Не сохранилось'));
    } finally {
      setBusy(false);
    }
  };

  if (!languages) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем…</p>
        )}
      </SettingsLayout>
    );
  }

  const toggleEnglish = () =>
    void save(languages.english ? { english: false, default: 'ru' } : { english: true, default: languages.default });

  return (
    <SettingsLayout>
      <div className="sec-stack">
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <section className="sec" aria-labelledby="lang-h">
          <div className="sec-head">
            <h2 id="lang-h" className="section-title">
              Языки для клиентов
            </h2>
            <span className="muted">Бот, кабинет, мини-приложение, лендинг и письма. Админка всегда на русском.</span>
          </div>
          <div className="row adm-row">
            <span className="adm-who">
              <span className="adm-email">Русский</span>
              <span className="adm-status">Включён всегда. Если для какого-то текста нет перевода, клиент видит его по-русски.</span>
            </span>
          </div>
          <div className="row adm-row">
            <span className="adm-who">
              <span className="adm-email">Английский</span>
              <span className="adm-status">
                {languages.english
                  ? 'Клиент выбирает язык в боте, кабинете и мини-приложении. Новым клиентам язык ставится по Telegram или браузеру.'
                  : 'Выключен: все клиенты видят русский, переключателя языка нет.'}
              </span>
            </span>
            <span className="adm-acts">
              <Switch on={languages.english} onToggle={toggleEnglish} label="Английский язык для клиентов" disabled={busy} />
            </span>
          </div>
        </section>
        {languages.english ? (
          <section className="sec" aria-labelledby="lang-default-h">
            <div className="sec-head">
              <h2 id="lang-default-h" className="section-title">
                Язык по умолчанию
              </h2>
              <span className="muted">
                Для новых клиентов, у которых Telegram или браузер не сообщил язык. Остальным язык выбирается сам: русский для русского, украинского, белорусского и
                казахского, английский для всех других.
              </span>
            </div>
            <div className="row">
              <span className="segs" role="group" aria-label="Язык по умолчанию">
                {(['ru', 'en'] as Lang[]).map((lang) => (
                  <button
                    key={lang}
                    type="button"
                    aria-pressed={languages.default === lang}
                    disabled={busy}
                    onClick={() => void save({ ...languages, default: lang })}
                  >
                    {NAMES[lang]}
                  </button>
                ))}
              </span>
            </div>
          </section>
        ) : null}
        <section className="sec" aria-labelledby="lang-texts-h">
          <div className="sec-head">
            <h2 id="lang-texts-h" className="section-title">
              Тексты на английском
            </h2>
            <span className="muted">
              Стандартные тексты уже переведены. Свои тексты бота на английском правьте в{' '}
              <Link className="link" to="/admin/settings/bot/texts">
                «Боте»
              </Link>
              , переключив язык над списком. Названия тарифов, документы и тексты лендинга из конструктора показываются как вы их написали.
            </span>
          </div>
        </section>
      </div>
    </SettingsLayout>
  );
}
