import { useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import type { AdminDesign, AdminTheme, Appearance, ClientDesign, ClientTheme } from '../../lib/types';
import { useSession } from '../session';
import { SettingsLayout } from './SettingsLayout';

type PreviewTheme = 'dark' | 'light';

function previewThemes(choice: AdminTheme | ClientTheme): PreviewTheme[] {
  if (choice === 'dark' || choice === 'light') {
    return [choice];
  }
  return ['dark', 'light'];
}

function AdminPreview({ layout, theme }: { layout: AdminDesign; theme: PreviewTheme }) {
  const tone = layout === 'side' ? 'pa' : 'pb';
  return (
    <span className={`pv ${theme} ${tone}`} aria-hidden="true" style={{ flexDirection: layout === 'side' ? 'row' : 'column' }}>
      {layout === 'side' ? (
        <span className="pv-side">
          <i />
          <i />
          <i />
          <i />
          <i />
        </span>
      ) : (
        <span className="pv-top">
          <i />
          <i />
          <i />
          <i />
        </span>
      )}
      <span className="pv-body">
        <span className="pv-row">
          <span className="pv-title" />
          <span style={{ flex: '1 1 auto' }} />
          <span className="pv-btn" />
        </span>
        <span className="pv-cards">
          <i />
          <i />
        </span>
      </span>
    </span>
  );
}

function ClientPreview({ design, theme }: { design: ClientDesign; theme: PreviewTheme }) {
  const tone = design === 'accent' ? 'pa' : 'pb';
  return (
    <span className={`pv pv-client ${theme} ${tone}`} aria-hidden="true">
      <span className="pv-line" />
      <span className="pv-chips">
        <i />
        <i />
        <i />
      </span>
      <span className="pv-pay">Оплатить</span>
    </span>
  );
}

type PickProps = {
  name: string;
  desc: string;
  on: boolean;
  onPick: () => void;
  children: React.ReactNode;
};

function Pick({ name, desc, on, onPick, children }: PickProps) {
  return (
    <button type="button" className="pick hov" aria-pressed={on} onClick={() => (on ? undefined : onPick())}>
      <span className="pick-previews">{children}</span>
      <span className="pick-radio-row">
        <span className="pick-radio" aria-hidden="true" />
        <span className="pick-name">{name}</span>
      </span>
      <span className="muted">{desc}</span>
    </button>
  );
}

type SegsProps<T extends string> = {
  label: string;
  value: T;
  options: Array<[T, string]>;
  onPick: (value: T) => void;
};

function Segs<T extends string>({ label, value, options, onPick }: SegsProps<T>) {
  return (
    <div role="group" aria-label={label} className="segs">
      {options.map(([key, text]) => (
        <button key={key} type="button" aria-pressed={value === key} onClick={() => (value === key ? undefined : onPick(key))}>
          {text}
        </button>
      ))}
    </div>
  );
}

export function SettingsLook() {
  const { me, setAppearance } = useSession();
  const saved = me.appearance;
  const [work, setWork] = useState<Appearance | null>(null);
  const [note, setNote] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const current = work ?? saved;
  const dirty = work !== null && JSON.stringify(work) !== JSON.stringify(saved);

  useEffect(() => {
    if (work && JSON.stringify(work) === JSON.stringify(saved)) {
      setWork(null);
    }
  }, [saved, work]);

  const edit = (patch: Partial<Appearance>) => {
    setWork({ ...current, ...patch });
    setNote('');
    setError(null);
  };

  const save = async () => {
    if (!dirty || busy || !work) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await api<Appearance>('/api/admin/settings/appearance', { method: 'PUT', body: work });
      setAppearance(next);
      setWork(null);
      setNote('Сохранено');
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Не получилось сохранить');
    } finally {
      setBusy(false);
    }
  };

  const adminThemes = previewThemes(current.admin_theme);
  const clientThemes = previewThemes(current.client_theme);

  return (
    <SettingsLayout>
      <section aria-label="Админка" className="card">
        <div className="card-pad" style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div className="head-stack">
            <h2 className="section-title">Дизайн админки</h2>
            <span className="muted">У каждого дизайна есть тёмная и светлая тема</span>
          </div>
          <div role="group" aria-label="Дизайн админки" className="picks">
            <Pick name="Боковое меню" desc="Узкая полоса слева, кнопки цвета акцента" on={current.admin === 'side'} onPick={() => edit({ admin: 'side' })}>
              {adminThemes.map((theme) => (
                <AdminPreview key={theme} layout="side" theme={theme} />
              ))}
            </Pick>
            <Pick name="Верхнее меню" desc="Разделы в строке сверху, монохромные кнопки" on={current.admin === 'top'} onPick={() => edit({ admin: 'top' })}>
              {adminThemes.map((theme) => (
                <AdminPreview key={theme} layout="top" theme={theme} />
              ))}
            </Pick>
          </div>
        </div>
        <div className="split">
          <div className="head-stack">
            <h2 className="section-title">Тема админки</h2>
            <span className="muted">Кнопка с солнцем или луной в меню переключает только тему</span>
          </div>
          <Segs<AdminTheme>
            label="Тема админки"
            value={current.admin_theme}
            options={[
              ['dark', 'Тёмная'],
              ['light', 'Светлая'],
              ['system', 'Как в системе'],
            ]}
            onPick={(value) => edit({ admin_theme: value })}
          />
        </div>
      </section>

      <section aria-label="Кабинет и мини-приложение" className="card">
        <div className="card-pad" style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div className="head-stack">
            <h2 className="section-title">Дизайн кабинета и мини-приложения</h2>
            <span className="muted">Для лендинга, кабинета и мини-приложения. Раскладка одна, отличаются кнопки</span>
          </div>
          <div role="group" aria-label="Дизайн кабинета и мини-приложения" className="picks">
            <Pick name="Акцент" desc="Главные кнопки цвета акцента" on={current.client === 'accent'} onPick={() => edit({ client: 'accent' })}>
              {clientThemes.map((theme) => (
                <ClientPreview key={theme} design="accent" theme={theme} />
              ))}
            </Pick>
            <Pick name="Монохром" desc="Монохромные кнопки, акцент у выбранного" on={current.client === 'mono'} onPick={() => edit({ client: 'mono' })}>
              {clientThemes.map((theme) => (
                <ClientPreview key={theme} design="mono" theme={theme} />
              ))}
            </Pick>
          </div>
        </div>
        <div className="split">
          <div className="head-stack">
            <h2 className="section-title">Тема для клиентов</h2>
            <span className="muted">Если выбирает клиент, у него будет переключатель темы</span>
          </div>
          <Segs<ClientTheme>
            label="Тема для клиентов"
            value={current.client_theme}
            options={[
              ['dark', 'Тёмная'],
              ['light', 'Светлая'],
              ['client', 'Клиент выбирает сам'],
            ]}
            onPick={(value) => edit({ client_theme: value })}
          />
        </div>
      </section>

      <div className="save-row">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          Сохранить
        </button>
        <button
          type="button"
          className="btn-line hov"
          onClick={() => {
            setWork(null);
            setNote('');
            setError(null);
          }}
          disabled={busy}
        >
          Отменить
        </button>
        {error ? (
          <span role="status" className="bad">
            {error}
          </span>
        ) : (
          <span role="status">{dirty ? 'Изменения применятся после сохранения' : note}</span>
        )}
      </div>
    </SettingsLayout>
  );
}
