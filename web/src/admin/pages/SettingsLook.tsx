import { useEffect, useRef, useState } from 'react';
import { api, ApiError, upload } from '../../lib/api';
import { BrandMark } from '../../lib/brand';
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

type ImageInfo = { url: string; content_type: string; width: number; height: number; size: number };

type SiteImages = {
  title: string | null;
  brand_name: string;
  logo: ImageInfo | null;
  favicon: ImageInfo | null;
};

type Slot = {
  kind: 'logo' | 'favicon';
  title: string;
  sub: string;
  types: string[];
  max: number;
  accept: string;
  hint: string;
  tooHeavy: string;
  wrongType: string;
  failed: string;
  done: string;
  removed: string;
};

const LOGO_SLOT: Slot = {
  kind: 'logo',
  title: 'Логотип',
  sub: 'В меню админки, в кабинете, мини-приложении и на сайте',
  types: ['image/png', 'image/jpeg', 'image/webp'],
  max: 1024 * 1024,
  accept: 'image/png,image/jpeg,image/webp',
  hint: 'PNG, JPEG или WebP до 1\u00a0МБ. Лучше квадрат от 256×256 с прозрачным фоном. Меняется сразу, без кнопки «Сохранить»',
  tooHeavy: 'Файл больше 1\u00a0МБ: уменьшите картинку',
  wrongType: 'Подходят только PNG, JPEG и WebP',
  failed: 'Логотип не загрузился',
  done: 'Логотип обновлён',
  removed: 'Логотип убран, вместо него снова первая буква названия',
};

const FAVICON_SLOT: Slot = {
  kind: 'favicon',
  title: 'Значок во вкладке браузера',
  sub: 'Фавикон: виден во вкладке, в закладках и на экране телефона, если сайт добавят туда',
  types: ['image/png', 'image/x-icon', 'image/vnd.microsoft.icon'],
  max: 512 * 1024,
  accept: 'image/png,image/x-icon,image/vnd.microsoft.icon,.ico',
  hint: 'PNG или ICO, квадрат от 16×16 до 1024×1024, до 512\u00a0КБ. Лучше PNG 512×512: он же пойдёт на экран телефона. Меняется сразу',
  tooHeavy: 'Файл больше 512\u00a0КБ: уменьшите значок',
  wrongType: 'Подходят только PNG и ICO',
  failed: 'Значок не загрузился',
  done: 'Значок обновлён',
  removed: 'Свой значок убран',
};

function weight(size: number): string {
  if (size >= 1024 * 1024) {
    return `${(size / 1024 / 1024).toFixed(1).replace('.', ',')}\u00a0МБ`;
  }
  return `${Math.max(1, Math.round(size / 1024))}\u00a0КБ`;
}

function fileType(file: File): string {
  if (file.type) {
    return file.type;
  }
  const name = file.name.toLowerCase();
  if (name.endsWith('.ico')) {
    return 'image/x-icon';
  }
  if (name.endsWith('.png')) {
    return 'image/png';
  }
  return '';
}

function tabIcon(images: SiteImages | null | undefined): { url: string; type: string } {
  if (images?.favicon) {
    return { url: images.favicon.url, type: images.favicon.content_type };
  }
  if (images?.logo) {
    return { url: images.logo.url, type: images.logo.content_type };
  }
  return { url: '/api/public/icon.svg', type: 'image/svg+xml' };
}

function applyTabIcon(icon: { url: string; type: string }) {
  let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (!link) {
    link = document.createElement('link');
    link.rel = 'icon';
    document.head.appendChild(link);
  }
  link.type = icon.type;
  link.href = icon.url;
}

type CardProps = {
  slot: Slot;
  info: ImageInfo | null | undefined;
  current: string;
  preview: React.ReactNode;
  onChange: (next: ImageInfo | null) => Promise<void>;
};

function ImageCard({ slot, info, current, preview, onChange }: CardProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState('');
  const input = useRef<HTMLInputElement>(null);
  const id = `${slot.kind}-h`;

  const send = async (file: File | undefined) => {
    if (!file) {
      return;
    }
    setNote('');
    const type = fileType(file);
    if (!slot.types.includes(type)) {
      setError(slot.wrongType);
      return;
    }
    if (file.size > slot.max) {
      setError(slot.tooHeavy);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await upload<ImageInfo>(`/api/admin/settings/${slot.kind}`, new Blob([file], { type }));
      await onChange(next);
      setNote(slot.done);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : slot.failed);
    } finally {
      setBusy(false);
      if (input.current) {
        input.current.value = '';
      }
    }
  };

  const remove = async () => {
    setBusy(true);
    setError(null);
    setNote('');
    try {
      await api(`/api/admin/settings/${slot.kind}`, { method: 'DELETE' });
      await onChange(null);
      setNote(slot.removed);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Не получилось убрать');
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby={id} className="card">
      <div className="card-pad lg">
        <div className="head-stack">
          <h2 id={id} className="section-title">
            {slot.title}
          </h2>
          <span className="muted">{slot.sub}</span>
        </div>
        <div className="lg-row">
          <span className="lg-previews" aria-hidden="true">
            {preview}
          </span>
          <span className="lg-side">
            <span className="lg-btns">
              <button type="button" className="btn-line hov" onClick={() => input.current?.click()} disabled={busy || info === undefined}>
                {busy ? 'Загружаем…' : info ? 'Заменить' : 'Загрузить'}
              </button>
              {info ? (
                <button type="button" className="btn-line hov" onClick={() => void remove()} disabled={busy}>
                  Убрать
                </button>
              ) : null}
            </span>
            <span className="cap">{current}</span>
            <span className="cap">{slot.hint}</span>
            {error ? (
              <span role="alert" className="err">
                {error}
              </span>
            ) : (
              <span role="status" className="lg-note">
                {note}
              </span>
            )}
          </span>
        </div>
        <input ref={input} type="file" accept={slot.accept} hidden onChange={(e) => void send(e.target.files?.[0])} />
      </div>
    </section>
  );
}

function TabPreview({ theme, icon, title }: { theme: PreviewTheme; icon: string; title: string }) {
  return (
    <span className={`fv-pv ${theme}`}>
      <span className="fv-tab">
        <img src={icon} alt="" width={16} height={16} draggable={false} />
        <span className="fv-title">{title}</span>
        <span className="fv-x">×</span>
      </span>
    </span>
  );
}

function BrandCards() {
  const { me, reload } = useSession();
  const [images, setImages] = useState<SiteImages | null | undefined>(undefined);

  useEffect(() => {
    api<SiteImages>('/api/admin/settings/site')
      .then(setImages)
      .catch(() => setImages(null));
  }, []);

  const change = async (patch: Partial<SiteImages>) => {
    const next = { ...(images ?? { title: null, brand_name: me.brand_name, logo: null, favicon: null }), ...patch };
    setImages(next);
    applyTabIcon(tabIcon(next));
    await reload();
  };

  const logo = images?.logo ?? null;
  const favicon = images?.favicon ?? null;
  const name = me.brand_name.toUpperCase();
  const tab = tabIcon(images);
  const tabTitle = images?.title || me.brand_name;
  const loading = images === undefined ? undefined : null;
  return (
    <>
      <ImageCard
        slot={LOGO_SLOT}
        info={logo ?? loading}
        current={logo ? `Сейчас: ${logo.width}×${logo.height}, ${weight(logo.size)}` : 'Сейчас вместо логотипа первая буква названия'}
        onChange={(next) => change({ logo: next })}
        preview={
          <>
            <span className="lg-pv dark">
              <BrandMark name={me.brand_name} logo={logo?.url} className="mark" />
              <span>{name}</span>
            </span>
            <span className="lg-pv light">
              <BrandMark name={me.brand_name} logo={logo?.url} className="mark" />
              <span>{name}</span>
            </span>
          </>
        }
      />
      <ImageCard
        slot={FAVICON_SLOT}
        info={favicon ?? loading}
        current={
          favicon
            ? `Сейчас: свой значок ${favicon.width}×${favicon.height}, ${favicon.content_type === 'image/png' ? 'PNG' : 'ICO'}, ${weight(favicon.size)}`
            : logo
              ? 'Сейчас своего значка нет — во вкладке логотип'
              : 'Сейчас своего значка нет — во вкладке первая буква названия'
        }
        onChange={(next) => change({ favicon: next })}
        preview={
          <>
            <TabPreview theme="dark" icon={tab.url} title={tabTitle} />
            <TabPreview theme="light" icon={tab.url} title={tabTitle} />
          </>
        }
      />
    </>
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
      <BrandCards />
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
