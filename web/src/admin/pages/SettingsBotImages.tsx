import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError, upload } from '../../lib/api';
import '../form.css';
import { BotTabs } from './BotTabs';
import { SettingsLayout } from './SettingsLayout';

type Mode = 'common' | 'own' | 'off';
type Slot = { slot: string; title: string; mode: Mode | null; version: string | null; sections: string[] };
type Images = { slots: Slot[]; max_bytes: number; min_side: number };

const MODES: Array<{ value: Mode; label: string }> = [
  { value: 'common', label: 'Общая' },
  { value: 'own', label: 'Своя' },
  { value: 'off', label: 'Без картинки' },
];

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function fileUrl(slot: Slot): string {
  return `/api/admin/bot/images/${slot.slot}/file?v=${slot.version ?? ''}`;
}

export function SettingsBotImages() {
  const [images, setImages] = useState<Images | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notes, setNotes] = useState<Record<string, { text: string; bad: boolean }>>({});

  const load = useCallback(async () => {
    try {
      setImages(await api<Images>('/api/admin/bot/images'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Картинки бота не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const note = (slot: string, text: string, bad: boolean) => setNotes((prev) => ({ ...prev, [slot]: { text, bad } }));

  const send = async (slot: string, file: File) => {
    if (!images) {
      return;
    }
    if (!['image/png', 'image/jpeg'].includes(file.type)) {
      note(slot, 'Подходят только JPEG и PNG', true);
      return;
    }
    if (file.size > images.max_bytes) {
      note(slot, `Файл больше ${Math.round(images.max_bytes / 1024 / 1024)} МБ`, true);
      return;
    }
    setBusy(slot);
    try {
      setImages(await upload<Images>(`/api/admin/bot/images/${slot}`, file));
      note(slot, 'Картинка сохранена', false);
    } catch (err) {
      note(slot, failText(err, 'Картинка не загрузилась'), true);
    } finally {
      setBusy(null);
    }
  };

  const remove = async (slot: string) => {
    setBusy(slot);
    try {
      setImages(await api<Images>(`/api/admin/bot/images/${slot}`, { method: 'DELETE' }));
      note(slot, 'Картинка удалена', false);
    } catch (err) {
      note(slot, failText(err, 'Картинка не удалилась'), true);
    } finally {
      setBusy(null);
    }
  };

  const setMode = async (slot: string, mode: Mode) => {
    setBusy(slot);
    try {
      setImages(await api<Images>(`/api/admin/bot/images/${slot}/mode`, { method: 'PUT', body: { mode } }));
      setNotes((prev) => {
        const next = { ...prev };
        delete next[slot];
        return next;
      });
    } catch (err) {
      note(slot, failText(err, 'Не получилось сохранить'), true);
    } finally {
      setBusy(null);
    }
  };

  if (!images) {
    return (
      <SettingsLayout>
        <BotTabs />
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем картинки бота…</p>
        )}
      </SettingsLayout>
    );
  }

  const common = images.slots.find((s) => s.slot === 'common');
  const sections = images.slots.filter((s) => s.slot !== 'common');

  return (
    <SettingsLayout>
      <BotTabs />
      <section className="sec" aria-labelledby="bi-h">
        <div className="sec-head">
          <h2 id="bi-h" className="section-title">
            Картинки к сообщениям
          </h2>
          <span className="muted">
            Картинка стоит над текстом сообщения. Можно поставить одну общую на все разделы или свою для раздела. JPEG или PNG, от {images.min_side} точек по
            стороне, до {Math.round(images.max_bytes / 1024 / 1024)} МБ.
          </span>
          <span className="cap">Подпись под картинкой в Telegram — до 1024 знаков. Если текст экрана длиннее, бот отправит его без картинки.</span>
        </div>
        {common ? (
          <div className="row bi-row">
            <span className="rl">
              Общая картинка
              <span className="cap bi-where">Для всех разделов, где выбрано «Общая»</span>
            </span>
            <div className="rc">
              <Picture slot={common} busy={busy === common.slot} onFile={(file) => void send(common.slot, file)} onRemove={() => void remove(common.slot)} />
              <NoteLine note={notes[common.slot]} />
            </div>
          </div>
        ) : null}
      </section>

      <section className="sec" aria-labelledby="bi-s">
        <div className="sec-head">
          <h2 id="bi-s" className="section-title">
            По разделам
          </h2>
          <span className="muted">Если у раздела своя картинка, она заменяет общую. «Без картинки» — сообщения раздела только текстом.</span>
        </div>
        {sections.map((slot) => (
          <div key={slot.slot} className="row bi-row">
            <span className="rl">
              {slot.title}
              {slot.sections.length > 1 ? <span className="cap bi-where">{slot.sections.join(', ')}</span> : null}
            </span>
            <div className="rc">
              <span className="segs" role="group" aria-label={`Картинка раздела «${slot.title}»`}>
                {MODES.map((mode) => (
                  <button
                    key={mode.value}
                    type="button"
                    aria-pressed={slot.mode === mode.value}
                    onClick={() => void setMode(slot.slot, mode.value)}
                    disabled={busy !== null}
                  >
                    {mode.label}
                  </button>
                ))}
              </span>
              {slot.mode === 'own' ? (
                <Picture slot={slot} busy={busy === slot.slot} onFile={(file) => void send(slot.slot, file)} onRemove={() => void remove(slot.slot)} />
              ) : null}
              {slot.mode === 'own' && !slot.version ? <span className="cap">Пока своей картинки нет — показывается общая.</span> : null}
              {slot.mode === 'common' ? (
                common?.version ? (
                  <img className="bi-thumb small" src={fileUrl(common)} alt="Общая картинка" />
                ) : (
                  <span className="cap">Общей картинки нет — сообщения раздела без картинки.</span>
                )
              ) : null}
              {slot.mode === 'off' ? <span className="cap">Сообщения раздела уходят без картинки.</span> : null}
              <NoteLine note={notes[slot.slot]} />
            </div>
          </div>
        ))}
      </section>
    </SettingsLayout>
  );
}

function NoteLine({ note }: { note: { text: string; bad: boolean } | undefined }) {
  if (!note) {
    return null;
  }
  return <span className={note.bad ? 'err' : 'cap'}>{note.text}</span>;
}

type PictureProps = { slot: Slot; busy: boolean; onFile: (file: File) => void; onRemove: () => void };

function Picture({ slot, busy, onFile, onRemove }: PictureProps) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <span className="bi-picture">
      {slot.version ? <img className="bi-thumb" src={fileUrl(slot)} alt={slot.title} /> : <span className="bi-empty">Картинки нет</span>}
      <span className="bi-actions">
        <input
          ref={input}
          type="file"
          accept="image/png,image/jpeg"
          hidden
          onChange={(e) => {
            const file = e.target.files?.[0];
            e.target.value = '';
            if (file) {
              onFile(file);
            }
          }}
        />
        <button type="button" className="b34 hov" onClick={() => input.current?.click()} disabled={busy}>
          {busy ? 'Загружаем…' : slot.version ? 'Заменить' : 'Загрузить'}
        </button>
        {slot.version ? (
          <button type="button" className="b34 hov bi-remove" onClick={onRemove} disabled={busy}>
            Удалить
          </button>
        ) : null}
      </span>
    </span>
  );
}
