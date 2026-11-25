import { Fragment, useEffect, useState } from 'react';
import { api, ApiError, upload } from '../../lib/api';
import { forbiddenScheme } from '../../lib/appScheme';
import { count } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { Switch } from '../ui';
import './connect-apps.css';

export type Platform = 'ios' | 'android' | 'windows' | 'macos' | 'linux' | 'android_tv' | 'apple_tv';
export type Words = { ru: string; en: string };
export type LinkButton = { label: Words; url: string };
export type Step =
  | { kind: 'install'; text: Words; buttons: LinkButton[] }
  | { kind: 'add'; text: Words; link: string | null }
  | { kind: 'connect'; text: Words }
  | { kind: 'note'; title: Words; text: Words; buttons: LinkButton[] };
export type IconRef = { tag: string; kind: 'png' | 'jpeg' | 'webp' | 'svg' };
export type ConnectApp = { id: string; name: string; icon: IconRef | null; enabled: boolean; steps: Step[] };
export type Apps = Record<Platform, ConnectApp[]>;
export type Limits = {
  apps: number;
  name: number;
  label: number;
  title: number;
  text: number;
  link: number;
  buttons: number;
  notes: number;
  icon_bytes: number;
  svg_bytes: number;
  icon_min_side: number;
};
export type AppsView = { apps: Apps; standard: boolean; defaults: Apps; limits: Limits };
type PageConfig = { uuid: string; name: string };
type Preview = { name: string; apps: Apps; platforms: Array<{ platform: Platform; title: string; apps: string[] }>; total: number; notes: string[] };
type Lang = 'ru' | 'en';

export const PLATFORMS: Array<[Platform, string]> = [
  ['ios', 'iPhone и iPad'],
  ['android', 'Android'],
  ['windows', 'Windows'],
  ['macos', 'macOS'],
  ['linux', 'Linux'],
  ['android_tv', 'Android TV'],
  ['apple_tv', 'Apple TV'],
];

const TITLE = Object.fromEntries(PLATFORMS) as Record<Platform, string>;

const BASE_NAME: Record<'install' | 'add' | 'connect', string> = {
  install: 'Установка',
  add: 'Добавление',
  connect: 'Включение',
};

export function fullApps(apps: Partial<Apps> | undefined): Apps {
  const out = {} as Apps;
  for (const [platform] of PLATFORMS) {
    out[platform] = apps?.[platform] ?? [];
  }
  return out;
}

export function sameApps(a: Apps, b: Apps): boolean {
  return JSON.stringify(fullApps(a)) === JSON.stringify(fullApps(b));
}

function iconUrl(icon: IconRef): string {
  return `/api/public/app-icon/${icon.tag}.${icon.kind === 'jpeg' ? 'jpg' : icon.kind}`;
}

export function httpsProblem(raw: string): string | null {
  const link = raw.trim();
  if (!link) {
    return 'Нужна ссылка';
  }
  if (/[\s"'<>\\`{}]/.test(link)) {
    return 'В ссылке есть пробелы, кавычки или скобки';
  }
  const found = /^https:\/\/([^/?#]*)/i.exec(link);
  const host = found?.[1]?.split('@').pop() ?? '';
  if (!host || !/^[A-Za-z0-9.\-:[\]_]+$/.test(host)) {
    return 'Нужна ссылка https://';
  }
  return null;
}

export function appLinkProblem(raw: string, needUrl: boolean): string | null {
  const link = raw.trim();
  if (!link) {
    return 'Нужна ссылка';
  }
  const probe = link.split('{url}').join('u').split('{name}').join('n');
  if (/[{}]/.test(probe)) {
    return 'Подставить можно только {url} и {name}';
  }
  if (/[\s"'<>\\`]/.test(probe)) {
    return 'В ссылке есть пробелы, кавычки или скобки';
  }
  const colon = probe.indexOf(':');
  const scheme = colon > 0 ? probe.slice(0, colon).toLowerCase() : '';
  if (!/^[a-z][a-z0-9+.-]{1,31}$/.test(scheme) || colon === probe.length - 1) {
    return 'Нужна схема приложения вроде happ:// или ссылка https://';
  }
  if (forbiddenScheme(scheme)) {
    return 'Такие ссылки запрещены: нужна схема приложения или https://';
  }
  if (scheme === 'https') {
    const problem = httpsProblem(probe);
    if (problem) {
      return problem;
    }
  }
  if (needUrl && !link.includes('{url}')) {
    return 'Нет подстановки {url}: без неё приложение не получит подписку';
  }
  return null;
}

function buttonProblem(button: LinkButton, install: boolean): string | null {
  if (!button.label.ru.trim() && !button.label.en.trim()) {
    return 'Напишите надпись кнопки';
  }
  return install ? httpsProblem(button.url) : appLinkProblem(button.url, false);
}

export function appProblem(app: ConnectApp): string | null {
  if (!app.name.trim()) {
    return 'Напишите название';
  }
  for (const step of app.steps) {
    if (step.kind === 'install' || step.kind === 'note') {
      if (step.kind === 'note' && !step.title.ru.trim() && !step.title.en.trim()) {
        return 'У дополнительного шага нет заголовка';
      }
      for (const button of step.buttons) {
        const problem = buttonProblem(button, step.kind === 'install');
        if (problem) {
          return `Кнопка «${button.label.ru || button.label.en || 'без надписи'}»: ${problem.toLowerCase()}`;
        }
      }
    }
    if (step.kind === 'add' && step.link && step.link.trim()) {
      const problem = appLinkProblem(step.link, true);
      if (problem) {
        return `Ссылка добавления: ${problem.toLowerCase()}`;
      }
    }
  }
  return null;
}

export function appsProblem(apps: Apps): string | null {
  for (const [platform, title] of PLATFORMS) {
    for (const app of apps[platform] ?? []) {
      const problem = appProblem(app);
      if (problem) {
        return `${title} · ${app.name.trim() || 'приложение без названия'}: ${problem}`;
      }
    }
  }
  return null;
}

function emptyApp(): ConnectApp {
  const words = { ru: '', en: '' };
  return {
    id: '',
    name: '',
    icon: null,
    enabled: true,
    steps: [
      { kind: 'install', text: words, buttons: [{ label: { ru: 'Скачать', en: 'Download' }, url: '' }] },
      { kind: 'add', text: words, link: '' },
      { kind: 'connect', text: words },
    ],
  };
}

function summary(app: ConnectApp): string {
  const install = app.steps.find((step) => step.kind === 'install');
  const add = app.steps.find((step) => step.kind === 'add');
  const labels = install && install.kind === 'install' ? install.buttons.map((button) => button.label.ru || button.label.en).filter(Boolean) : [];
  const link = add && add.kind === 'add' ? (add.link ?? '').trim() : '';
  return [labels.length ? labels.join(', ') : 'Нет кнопок установки', link || 'нет ссылки добавления'].join(' · ');
}

function merge(current: Apps, incoming: Apps, limit: number): Apps {
  const out = fullApps(current);
  for (const [platform] of PLATFORMS) {
    const list = [...out[platform]];
    for (const app of incoming[platform] ?? []) {
      const at = list.findIndex((item) => item.name.trim().toLowerCase() === app.name.trim().toLowerCase());
      if (at >= 0) {
        list[at] = app;
      } else if (list.length < limit) {
        list.push(app);
      }
    }
    out[platform] = list;
  }
  return out;
}

export function AppTile({ app, small }: { app: ConnectApp; small?: boolean }) {
  return (
    <span className={small ? 'ca-ico sm' : 'ca-ico'} aria-hidden="true">
      {app.icon ? <img src={iconUrl(app.icon)} alt="" decoding="async" /> : (app.name.trim().slice(0, 1) || '?').toUpperCase()}
    </span>
  );
}

type WordsField = { value: Words; lang: Lang; onChange: (value: Words) => void; max: number; label: string; multiline?: boolean; placeholder?: string; invalid?: boolean };

function WordsInput({ value, lang, onChange, max, label, multiline, placeholder, invalid }: WordsField) {
  const text = value[lang];
  const set = (next: string) => onChange({ ...value, [lang]: next });
  const hint = lang === 'en' && !text && value.ru ? value.ru : placeholder;
  return multiline ? (
    <textarea className="field" rows={Math.min(6, Math.max(2, Math.ceil(text.length / 70) + (text.match(/\n/g)?.length ?? 0)))} maxLength={max} value={text} placeholder={hint} aria-label={label} aria-invalid={invalid || undefined} onChange={(e) => set(e.target.value)} />
  ) : (
    <input className="field" maxLength={max} value={text} placeholder={hint} aria-label={label} aria-invalid={invalid || undefined} onChange={(e) => set(e.target.value)} />
  );
}

type ButtonsProps = { buttons: LinkButton[]; lang: Lang; limits: Limits; install: boolean; onChange: (buttons: LinkButton[]) => void };

function ButtonsEditor({ buttons, lang, limits, install, onChange }: ButtonsProps) {
  return (
    <div className="ca-col">
      {buttons.map((button, index) => {
        const problem = button.url.trim() ? (install ? httpsProblem(button.url) : appLinkProblem(button.url, false)) : null;
        return (
          <Fragment key={index}>
            <div className="ca-btnrow">
              <WordsInput value={button.label} lang={lang} max={limits.label} label="Надпись кнопки" placeholder="Надпись" onChange={(label) => onChange(buttons.map((item, i) => (i === index ? { ...item, label } : item)))} />
              <input
                className="field mono"
                value={button.url}
                maxLength={limits.link}
                placeholder={install ? 'https://' : 'https:// или схема приложения'}
                aria-label="Ссылка кнопки"
                aria-invalid={!!problem || undefined}
                spellCheck={false}
                onChange={(e) => onChange(buttons.map((item, i) => (i === index ? { ...item, url: e.target.value } : item)))}
              />
              <button type="button" className="ca-x" aria-label={`Убрать кнопку ${button.label.ru || ''}`.trim()} onClick={() => onChange(buttons.filter((_, i) => i !== index))}>
                <Icon name="close" size={18} />
              </button>
            </div>
            {problem ? <span className="err">{problem}</span> : null}
          </Fragment>
        );
      })}
      {buttons.length < limits.buttons ? (
        <button type="button" className="ca-more" onClick={() => onChange([...buttons, { label: { ru: '', en: '' }, url: '' }])}>
          Добавить кнопку
        </button>
      ) : null}
    </div>
  );
}

type PanelProps = {
  app: ConnectApp;
  platform: Platform;
  main: boolean;
  lang: Lang;
  limits: Limits;
  onLang: (lang: Lang) => void;
  onChange: (app: ConnectApp) => void;
  onMain: () => void;
  onDelete: () => void;
  onCopy: (targets: Platform[]) => void;
};

function AppPanel({ app, platform, main, lang, limits, onLang, onChange, onMain, onDelete, onCopy }: PanelProps) {
  const [ask, setAsk] = useState<'delete' | 'copy' | null>(null);
  const [targets, setTargets] = useState<Platform[]>([]);
  const [iconBusy, setIconBusy] = useState(false);
  const [iconError, setIconError] = useState<string | null>(null);
  const setStep = (index: number, step: Step) => onChange({ ...app, steps: app.steps.map((item, i) => (i === index ? step : item)) });
  const notes = app.steps.filter((step) => step.kind === 'note').length;
  const addStep = app.steps.find((step) => step.kind === 'add');
  const addLink = addStep && addStep.kind === 'add' ? (addStep.link ?? '') : '';
  const linkProblem = addLink.trim() ? appLinkProblem(addLink, true) : null;

  const pickIcon = async (file: File | undefined) => {
    if (!file) {
      return;
    }
    setIconBusy(true);
    setIconError(null);
    try {
      const saved = await upload<{ tag: string; kind: IconRef['kind'] }>('/api/admin/settings/connect-apps/icons', new Blob([file], { type: file.type }));
      onChange({ ...app, icon: { tag: saved.tag, kind: saved.kind } });
    } catch (err) {
      setIconError(err instanceof ApiError ? err.message : 'Значок не загрузился');
    } finally {
      setIconBusy(false);
    }
  };

  const move = (index: number, shift: number) => {
    const next = [...app.steps];
    const [step] = next.splice(index, 1);
    if (!step) {
      return;
    }
    next.splice(index + shift, 0, step);
    onChange({ ...app, steps: next });
  };

  let number = 0;
  return (
    <li className="ca-panel">
      <div className="ca-grid">
        <span className="ca-lbl">Название и значок</span>
        <div className="ca-col">
          <div className="ca-inline">
            <input className="field ca-name-field" value={app.name} maxLength={limits.name} placeholder="Например, Happ" aria-label="Название приложения" aria-invalid={!app.name.trim() || undefined} onChange={(e) => onChange({ ...app, name: e.target.value })} />
            <AppTile app={app} />
            <label className={iconBusy ? 'b2 hov ca-file off' : 'b2 hov ca-file'}>
              {app.icon ? 'Заменить значок' : 'Загрузить значок'}
              <input
                type="file"
                accept="image/png,image/jpeg,image/webp,image/svg+xml"
                disabled={iconBusy}
                onChange={(e) => {
                  void pickIcon(e.target.files?.[0]);
                  e.target.value = '';
                }}
              />
            </label>
            {app.icon ? (
              <button type="button" className="ca-plain" onClick={() => onChange({ ...app, icon: null })}>
                Убрать
              </button>
            ) : null}
          </div>
          {iconError ? <span className="err">{iconError}</span> : <span className="cap">Квадратный PNG, JPEG, WebP до {Math.round(limits.icon_bytes / 1024)} КБ или SVG до {Math.round(limits.svg_bytes / 1024)} КБ. Без значка клиент увидит первую букву</span>}
        </div>

        <span className="ca-lbl">Для клиента</span>
        <div className="ca-inline ca-flags">
          <span className="ca-flag">
            <Switch on={app.enabled} label="Показывать клиенту" onToggle={() => onChange({ ...app, enabled: !app.enabled })} />
            Показывать
          </span>
          {main ? (
            <span className="cap">Главное на {TITLE[platform]}: клиент увидит его первым</span>
          ) : (
            <button type="button" className="b2 hov" onClick={onMain} disabled={!app.enabled}>
              Сделать главным
            </button>
          )}
        </div>

        <span className="ca-lbl">Язык текстов</span>
        <div className="ca-col">
          <div role="group" aria-label="Язык текстов" className="segs">
            {(
              [
                ['ru', 'Русский'],
                ['en', 'English'],
              ] as Array<[Lang, string]>
            ).map(([code, label]) => (
              <button key={code} type="button" aria-pressed={lang === code} onClick={() => onLang(code)}>
                {label}
              </button>
            ))}
          </div>
          <span className="cap">Клиент видит тексты на своём языке. Где нет английского текста, покажем русский</span>
        </div>

        {app.steps.map((step, index) => {
          number += 1;
          if (step.kind === 'install') {
            return (
              <Fragment key={index}>
                <span className="ca-lbl">Кнопки установки</span>
                <ButtonsEditor buttons={step.buttons} lang={lang} limits={limits} install onChange={(buttons) => setStep(index, { ...step, buttons })} />
              </Fragment>
            );
          }
          if (step.kind === 'add') {
            return (
              <Fragment key={index}>
                <span className="ca-lbl">Ссылка добавления</span>
                <div className="ca-col">
                  <input className="field mono" value={step.link ?? ''} maxLength={limits.link} placeholder="happ://add/{url}" aria-label="Ссылка добавления подписки" aria-invalid={!!linkProblem || undefined} spellCheck={false} onChange={(e) => setStep(index, { ...step, link: e.target.value })} />
                  {linkProblem ? <span className="err">{linkProblem}</span> : null}
                  <span className="cap">Вместо {'{url}'} подставится ссылка подписки клиента, вместо {'{name}'} — название сервиса. Подойдёт схема приложения или https://. Без ссылки клиент сможет только скопировать подписку</span>
                </div>
              </Fragment>
            );
          }
          return null;
        })}

        <span className="ca-lbl">Шаги</span>
        <div className="ca-col">
          <div className="ca-steps">
            {app.steps.map((step, index) => {
              const shown = index + 1;
              if (step.kind !== 'note') {
                return (
                  <div key={index} className="ca-step">
                    <span className="ca-num">{shown}</span>
                    <span className="ca-step-name">{BASE_NAME[step.kind]}</span>
                    <WordsInput value={step.text} lang={lang} max={limits.text} label={`Текст шага «${BASE_NAME[step.kind]}»`} multiline placeholder="Подсказка под шагом, можно оставить пустой" onChange={(text) => setStep(index, { ...step, text })} />
                  </div>
                );
              }
              const titleMissing = !step.title.ru.trim() && !step.title.en.trim();
              return (
                <div key={index} className="ca-step extra">
                  <span className="ca-num">{shown}</span>
                  <div className="ca-col">
                    <div className="ca-inline ca-note-head">
                      <WordsInput value={step.title} lang={lang} max={limits.title} label="Заголовок шага" placeholder="Например, «Если подписка не добавилась»" invalid={titleMissing} onChange={(title) => setStep(index, { ...step, title })} />
                      <button type="button" className="pay-arrow" aria-label="Шаг выше" aria-disabled={index === 0} onClick={() => index > 0 && move(index, -1)}>
                        <Icon name="arrow-up" size={18} />
                      </button>
                      <button type="button" className="pay-arrow" aria-label="Шаг ниже" aria-disabled={index === app.steps.length - 1} onClick={() => index < app.steps.length - 1 && move(index, 1)}>
                        <Icon name="arrow-down" size={18} />
                      </button>
                      <button type="button" className="ca-x" aria-label="Убрать шаг" onClick={() => onChange({ ...app, steps: app.steps.filter((_, i) => i !== index) })}>
                        <Icon name="close" size={18} />
                      </button>
                    </div>
                    {titleMissing ? <span className="err">Напишите заголовок шага</span> : null}
                    <WordsInput value={step.text} lang={lang} max={limits.text} label="Текст шага" multiline onChange={(text) => setStep(index, { ...step, text })} />
                    <ButtonsEditor buttons={step.buttons} lang={lang} limits={limits} install={false} onChange={(buttons) => setStep(index, { ...step, buttons })} />
                  </div>
                </div>
              );
            })}
          </div>
          {notes < limits.notes ? (
            <button
              type="button"
              className="ca-more"
              onClick={() => {
                const at = app.steps.findIndex((step) => step.kind === 'connect');
                const next = [...app.steps];
                next.splice(at < 0 ? next.length : at, 0, { kind: 'note', title: { ru: '', en: '' }, text: { ru: '', en: '' }, buttons: [] });
                onChange({ ...app, steps: next });
              }}
            >
              Добавить шаг
            </button>
          ) : null}
        </div>
      </div>

      {ask === 'delete' ? (
        <div className="ca-ask" role="group" aria-label="Удаление приложения">
          <span>
            Удалить «{app.name.trim() || 'приложение без названия'}» с платформы {TITLE[platform]}?
          </span>
          <span className="ca-inline">
            <button type="button" className="btn-bad ca-small" onClick={onDelete}>
              Удалить
            </button>
            <button type="button" className="b2 hov" onClick={() => setAsk(null)}>
              Отменить
            </button>
          </span>
        </div>
      ) : ask === 'copy' ? (
        <div className="ca-ask" role="group" aria-label="Копирование на другие платформы">
          <span>На какие платформы скопировать? Приложение с тем же названием там заменится</span>
          <span className="ca-chk">
            {PLATFORMS.filter(([key]) => key !== platform).map(([key, title]) => (
              <button key={key} type="button" aria-pressed={targets.includes(key)} onClick={() => setTargets(targets.includes(key) ? targets.filter((item) => item !== key) : [...targets, key])}>
                {title}
              </button>
            ))}
          </span>
          <span className="ca-inline">
            <button
              type="button"
              className="btn-main btn ca-small"
              disabled={!targets.length}
              onClick={() => {
                onCopy(targets);
                setTargets([]);
                setAsk(null);
              }}
            >
              Скопировать
            </button>
            <button type="button" className="b2 hov" onClick={() => setAsk(null)}>
              Отменить
            </button>
          </span>
        </div>
      ) : (
        <div className="ca-foot">
          <button type="button" className="b2 hov" onClick={() => setAsk('copy')}>
            Скопировать на другие платформы
          </button>
          <span className="grow" />
          <button type="button" className="ca-bad" onClick={() => setAsk('delete')}>
            Удалить приложение
          </button>
        </div>
      )}
    </li>
  );
}

type ImportProps = { configs: PageConfig[]; initial: string; current: number; onClose: () => void; onApply: (apps: Apps, mode: 'replace' | 'add') => void };

function ImportDialog({ configs, initial, current, onClose, onApply }: ImportProps) {
  const [config, setConfig] = useState(initial);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<'replace' | 'add'>('replace');

  useEffect(() => {
    let alive = true;
    setBusy(true);
    setError(null);
    setPreview(null);
    api<Preview>('/api/admin/settings/connect-apps/import', { method: 'POST', body: { page_config: config } })
      .then((loaded) => alive && setPreview(loaded))
      .catch((err) => alive && setError(err instanceof ApiError ? err.message : 'Оформление не прочиталось'))
      .finally(() => alive && setBusy(false));
    return () => {
      alive = false;
    };
  }, [config]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div className="dialog-back" role="presentation" onClick={onClose}>
      <div className="dialog ca-dialog" role="dialog" aria-modal="true" aria-labelledby="ca-imp-h" onClick={(event) => event.stopPropagation()}>
        <h2 id="ca-imp-h">Взять приложения из Remnawave</h2>
        <label className="sf-field ca-wide">
          <span className="sf-label">Оформление страницы подписки</span>
          <select className="field" value={config} onChange={(e) => setConfig(e.target.value)}>
            {configs.map((item) => (
              <option key={item.uuid} value={item.uuid}>
                {item.name}
              </option>
            ))}
          </select>
        </label>
        {busy ? <p className="muted">Читаем оформление…</p> : null}
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        {preview ? (
          <>
            <span className="ca-lead">
              Перенесём {count(preview.total, 'приложение', 'приложения', 'приложений')} на {count(preview.platforms.length, 'платформе', 'платформах', 'платформах')}:
            </span>
            <dl className="ca-imp">
              {preview.platforms.map((item) => (
                <Fragment key={item.platform}>
                  <dt>{item.title}</dt>
                  <dd>
                    <b>{item.apps[0]}</b> главное{item.apps.length > 1 ? ` · ${item.apps.slice(1).join(', ')}` : ''}
                  </dd>
                </Fragment>
              ))}
            </dl>
            {preview.notes.length ? (
              <ul className="ca-notes">
                {preview.notes.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            ) : null}
            <div className="ca-modes" role="radiogroup" aria-label="Как перенести">
              <button type="button" className="ca-mode" role="radio" aria-checked={mode === 'replace'} onClick={() => setMode('replace')}>
                <span className="ca-radio" />
                <span className="ca-mode-text">
                  <strong>Заменить список</strong>
                  <span>{current ? `Нынешние ${count(current, 'приложение', 'приложения', 'приложений')} уберём` : 'Сейчас список пуст'}</span>
                </span>
              </button>
              <button type="button" className="ca-mode" role="radio" aria-checked={mode === 'add'} onClick={() => setMode('add')}>
                <span className="ca-radio" />
                <span className="ca-mode-text">
                  <strong>Добавить к списку</strong>
                  <span>Приложения с тем же названием обновим, новые добавим в конец</span>
                </span>
              </button>
            </div>
          </>
        ) : null}
        <div className="dialog-actions">
          <button type="button" className="btn-main btn" disabled={!preview || busy} onClick={() => preview && onApply(preview.apps, mode)}>
            Перенести
          </button>
          <button type="button" className="btn-line hov" onClick={onClose}>
            Отменить
          </button>
        </div>
        <span className="cap">Список изменится в форме. Клиенты увидят его после «Сохранить»</span>
      </div>
    </div>
  );
}

type EditorProps = {
  apps: Apps;
  defaults: Apps;
  limits: Limits;
  configs: PageConfig[];
  canImport: boolean;
  look: string;
  version: number;
  onChange: (apps: Apps) => void;
};

export function ConnectAppsEditor({ apps, defaults, limits, configs, canImport, look, version, onChange }: EditorProps) {
  const [platform, setPlatform] = useState<Platform>('ios');
  const [open, setOpen] = useState<number | null>(null);
  const [lang, setLang] = useState<Lang>('ru');
  const [importing, setImporting] = useState(false);
  const [note, setNote] = useState('');
  useEffect(() => {
    setNote('');
  }, [version]);
  const list = apps[platform] ?? [];
  const mainIndex = list.findIndex((app) => app.enabled);
  const total = PLATFORMS.reduce((sum, [key]) => sum + (apps[key]?.length ?? 0), 0);

  const setList = (next: ConnectApp[]) => {
    onChange({ ...fullApps(apps), [platform]: next });
    setNote('');
  };
  const moveApp = (index: number, shift: number) => {
    const target = index + shift;
    if (target < 0 || target >= list.length) {
      return;
    }
    const next = [...list];
    const [app] = next.splice(index, 1);
    if (!app) {
      return;
    }
    next.splice(target, 0, app);
    setList(next);
    setOpen(open === index ? target : open === target ? index : open);
  };

  return (
    <>
      <div className="ca-top">
        <div className="head-stack">
          <h3 className="ca-h3">Приложения</h3>
          <span className="cap">Клиент увидит главное приложение первым, остальные сможет выбрать. Платформу без приложений не покажем</span>
        </div>
        <button
          type="button"
          className="b2 hov"
          disabled={!canImport || !configs.length}
          title={!canImport ? 'Нужна связь с панелью Remnawave' : !configs.length ? 'В Remnawave нет оформлений страницы подписки' : undefined}
          onClick={() => setImporting(true)}
        >
          Взять из Remnawave
        </button>
        <button
          type="button"
          className="b2 hov"
          disabled={sameApps(apps, defaults)}
          onClick={() => {
            onChange(fullApps(defaults));
            setOpen(null);
            setNote('Вернули набор по умолчанию: Happ на телефонах, Hiddify на компьютерах. Сохраните, чтобы клиенты его увидели');
          }}
        >
          Вернуть как было
        </button>
      </div>
      {note ? (
        <p role="status" className="ca-status">
          {note}
        </p>
      ) : null}
      <div className="ca-plats">
        <div role="group" aria-label="Платформа" className="segs">
          {PLATFORMS.map(([key, title]) => {
            const on = (apps[key] ?? []).filter((app) => app.enabled).length;
            return (
              <button
                key={key}
                type="button"
                aria-pressed={platform === key}
                onClick={() => {
                  setPlatform(key);
                  setOpen(null);
                }}
              >
                {title}
                <span className="ca-n">{on || 'нет'}</span>
              </button>
            );
          })}
        </div>
      </div>
      <ul className="ca-list" aria-label={`Приложения: ${TITLE[platform]}`}>
        {list.length === 0 ? <li className="pay-empty muted">На {TITLE[platform]} приложений нет — клиент не увидит эту платформу</li> : null}
        {list.map((app, index) => {
          const problem = appProblem(app);
          const expanded = open === index;
          const tag = problem ? (
            <span className="ca-tag bad">Есть ошибка</span>
          ) : index === mainIndex ? (
            <span className="ca-tag">Главное</span>
          ) : !app.enabled ? (
            <span className="ca-tag off">Выключено</span>
          ) : null;
          return (
            <Fragment key={index}>
              <li className={app.enabled ? 'ca-row' : 'ca-row off'}>
                <AppTile app={app} />
                <span className="ca-name">
                  <strong>{app.name.trim() || 'Без названия'}</strong>
                  <span className="ca-sub">{summary(app)}</span>
                  {tag ? <span className="ca-narrow">{tag}</span> : null}
                </span>
                {tag ? <span className="ca-wide-tag">{tag}</span> : null}
                <button type="button" className="pay-setup ca-setup" aria-expanded={expanded} aria-label={`${expanded ? 'Свернуть' : 'Настроить'}: ${app.name.trim() || 'без названия'}`} onClick={() => setOpen(expanded ? null : index)}>
                  <span className="ca-setup-text">{expanded ? 'Свернуть' : 'Настроить'}</span>
                  <Icon name="chevron-down" size={18} />
                </button>
                <button type="button" className="pay-arrow" aria-label={`Выше: ${app.name}`} aria-disabled={index === 0} onClick={() => moveApp(index, -1)}>
                  <Icon name="arrow-up" size={18} />
                </button>
                <button type="button" className="pay-arrow" aria-label={`Ниже: ${app.name}`} aria-disabled={index === list.length - 1} onClick={() => moveApp(index, 1)}>
                  <Icon name="arrow-down" size={18} />
                </button>
              </li>
              {expanded ? (
                <AppPanel
                  key={`panel-${platform}-${index}`}
                  app={app}
                  platform={platform}
                  main={index === mainIndex}
                  lang={lang}
                  limits={limits}
                  onLang={setLang}
                  onChange={(next) => setList(list.map((item, i) => (i === index ? next : item)))}
                  onMain={() => {
                    const next = [...list];
                    const [picked] = next.splice(index, 1);
                    if (!picked) {
                      return;
                    }
                    next.unshift(picked);
                    setList(next);
                    setOpen(0);
                  }}
                  onDelete={() => {
                    setList(list.filter((_, i) => i !== index));
                    setOpen(null);
                  }}
                  onCopy={(targets) => {
                    const next = fullApps(apps);
                    const copied: string[] = [];
                    for (const target of targets) {
                      const items = [...next[target]];
                      const at = items.findIndex((item) => item.name.trim().toLowerCase() === app.name.trim().toLowerCase());
                      if (at >= 0) {
                        items[at] = app;
                      } else if (items.length < limits.apps) {
                        items.push(app);
                      } else {
                        continue;
                      }
                      next[target] = items;
                      copied.push(TITLE[target]);
                    }
                    onChange(next);
                    setNote(copied.length ? `Скопировали на ${copied.join(', ')}` : `На выбранных платформах уже по ${limits.apps} приложений`);
                  }}
                />
              ) : null}
            </Fragment>
          );
        })}
      </ul>
      <div className="ca-addrow">
        <button
          type="button"
          className="b2 hov"
          disabled={list.length >= limits.apps}
          onClick={() => {
            setList([...list, emptyApp()]);
            setOpen(list.length);
          }}
        >
          Добавить приложение
        </button>
        {list.length >= limits.apps ? <span className="cap">На платформе уже {limits.apps} приложений — это предел</span> : null}
      </div>
      {importing ? (
        <ImportDialog
          configs={configs}
          initial={configs.some((item) => item.uuid === look) ? look : (configs[0]?.uuid ?? '')}
          current={total}
          onClose={() => setImporting(false)}
          onApply={(incoming, mode) => {
            onChange(mode === 'replace' ? fullApps(incoming) : merge(apps, incoming, limits.apps));
            setImporting(false);
            setOpen(null);
            setNote(mode === 'replace' ? 'Список заменён приложениями из Remnawave. Проверьте его и сохраните' : 'Приложения из Remnawave добавлены. Проверьте список и сохраните');
          }}
        />
      ) : null}
    </>
  );
}
