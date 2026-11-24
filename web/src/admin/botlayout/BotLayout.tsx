import { useEffect, useRef, useState, type DragEvent, type KeyboardEvent } from 'react';
import { Link } from 'react-router';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { Switch } from '../ui';
import '../form.css';
import { BotTabs } from '../pages/BotTabs';
import { SettingsLayout } from '../pages/SettingsLayout';
import {
  applyMove,
  arrange,
  dropTarget,
  gapTarget,
  keyboardFor,
  kindIn,
  MENU_MODES,
  messageFor,
  moveTarget,
  opensText,
  pack,
  place,
  placeText,
  project,
  sameRows,
  spread,
  STATE_LABEL,
  type ClientState,
  type Draft,
  type Lang,
  type Move,
  type ScreenKey,
  type ScreenView,
  type Slot,
  type Target,
  webOn,
} from './model';
import { labelOf, TgPreview } from './TgPreview';
import { useLayoutEditor } from './useLayoutEditor';
import './bot-layout.css';

type Mode = 'all' | ClientState;
type Drop = { key: string; after: boolean } | { gap: number } | null;

const MOVES: Array<{ move: Move; label: string; icon?: 'chevron-left' | 'chevron-right' | 'arrow-up' | 'arrow-down'; after?: boolean }> = [
  { move: 'left', label: 'Левее', icon: 'chevron-left' },
  { move: 'right', label: 'Правее', icon: 'chevron-right', after: true },
  { move: 'up', label: 'В ряд выше', icon: 'arrow-up' },
  { move: 'down', label: 'В ряд ниже', icon: 'arrow-down' },
  { move: 'own', label: 'Отдельным рядом' },
];

const KEY_MOVES: Record<string, Move> = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down' };

const SPREAD = [
  { n: 1, label: 'по одной' },
  { n: 2, label: 'по две' },
  { n: 3, label: 'по три' },
];

function allKeys(screen: ScreenView, draft: Draft): Slot[][] {
  const present: Array<[string, Slot]> = screen.slots.map((slot) => [slot.key, slot]);
  return draft.custom || screen.packed === null ? arrange(draft.rows, present) : pack(present, screen.packed);
}

function hiddenWhy(slot: Slot, hidden: boolean, state: ClientState): string {
  if (hidden) {
    return 'скрыта: включите «Показывать в боте»';
  }
  if (slot.off) {
    return slot.off.charAt(0).toLowerCase() + slot.off.slice(1);
  }
  if (slot.audience === 'subscribed') {
    return 'появится, когда будет подписка';
  }
  if (slot.audience === 'unsubscribed') {
    return state === 'new' ? 'этому клиенту сейчас не нужна' : 'видна только тем, у кого нет подписки';
  }
  return 'этому клиенту не видна';
}

export function SettingsBotLayout() {
  const editor = useLayoutEditor();
  const phone = usePhone();
  const [screenKey, setScreenKey] = useState<ScreenKey>('menu');
  const [mode, setMode] = useState<Mode>('all');
  const [lang, setLang] = useState<Lang>('ru');
  const [selected, setSelected] = useState<string | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const [drop, setDrop] = useState<Drop>(null);
  const [said, setSaid] = useState('');
  const refocus = useRef<string | null>(null);
  const stage = useRef<HTMLDivElement>(null);
  const sheet = useRef<HTMLDivElement>(null);

  const { view, drafts } = editor;
  const screen = view?.screens.find((s) => s.key === screenKey) ?? null;
  const draft = drafts?.[screenKey] ?? null;

  useEffect(() => {
    const key = refocus.current;
    if (!key || !stage.current) {
      return;
    }
    refocus.current = null;
    stage.current.querySelector<HTMLButtonElement>(`[data-key="${CSS.escape(key)}"]`)?.focus();
  });

  useEffect(() => {
    if (!phone || !selected || !stage.current || !sheet.current) {
      return;
    }
    const key = stage.current.querySelector(`[data-key="${CSS.escape(selected)}"]`);
    if (!key) {
      return;
    }
    const box = key.getBoundingClientRect();
    const room = sheet.current.getBoundingClientRect().top - 16;
    if (box.bottom > room || box.top < 16) {
      window.scrollBy({ top: box.bottom - room + Math.max(0, (room - 16 - box.height) / 2) });
    }
  }, [phone, selected, draft]);

  if (!view || !screen || !draft) {
    return (
      <SettingsLayout>
        <BotTabs />
        {editor.error ? (
          <p className="err" role="alert">
            {editor.error}
          </p>
        ) : (
          <p className="muted">Загружаем кнопки бота…</p>
        )}
      </SettingsLayout>
    );
  }

  const english = view.english;
  const shownLang: Lang = english ? lang : 'ru';
  const isHidden = (slot: Slot) => editor.isHidden(slot.key, slot.hidden);
  const bySlot = new Map(screen.slots.map((slot) => [slot.key, slot]));
  const keyboard = mode === 'all' ? allKeys(screen, draft) : keyboardFor(screen, draft, mode, editor.hidden);
  const shownKeys = keyboard.map((row) => row.map((slot) => slot.key));
  const visible = (key: string) => {
    const slot = bySlot.get(key);
    return mode === 'all' || (slot !== undefined && slot.shown.includes(mode) && !isHidden(slot));
  };
  const web = webOn(view, editor.menuMode);
  const marked = screenKey === 'menu' ? keyboard.map((row) => row.map((slot) => ({ ...slot, kind: kindIn(slot, web) }))) : keyboard;
  const modeText = MENU_MODES.find((item) => item.mode === editor.menuMode)?.text ?? '';
  const projection = project(draft.rows, visible);
  const pick = selected ? (bySlot.get(selected) ?? null) : null;
  const pickShown = pick !== null && shownKeys.some((row) => row.includes(pick.key));
  const notShown = mode === 'all' ? [] : screen.slots.filter((slot) => !visible(slot.key));
  const changedHere =
    draft.custom !== screen.custom ||
    !sameRows(draft.rows, screen.rows) ||
    editor.modeChanged ||
    screen.slots.some((slot) => editor.hidden[slot.key] !== undefined && editor.hidden[slot.key] !== slot.hidden);
  const standardNow = !draft.custom && sameRows(draft.rows, screen.standard);
  const text = messageFor(view, screenKey, mode === 'all' ? 'active' : mode, shownLang);

  const commit = (rows: string[][], key: string) => {
    if (sameRows(rows, draft.rows)) {
      return;
    }
    editor.setRows(screenKey, rows);
    const after = project(rows, visible).rows;
    const where = placeText(after, key);
    const slot = bySlot.get(key);
    if (where && slot) {
      setSaid(`«${labelOf(slot, shownLang)}»: ${where.toLowerCase()}`);
    }
  };

  const move = (key: string, how: Move) => commit(applyMove(draft.rows, projection, key, how), key);

  const can = (how: Move) => pick !== null && pickShown && moveTarget(draft.rows, projection, pick.key, how) !== null;

  const switchScreen = (key: ScreenKey) => {
    setScreenKey(key);
    setSelected(null);
    const next = view.screens.find((s) => s.key === key);
    if (mode !== 'all' && next && !next.states.includes(mode)) {
      setMode('all');
    }
  };

  const onKey = (key: string, event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'Escape') {
      setSelected(null);
      return;
    }
    const how = KEY_MOVES[event.key];
    if (!how || selected !== key) {
      return;
    }
    event.preventDefault();
    refocus.current = key;
    move(key, how);
  };

  const onDragStart = (key: string, event: DragEvent<HTMLButtonElement>) => {
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/plain', key);
    setDragging(key);
    setSelected(key);
  };

  const onDragEnd = () => {
    setDragging(null);
    setDrop(null);
  };

  const onDragOverKey = (over: string, event: DragEvent<HTMLButtonElement>) => {
    if (!dragging) {
      return;
    }
    const box = event.currentTarget.getBoundingClientRect();
    const after = event.clientX > box.left + box.width / 2;
    if (dropTarget(draft.rows, dragging, over, after)) {
      event.preventDefault();
      event.dataTransfer.dropEffect = 'move';
      if (!drop || !('key' in drop) || drop.key !== over || drop.after !== after) {
        setDrop({ key: over, after });
      }
    } else if (drop) {
      setDrop(null);
    }
  };

  const finish = (key: string, target: Target | null) => {
    if (target) {
      commit(place(draft.rows, key, target), key);
    }
    onDragEnd();
  };

  const onDropKey = (over: string, event: DragEvent<HTMLButtonElement>) => {
    event.preventDefault();
    if (!dragging) {
      return;
    }
    const box = event.currentTarget.getBoundingClientRect();
    finish(dragging, dropTarget(draft.rows, dragging, over, event.clientX > box.left + box.width / 2));
  };

  const gap = (index: number) =>
    dragging ? (
      <div
        className={drop && 'gap' in drop && drop.gap === index ? 'kb-gap on' : 'kb-gap'}
        onDragOver={(event) => {
          event.preventDefault();
          event.dataTransfer.dropEffect = 'move';
          if (!drop || !('gap' in drop) || drop.gap !== index) {
            setDrop({ gap: index });
          }
        }}
        onDragLeave={() => setDrop(null)}
        onDrop={(event) => {
          event.preventDefault();
          if (dragging) {
            finish(dragging, gapTarget(draft.rows, shownKeys, index));
          }
        }}
      >
        <span>Новый ряд</span>
      </div>
    ) : null;

  const partial = (slot: Slot) => mode === 'all' && slot.shown.length > 0 && slot.shown.length < screen.states.length;
  const faded = (slot: Slot) => mode === 'all' && (slot.shown.length === 0 || isHidden(slot));

  const card = pick ? (
    <>
      <div className="kb-card-head">
        <span className="cap kb-kind">Кнопка</span>
        <span className="kb-card-title">
          <strong>{labelOf(pick, shownLang)}</strong>
          {phone ? (
            <button type="button" className="bt-reset hov" onClick={() => setSelected(null)}>
              Готово
            </button>
          ) : null}
        </span>
        <span className="cap">{pickShown ? placeText(shownKeys, pick.key) : 'Этому клиенту не видна'}</span>
      </div>
      {pickShown ? (
        <div className="kb-moves" role="group" aria-label={`Переставить «${labelOf(pick, shownLang)}»`}>
          {MOVES.map((item) => (
            <button
              key={item.move}
              type="button"
              className={item.move === 'own' ? 'b40 hov wide' : 'b40 hov'}
              disabled={!can(item.move)}
              onClick={() => move(pick.key, item.move)}
            >
              {item.icon && !item.after ? <Icon name={item.icon} size={18} /> : null}
              {item.label}
              {item.icon && item.after ? <Icon name={item.icon} size={18} /> : null}
            </button>
          ))}
        </div>
      ) : (
        <button type="button" className="b40 hov kb-all" onClick={() => setMode('all')}>
          Показать среди всех кнопок
        </button>
      )}
      {pick.hideable ? (
        <label className="kb-show">
          <Switch on={!isHidden(pick)} onToggle={() => editor.toggleHidden(pick.key, !isHidden(pick))} label={`Показывать кнопку «${pick.text}» в боте`} />
          <span>Показывать в боте</span>
        </label>
      ) : (
        <span className="cap">Эта кнопка нужна всегда, скрыть её нельзя.</span>
      )}
      <div className="kb-when">
        <span className="cap">Когда видна</span>
        <span>{pick.when}</span>
        {pick.off ? <span className="kb-off">Сейчас не видна: {pick.off.charAt(0).toLowerCase() + pick.off.slice(1)}</span> : null}
      </div>
      <div className="kb-when">
        <span className="cap">Что открывает</span>
        <span>{opensText(pick, screenKey === 'menu' && web)}</span>
        {screenKey === 'menu' && web && pick.web === null && pick.web_note ? <span className="kb-off">{pick.web_note}</span> : null}
      </div>
      {phone ? null : (
        <Link to="/admin/settings/bot/texts" className="kb-link">
          Надпись, цвет и премиум-эмодзи — в «Тексты и кнопки»
        </Link>
      )}
    </>
  ) : (
    <p className="cap kb-card-empty">
      {phone
        ? 'Нажмите кнопку в предпросмотре, чтобы переставить её, вынести в отдельный ряд или скрыть.'
        : 'Нажмите кнопку в предпросмотре, чтобы переставить её стрелками, вынести в отдельный ряд или скрыть. Нажатую кнопку двигают и клавишами со стрелками.'}
    </p>
  );

  const dirty = editor.unsaved > 0;

  return (
    <SettingsLayout>
      <BotTabs />
      <section className="sec kb-mode" aria-labelledby="kb-mode-h">
        <div className="sec-head">
          <h2 id="kb-mode-h" className="section-title">
            Режим меню
          </h2>
          <span className="muted">Что открывают кнопки главного меню: экраны бота или разделы мини-приложения</span>
        </div>
        <div className="row">
          <span className="rl">Кнопки меню</span>
          <div className="rc">
            <span className="segs" role="group" aria-label="Режим меню">
              {MENU_MODES.map((item) => (
                <button
                  key={item.mode}
                  type="button"
                  aria-pressed={editor.menuMode === item.mode}
                  disabled={editor.busy || (item.mode === 'web' && view.web_off !== null && editor.menuMode !== 'web')}
                  onClick={() => editor.setMenuMode(item.mode)}
                >
                  {item.label}
                </button>
              ))}
            </span>
            {view.web_off ? (
              <span className={`kb-mode-note ${editor.menuMode === 'web' ? 'warn-line' : 'cap'}`}>
                {editor.menuMode === 'web' ? 'Сейчас кнопки работают как в классическом режиме. ' : 'Веб-режим недоступен. '}
                {view.web_off}. <Link to="/admin/settings/client">Кабинет и мини-приложение</Link>
              </span>
            ) : (
              <span className="cap">{modeText}</span>
            )}
            {web ? <span className="cap">Стрелка в углу кнопки значит, что она откроет мини-приложение или ссылку, а не экран бота.</span> : null}
          </div>
        </div>
      </section>
      <section className="sec" aria-labelledby="kb-h">
        <div className="sec-head">
          <h2 id="kb-h" className="section-title">
            Порядок кнопок
          </h2>
          <span className="muted">
            Перетащите кнопку мышью или нажмите на неё и двигайте стрелками. Между рядами получится новый ряд, в ряду до трёх кнопок. Порядок общий для всех
            языков.
          </span>
        </div>
      </section>

      <div className="kb-bar">
        <span className="segs" role="group" aria-label="Экран бота">
          {view.screens.map((s) => (
            <button key={s.key} type="button" aria-pressed={s.key === screenKey} onClick={() => switchScreen(s.key)}>
              {s.title}
            </button>
          ))}
        </span>
        {english ? (
          <span className="segs" role="group" aria-label="Язык подписей">
            {(['ru', 'en'] as Lang[]).map((value) => (
              <button key={value} type="button" aria-pressed={lang === value} onClick={() => setLang(value)}>
                {value === 'ru' ? 'RU' : 'EN'}
              </button>
            ))}
          </span>
        ) : null}
      </div>
      <div className="kb-states">
        <span className="segs" role="group" aria-label="Кого показать">
          {(['all', ...screen.states] as Mode[]).map((value) => (
            <button key={value} type="button" aria-pressed={mode === value} onClick={() => setMode(value)}>
              {value === 'all' ? 'Все кнопки' : STATE_LABEL[value]}
            </button>
          ))}
        </span>
      </div>

      <div className="kb-grid">
        <div className="kb-stage" ref={stage}>
          <TgPreview
            html={text}
            rows={marked}
            lang={shownLang}
            brand={view.brand}
            empty="Этому клиенту не видно ни одной кнопки"
            marks={{ selected, ghost: dragging, partial, faded, drop: drop && 'key' in drop ? drop : null }}
            events={{
              onPick: (key) => setSelected(key === selected ? null : key),
              onKeyDown: onKey,
              onDragStart: phone ? undefined : onDragStart,
              onDragEnd,
              onDragOver: onDragOverKey,
              onDrop: onDropKey,
            }}
            renderRow={(_, index, keys) => (
              <>
                {index === 0 ? gap(0) : null}
                {keys}
                {gap(index + 1)}
              </>
            )}
          />
          {mode === 'all' ? (
            <div className="kb-legend">
              <p className="cap">Пунктиром обведены кнопки, которые видны не всем клиентам. Бледные сейчас не видит никто: модуль выключен или кнопка скрыта.</p>
              <span className="kb-spread">
                <span className="cap">Разложить:</span>
                {SPREAD.map((item) => (
                  <button key={item.n} type="button" className="bt-reset hov" onClick={() => editor.setRows(screenKey, spread(draft.rows, item.n))}>
                    {item.label}
                  </button>
                ))}
                <span className="cap">в ряд</span>
              </span>
            </div>
          ) : null}
          {screen.packed === 2 && !draft.custom ? (
            <p className="cap kb-note">
              Сейчас кнопки меню идут по две подряд: если какой-то кнопки нет, следующая встаёт на её место. После любой перестановки ряды станут постоянными, как в «Все кнопки».
            </p>
          ) : null}
          <span className="sr-only" role="status" aria-live="polite">
            {said}
          </span>
        </div>
        <div className="kb-side">
          {phone ? null : <aside className="sec kb-card">{card}</aside>}
          {notShown.length ? (
            <section className="sec kb-off-list" aria-labelledby="kb-off">
              <div className="sec-head">
                <h3 id="kb-off" className="section-title">
                  Не видны этому клиенту
                </h3>
              </div>
              {notShown.map((slot) => (
                <button key={slot.key} type="button" className="kb-off-row hov" aria-pressed={selected === slot.key} onClick={() => setSelected(slot.key)}>
                  <span className="kb-off-name">{labelOf(slot, shownLang)}</span>
                  <span className="cap">{hiddenWhy(slot, isHidden(slot), mode as ClientState)}</span>
                </button>
              ))}
            </section>
          ) : null}
        </div>
      </div>

      {!phone || dirty || editor.status ? (
        <div className="save-row bt-save kb-save">
          <button type="button" className="btn-main btn" onClick={() => void editor.save()} disabled={editor.busy || !dirty}>
            {editor.busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
          <button type="button" className="btn-line hov" onClick={editor.cancel} disabled={editor.busy || !dirty}>
            Отменить
          </button>
          <button type="button" className="bt-reset hov" onClick={() => editor.resetScreen(screenKey)} disabled={editor.busy || standardNow} title={`Стандартный порядок для экрана «${screen.title}»`}>
            Вернуть как было
          </button>
          <span role="status" className={editor.status?.bad ? 'bad' : undefined}>
            {editor.status?.text ?? (dirty ? `Не сохранено: ${editor.unsaved}${changedHere ? '' : ' на другом экране'}` : '')}
          </span>
        </div>
      ) : null}

      {phone && pick ? (
        <>
          <div className="kb-room" aria-hidden="true" />
          <div className="kb-sheet" ref={sheet} role="dialog" aria-label={`Кнопка «${labelOf(pick, shownLang)}»`}>
            {card}
          </div>
        </>
      ) : null}
    </SettingsLayout>
  );
}
