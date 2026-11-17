import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { count, groupDigits, NBSP, plural } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { ConfirmDialog, Dialog } from '../client/dialogs';
import { useSession } from '../session';
import { Switch } from '../ui';
import {
  blankDraft,
  devicesText,
  draftOf,
  fromRaw,
  GIB,
  inputOf,
  nextKey,
  perMonthNote,
  priceOf,
  priceText,
  RESETS,
  salesWord,
  termText,
  trafficText,
  type Applied,
  type Channels,
  type Draft,
  type Squad,
  type Tariff,
  type TrafficReset,
  type View,
} from './model';
import '../form.css';
import './tariffs.css';

type Trial = { enabled: boolean; days: number; squads: string[]; traffic: { limit_bytes: number | null; reset: TrafficReset }; devices: number };

const DEFAULT_CHANNELS: Channels = { bot: true, cabinet: true, miniapp: true };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function Tariffs() {
  const { can } = useSession();
  const settingsOk = can('settings');
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [squads, setSquads] = useState<Squad[] | null>(null);
  const [category, setCategory] = useState<number | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [saved, setSaved] = useState<{ id: number; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [hwidOff, setHwidOff] = useState(false);
  const [trial, setTrial] = useState<Trial | null>(null);
  const [dialog, setDialog] = useState<'' | 'category' | 'trial' | 'delete'>('');

  const load = useCallback(async () => {
    try {
      const data = await api<View>('/api/admin/tariffs');
      setView(data);
      setError(null);
      setCategory((current) => (current && data.categories.some((c) => c.id === current) ? current : (data.categories[0]?.id ?? null)));
      return data;
    } catch (err) {
      setError(failText(err, 'Тарифы не загрузились'));
      return null;
    }
  }, []);

  useEffect(() => {
    void load();
    api<Squad[]>('/api/admin/tariffs/squads')
      .then(setSquads)
      .catch(() => setSquads([]));
    if (settingsOk) {
      api<{ hwidSettings?: { enabled: boolean } | null }>('/api/admin/settings/subscription')
        .then((sub) => setHwidOff(sub.hwidSettings ? !sub.hwidSettings.enabled : false))
        .catch(() => setHwidOff(false));
      api<Trial>('/api/admin/settings/trial')
        .then(setTrial)
        .catch(() => setTrial(null));
    }
  }, [load, settingsOk]);

  const currency = view?.currency ?? 'USD';
  const stats = useMemo(() => new Map((view?.stats ?? []).map((s) => [s.tariff_id, s])), [view]);
  const squadName = useCallback(
    (uuid: string) => squads?.find((s) => s.uuid === uuid)?.name ?? `${uuid.slice(0, 8)}…`,
    [squads],
  );
  const counts = useMemo(() => {
    const map = new Map<number, number>();
    for (const t of view?.tariffs ?? []) {
      map.set(t.category_id, (map.get(t.category_id) ?? 0) + 1);
    }
    return map;
  }, [view]);
  const shown = (view?.tariffs ?? []).filter((t) => t.category_id === category);

  const pickCategory = (id: number) => {
    setCategory(id);
    setDraft(null);
    setSaved(null);
    setFormError(null);
  };

  const open = (tariff: Tariff) => {
    if (draft?.id === tariff.id) {
      setDraft(null);
      setFormError(null);
      return;
    }
    setSaved(null);
    setFormError(null);
    setDraft(draftOf(tariff, stats.get(tariff.id)?.channels ?? DEFAULT_CHANNELS, currency));
  };

  const startNew = () => {
    if (!view || category === null) {
      setDialog('category');
      return;
    }
    setSaved(null);
    setFormError(null);
    setDraft(blankDraft(category, squads ?? [], currency));
  };

  const toggleSale = async (tariff: Tariff) => {
    try {
      await api(`/api/admin/tariffs/${tariff.id}`, { method: 'PATCH', body: { on_sale: !tariff.on_sale } });
      await load();
    } catch (err) {
      setError(failText(err, 'Не получилось изменить продажу'));
    }
  };

  const save = async () => {
    if (!draft) {
      return;
    }
    const { body, error: invalid } = inputOf(draft, currency);
    if (invalid) {
      setFormError(invalid);
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      let id = draft.id;
      let applied: Applied = null;
      if (id === null) {
        const created = await api<Tariff>('/api/admin/tariffs', { method: 'POST', body });
        id = created.id;
      } else {
        const updated = await api<Tariff & { applied: Applied }>(`/api/admin/tariffs/${id}${draft.apply ? '?apply=true' : ''}`, {
          method: 'PUT',
          body,
        });
        applied = updated.applied;
      }
      const before = stats.get(id)?.channels ?? DEFAULT_CHANNELS;
      const c = draft.channels;
      if (c.bot !== before.bot || c.cabinet !== before.cabinet || c.miniapp !== before.miniapp) {
        await api(`/api/admin/tariffs/${id}/channels`, { method: 'PUT', body: c });
      }
      let text = 'Сохранено.';
      if (applied && (applied.squads || applied.traffic || applied.devices)) {
        if (applied.users === 0) {
          text = 'Сохранено. Текущих подписчиков на тарифе нет';
        } else if (applied.failed > 0) {
          text = `Сохранено, но у ${groupDigits(applied.failed)} из ${groupDigits(applied.users)} подписок Remnawave не принял изменения. Повторите сохранение позже`;
        } else {
          text = `Сохранено. Изменения для текущих подписчиков отправлены в Remnawave: ${count(applied.users, 'подписка', 'подписки', 'подписок')}`;
        }
      }
      setDraft(null);
      setSaved({ id, text });
      await load();
    } catch (err) {
      setFormError(failText(err, 'Тариф не сохранился'));
    } finally {
      setBusy(false);
    }
  };

  const edit = (patch: Partial<Draft>) => setDraft((d) => (d ? { ...d, ...patch } : d));

  if (error && !view) {
    return (
      <div className="tf">
        <h1 className="page-title">Тарифы</h1>
        <p className="err" role="alert">
          {error}
        </p>
      </div>
    );
  }

  const subtitle = `Что видят клиенты в боте, кабинете и мини-приложении. ${currency === 'RUB' ? 'Цены в рублях' : 'Цены в долларах'}, валюта выбирается в настройках.`;
  const isNew = draft !== null && draft.id === null;

  return (
    <div className="tf">
      <header className="tf-head">
        <div className="tf-title">
          <h1 className="page-title">Тарифы</h1>
          <span className="muted">{subtitle}</span>
        </div>
        {settingsOk && trial ? (
          <button type="button" className="b44 hov tf-trial" onClick={() => setDialog('trial')}>
            Пробный период <span className="muted">{trial.enabled ? `${trial.days}${NBSP}${plural(trial.days, 'день', 'дня', 'дней')}` : 'выключен'}</span>
          </button>
        ) : null}
        <button type="button" className="btn-main btn" onClick={startNew} style={{ padding: '0 18px' }}>
          Новый тариф
        </button>
      </header>

      <div role="group" aria-label="Категория" className="chips">
        {(view?.categories ?? []).map((c) => (
          <button key={c.id} type="button" className="chip" aria-pressed={c.id === category} onClick={() => pickCategory(c.id)}>
            <span>{c.name}</span>
            <span className="chip-n">{counts.get(c.id) ?? 0}</span>
          </button>
        ))}
        <button type="button" className="chip dashed hov" onClick={() => setDialog('category')}>
          Новая категория
        </button>
      </div>

      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}

      {isNew && draft ? (
        <section aria-label="Новый тариф" className="tf-sec">
          <div className="tf-row">
            <div className="tf-info">
              <h2 className="tf-name">Новый тариф</h2>
              <span className="muted">Заполните поля и сохраните — тариф появится в выбранной категории</span>
            </div>
          </div>
          <TariffForm
            draft={draft}
            currency={currency}
            squads={squads}
            hwidOff={hwidOff}
            busy={busy}
            error={formError}
            onEdit={edit}
            onSave={() => void save()}
            onCancel={() => {
              setDraft(null);
              setFormError(null);
            }}
            onDelete={null}
          />
        </section>
      ) : null}

      {view && category !== null && shown.length === 0 && !isNew ? (
        <section className="tf-sec tf-empty">
          <p className="muted">В этой категории пока нет тарифов.</p>
          <div className="cc-btns" style={{ gap: 8 }}>
            <button type="button" className="b38 hov" onClick={startNew}>
              Создать тариф
            </button>
            <button type="button" className="b38 hov" style={{ color: 'var(--bad)' }} onClick={() => setDialog('delete')}>
              Удалить категорию
            </button>
          </div>
        </section>
      ) : null}

      {view && view.categories.length === 0 ? (
        <section className="tf-sec tf-empty">
          <p className="muted">Тарифов ещё нет. Начните с категории: например, «Стандарт» или «Белые списки».</p>
          <div className="cc-btns">
            <button type="button" className="b38 hov" onClick={() => setDialog('category')}>
              Создать категорию
            </button>
          </div>
        </section>
      ) : null}

      {shown.map((tariff) => {
        const st = stats.get(tariff.id);
        const sales = st?.sales_30d ?? 0;
        const opened = draft?.id === tariff.id;
        const meta = [devicesText(tariff.devices), trafficText(tariff.traffic), `сквады: ${tariff.squads.map(squadName).join(', ')}`].join(' · ');
        return (
          <section key={tariff.id} aria-label={tariff.name} className="tf-sec">
            <div className="tf-row">
              <div className="tf-info">
                <div className="tf-namerow">
                  <h2 className="tf-name">{tariff.name}</h2>
                  {tariff.description ? <span className="muted">{tariff.description}</span> : null}
                </div>
                <div className="tf-prices">
                  {tariff.periods.map((p) => (
                    <span key={p.id} className="tf-price">
                      <span className="muted">{termText(p.days)}</span>
                      <strong className="num">{priceText(priceOf(p.prices, currency), currency)}</strong>
                    </span>
                  ))}
                </div>
                <span style={{ color: 'var(--ink-2)' }}>{meta}</span>
              </div>
              <div className="tf-side">
                <span className="tf-sales">
                  <strong className="num">{groupDigits(sales)}</strong>
                  <span>{salesWord(sales)}</span>
                </span>
                <span className="tf-sale">
                  <Switch on={tariff.on_sale} onToggle={() => void toggleSale(tariff)} label={`В продаже: ${tariff.name}`} />
                  <span style={{ color: tariff.on_sale ? 'var(--ink-2)' : 'var(--muted)' }}>{tariff.on_sale ? 'В продаже' : 'Скрыт'}</span>
                </span>
                <button type="button" className="b40 hov" aria-expanded={opened} onClick={() => open(tariff)}>
                  {opened ? 'Свернуть' : 'Изменить'}
                </button>
              </div>
            </div>
            {saved?.id === tariff.id && !opened ? (
              <div style={{ padding: '0 20px 18px' }}>
                <p role="status" className="tf-saved">
                  <strong>{saved.text.split(' ')[0]}</strong> {saved.text.split(' ').slice(1).join(' ')}
                </p>
              </div>
            ) : null}
            {opened && draft ? (
              <TariffForm
                draft={draft}
                currency={currency}
                squads={squads}
                hwidOff={hwidOff}
                busy={busy}
                error={formError}
                onEdit={edit}
                onSave={() => void save()}
                onCancel={() => {
                  setDraft(null);
                  setFormError(null);
                }}
                onDelete={() => setDialog('delete')}
              />
            ) : null}
          </section>
        );
      })}

      {dialog === 'category' ? (
        <CategoryDialog
          onClose={() => setDialog('')}
          onDone={async (id) => {
            setDialog('');
            await load();
            pickCategory(id);
          }}
        />
      ) : null}
      {dialog === 'trial' && trial ? (
        <TrialDialog
          trial={trial}
          squads={squads ?? []}
          onClose={() => setDialog('')}
          onDone={(next) => {
            setTrial(next);
            setDialog('');
          }}
        />
      ) : null}
      {dialog === 'delete' && draft?.id ? (
        <ConfirmDialog
          title="Удалить тариф"
          text="Тариф пропадёт из продажи и из списка. Удалить можно, только если на нём нет действующих подписок; оплаты останутся в отчётах."
          confirm="Удалить тариф"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`/api/admin/tariffs/${draft.id}`, { method: 'DELETE' });
            setDialog('');
            setDraft(null);
            await load();
          }}
        />
      ) : null}
      {dialog === 'delete' && !draft && category !== null ? (
        <ConfirmDialog
          title="Удалить категорию"
          text="Пустая категория исчезнет из списка и из каталога для клиентов."
          confirm="Удалить категорию"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`/api/admin/categories/${category}`, { method: 'DELETE' });
            setDialog('');
            setCategory(null);
            await load();
          }}
        />
      ) : null}
    </div>
  );
}

type FormProps = {
  draft: Draft;
  currency: 'USD' | 'RUB';
  squads: Squad[] | null;
  hwidOff: boolean;
  busy: boolean;
  error: string | null;
  onEdit: (patch: Partial<Draft>) => void;
  onSave: () => void;
  onCancel: () => void;
  onDelete: (() => void) | null;
};

function TariffForm({ draft, currency, squads, hwidOff, busy, error, onEdit, onSave, onCancel, onDelete }: FormProps) {
  const ids = useId();
  const sym = currency === 'RUB' ? '₽' : '$';
  const base = draft.periods.find((p) => Number(p.days) === 30);
  const baseMinor = base ? fromRaw(base.price, currency) : null;
  const resetHint = draft.gb.trim() ? (RESETS.find((r) => r.value === draft.reset)?.hint ?? '') : '';
  const extraMax = Number(draft.extraMax);
  const extraWord = Number.isInteger(extraMax) && extraMax % 10 === 1 && extraMax % 100 !== 11 ? 'устройства' : 'устройств';
  const knownSquads = squads ?? [];
  const unknown = draft.squads.filter((s) => !knownSquads.some((k) => k.uuid === s));

  const setPeriod = (key: string, patch: Partial<Draft['periods'][number]>) =>
    onEdit({ periods: draft.periods.map((p) => (p.key === key ? { ...p, ...patch } : p)) });
  const toggleSquad = (uuid: string) => {
    const on = draft.squads.includes(uuid);
    if (on && draft.squads.length === 1) {
      return;
    }
    onEdit({ squads: on ? draft.squads.filter((s) => s !== uuid) : [...draft.squads, uuid] });
  };
  const toggleChannel = (key: keyof Channels) => {
    const next = { ...draft.channels, [key]: !draft.channels[key] };
    if (!next.bot && !next.cabinet && !next.miniapp) {
      return;
    }
    onEdit({ channels: next });
  };

  return (
    <div className="tf-form">
      <div className="tf-grid">
        <label className="tf-field">
          <span>Название</span>
          <input className="field" type="text" value={draft.name} maxLength={100} onChange={(e) => onEdit({ name: e.target.value })} disabled={busy} />
        </label>
        <label className="tf-field">
          <span>Описание для клиента</span>
          <input className="field" type="text" value={draft.description} maxLength={2000} onChange={(e) => onEdit({ description: e.target.value })} disabled={busy} />
        </label>
      </div>

      <div className="tf-block" style={{ gap: 8 }}>
        <span className="tf-bt">Цены</span>
        {draft.periods.map((p) => {
          const days = Number(p.days);
          const minor = fromRaw(p.price, currency);
          const note =
            Number.isInteger(days) && typeof minor === 'number' && typeof baseMinor === 'number' && p.key !== base?.key
              ? perMonthNote(days, minor, baseMinor, currency)
              : Number.isInteger(days) && typeof minor === 'number' && p.key !== base?.key
                ? perMonthNote(days, minor, null, currency)
                : '';
          const term = Number.isInteger(days) && days > 0 ? termText(days) : 'срок';
          return (
            <div key={p.key} className="tf-period">
              <label className="tf-inline">
                <span className="sr-only">Срок в днях</span>
                <input className="field num" inputMode="numeric" value={p.days} onChange={(e) => setPeriod(p.key, { days: e.target.value.replace(/\D/g, '').slice(0, 4) })} style={{ width: 88 }} disabled={busy} />
                <span className="muted" style={{ minWidth: 36 }}>
                  {plural(Number.isInteger(days) ? days : 0, 'день', 'дня', 'дней')}
                </span>
              </label>
              <label className="tf-inline">
                <span className="sr-only">Цена</span>
                {currency === 'USD' ? <span className="muted">{sym}</span> : null}
                <input className="field num" inputMode="decimal" value={p.price} onChange={(e) => setPeriod(p.key, { price: e.target.value })} style={{ width: 104 }} disabled={busy} />
                {currency === 'RUB' ? <span className="muted">{sym}</span> : null}
              </label>
              <label className="tf-inline">
                <span className="sr-only">Цена в звёздах Telegram</span>
                <input className="field num" inputMode="numeric" value={p.stars} onChange={(e) => setPeriod(p.key, { stars: e.target.value.replace(/\D/g, '').slice(0, 7) })} style={{ width: 96 }} disabled={busy} />
                <span className="muted" style={{ minWidth: 48 }}>
                  {plural(Number(p.stars) || 0, 'звезда', 'звезды', 'звёзд')}
                </span>
              </label>
              <span className="muted tf-note">{note}</span>
              {draft.periods.length > 1 ? (
                <button type="button" className="tf-x hov" aria-label={`Убрать срок ${term}`} onClick={() => onEdit({ periods: draft.periods.filter((x) => x.key !== p.key) })} disabled={busy}>
                  <Icon name="close" size={16} stroke={2} />
                </button>
              ) : null}
            </div>
          );
        })}
        <button
          type="button"
          className="tf-add"
          onClick={() =>
            onEdit({
              periods: [...draft.periods, { key: nextKey(), id: null, days: '', price: '', stars: '', other: null, sort: draft.periods.length, active: true }],
            })
          }
          disabled={busy || draft.periods.length >= 8}
        >
          Добавить срок
        </button>
      </div>

      <div className="tf-block">
        <span className="tf-bt">Устройства</span>
        <div className="tf-wrap" style={{ alignItems: 'flex-end' }}>
          <label className="tf-field">
            <span>В тарифе</span>
            <input className="field num" inputMode="numeric" value={draft.included} onChange={(e) => onEdit({ included: e.target.value.replace(/\D/g, '').slice(0, 3) })} style={{ width: 104 }} disabled={busy} />
          </label>
          <span className="tf-switchrow">
            <Switch on={draft.extraOn} onToggle={() => onEdit({ extraOn: !draft.extraOn })} label="Продавать доп. устройства" />
            <span style={{ fontWeight: 600 }} aria-hidden="true">
              Продавать доп. устройства
            </span>
          </span>
          {draft.extraOn ? (
            <>
              <label className="tf-field">
                <span>Цена за устройство в месяц</span>
                <span className="tf-inline">
                  {currency === 'USD' ? <span className="muted">{sym}</span> : null}
                  <input className="field num" inputMode="decimal" value={draft.extraPrice} onChange={(e) => onEdit({ extraPrice: e.target.value })} style={{ width: 104 }} disabled={busy} />
                  {currency === 'RUB' ? <span className="muted">{sym}</span> : null}
                </span>
              </label>
              <label className="tf-field">
                <span>В звёздах</span>
                <span className="tf-inline">
                  <input className="field num" inputMode="numeric" value={draft.extraStars} onChange={(e) => onEdit({ extraStars: e.target.value.replace(/\D/g, '').slice(0, 6) })} style={{ width: 88 }} disabled={busy} />
                  <span className="muted">{plural(Number(draft.extraStars) || 0, 'звезда', 'звезды', 'звёзд')}</span>
                </span>
              </label>
              <label className="tf-field">
                <span>Не больше</span>
                <span className="tf-inline">
                  <input className="field num" inputMode="numeric" value={draft.extraMax} onChange={(e) => onEdit({ extraMax: e.target.value.replace(/\D/g, '').slice(0, 3) })} style={{ width: 88 }} disabled={busy} />
                  <span className="muted">{extraWord}</span>
                </span>
              </label>
            </>
          ) : null}
        </div>
        {hwidOff ? (
          <p className="tf-warn">
            Лимит устройств выключен в Remnawave: клиенты подключат сколько угодно устройств.{' '}
            <a href="/admin/settings/remnawave#devices" aria-label="Включить лимит устройств в Remnawave">
              Включить
            </a>
          </p>
        ) : null}
      </div>

      <div className="tf-block">
        <span className="tf-bt">Трафик</span>
        <div className="tf-wrap" style={{ alignItems: 'flex-start' }}>
          <label className="tf-field">
            <span>Лимит трафика</span>
            <span className="tf-inline">
              <input className="field num" inputMode="numeric" placeholder="Без лимита" value={draft.gb} onChange={(e) => onEdit({ gb: e.target.value.replace(/\D/g, '').slice(0, 6) })} style={{ width: 140 }} disabled={busy} />
              <span className="muted">ГБ</span>
            </span>
          </label>
          <div className="tf-field">
            <label htmlFor={`${ids}-reset`} style={{ fontWeight: 600 }}>
              Сброс
            </label>
            <select
              id={`${ids}-reset`}
              className="field"
              value={draft.reset}
              onChange={(e) => onEdit({ reset: e.target.value as TrafficReset })}
              aria-describedby={resetHint ? `${ids}-hint` : undefined}
              disabled={busy}
              style={{ width: 300, maxWidth: '100%' }}
            >
              {RESETS.map((r) => (
                <option key={r.value} value={r.value}>
                  {r.label}
                </option>
              ))}
            </select>
            {resetHint ? (
              <span id={`${ids}-hint`} className="cap">
                {resetHint}
              </span>
            ) : null}
          </div>
        </div>
      </div>

      <div className="tf-block">
        <div className="tf-col" style={{ gap: 2 }}>
          <span id={`${ids}-sq`} className="tf-bt">
            Сквады
          </span>
          <span className="muted">К каким нодам получит доступ клиент</span>
        </div>
        <div role="group" aria-labelledby={`${ids}-sq`} className="chips">
          {squads === null ? <span className="muted">Загружаем сквады…</span> : null}
          {squads && squads.length === 0 && unknown.length === 0 ? <span className="muted">Remnawave не отдал сквады: проверьте подключение в настройках</span> : null}
          {knownSquads.map((s) => {
            const on = draft.squads.includes(s.uuid);
            const lock = on && draft.squads.length === 1;
            return (
              <button key={s.uuid} type="button" className="chip" aria-pressed={on} aria-disabled={lock} onClick={() => toggleSquad(s.uuid)} style={lock ? { cursor: 'default', opacity: 1 } : undefined}>
                <span>{s.name}</span>
                {s.nodes !== null ? <span className="chip-n">{count(s.nodes, 'нода', 'ноды', 'нод')}</span> : null}
              </button>
            );
          })}
          {unknown.map((uuid) => (
            <button key={uuid} type="button" className="chip" aria-pressed="true" onClick={() => toggleSquad(uuid)} title="Сквада нет в Remnawave">
              <span>{uuid.slice(0, 8)}…</span>
              <span className="chip-n">нет в панели</span>
            </button>
          ))}
        </div>
        {draft.squads.length === 1 ? <span className="cap">Нужен хотя бы один сквад, последний снять нельзя.</span> : null}
      </div>

      <fieldset className="tf-fieldset">
        <legend className="tf-bt">Где показывать</legend>
        <div className="tf-wrap" style={{ gap: '10px 24px' }}>
          {(
            [
              ['bot', 'Бот'],
              ['cabinet', 'Кабинет'],
              ['miniapp', 'Мини-приложение'],
            ] as Array<[keyof Channels, string]>
          ).map(([key, label]) => (
            <label key={key} className="tf-check">
              <input type="checkbox" checked={draft.channels[key]} onChange={() => toggleChannel(key)} disabled={busy} />
              {label}
            </label>
          ))}
        </div>
      </fieldset>

      <div className="tf-col" style={{ gap: 16 }}>
        {draft.id !== null ? (
          <div className="tf-col" style={{ gap: 2 }}>
            <label className="tf-check" style={{ fontWeight: 600, width: 'fit-content', cursor: 'pointer' }}>
              <input type="checkbox" checked={draft.apply} onChange={() => onEdit({ apply: !draft.apply })} aria-describedby={`${ids}-apply`} disabled={busy} />
              <span>Применить к текущим подписчикам тарифа</span>
            </label>
            <span id={`${ids}-apply`} className="cap" style={{ paddingLeft: 25 }}>
              Без галочки изменения коснутся только новых покупок
            </span>
          </div>
        ) : null}
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <div className="tf-actions">
          <button type="button" className="btn-main btn" onClick={onSave} disabled={busy}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
          <button type="button" className="btn-line hov" onClick={onCancel} disabled={busy} style={{ padding: '0 16px' }}>
            Отменить
          </button>
          <span style={{ flex: 1 }} />
          {onDelete ? (
            <button type="button" className="tf-del" onClick={onDelete} disabled={busy}>
              Удалить тариф
            </button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function CategoryDialog({ onClose, onDone }: { onClose: () => void; onDone: (id: number) => Promise<void> }) {
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useId();
  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const created = await api<{ id: number }>('/api/admin/categories', { method: 'POST', body: { name: name.trim() } });
      await onDone(created.id);
    } catch (err) {
      setError(failText(err, 'Категория не создалась'));
      setBusy(false);
    }
  };
  return (
    <Dialog title="Новая категория" busy={busy} onClose={onClose}>
      <div className="field-row">
        <label htmlFor={field}>Название</label>
        <input id={field} className="field" maxLength={60} value={name} onChange={(e) => setName(e.target.value)} placeholder="Например: Белые списки" disabled={busy} autoFocus />
        <span className="cap">Клиенты увидят её как раздел каталога</span>
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void create()} disabled={busy || !name.trim()}>
          {busy ? 'Создаём…' : 'Создать'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

function TrialDialog({ trial, squads, onClose, onDone }: { trial: Trial; squads: Squad[]; onClose: () => void; onDone: (trial: Trial) => void }) {
  const [enabled, setEnabled] = useState(trial.enabled);
  const [daysRaw, setDays] = useState(String(trial.days));
  const [devices, setDevices] = useState(String(trial.devices));
  const [gb, setGb] = useState(trial.traffic.limit_bytes ? String(Math.round(trial.traffic.limit_bytes / GIB)) : '');
  const [chosen, setChosen] = useState<string[]>(trial.squads);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ids = useId();
  const save = async () => {
    const days = Number(daysRaw);
    const dev = Number(devices);
    const limit = gb.trim() ? Number(gb) : null;
    if (!Number.isInteger(days) || days < 1 || days > 30) {
      setError('Срок пробного периода: от 1 до 30 дней');
      return;
    }
    if (!Number.isInteger(dev) || dev < 1 || dev > 10) {
      setError('Устройств: от 1 до 10');
      return;
    }
    if (enabled && chosen.length === 0) {
      setError('Выберите хотя бы один сквад');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await api<Trial>('/api/admin/settings/trial', {
        method: 'PUT',
        body: { enabled, days, devices: dev, squads: chosen, traffic: { limit_bytes: limit === null ? null : limit * GIB, reset: 'no_reset' } },
      });
      onDone(next);
    } catch (err) {
      setError(failText(err, 'Настройки не сохранились'));
      setBusy(false);
    }
  };
  return (
    <Dialog title="Пробный период" busy={busy} onClose={onClose} wide>
      <span className="tf-switchrow">
        <Switch on={enabled} onToggle={() => setEnabled(!enabled)} label="Выдавать пробный период" />
        <span style={{ fontWeight: 600 }} aria-hidden="true">
          Выдавать пробный период новым клиентам
        </span>
      </span>
      <div className="tf-wrap">
        <label className="tf-field" htmlFor={`${ids}-d`}>
          <span>Срок</span>
          <span className="tf-inline">
            <input id={`${ids}-d`} className="field num" inputMode="numeric" value={daysRaw} onChange={(e) => setDays(e.target.value.replace(/\D/g, '').slice(0, 2))} style={{ width: 88 }} disabled={busy} />
            <span className="muted">{plural(Number(daysRaw) || 0, 'день', 'дня', 'дней')}</span>
          </span>
        </label>
        <label className="tf-field" htmlFor={`${ids}-v`}>
          <span>Устройств</span>
          <input id={`${ids}-v`} className="field num" inputMode="numeric" value={devices} onChange={(e) => setDevices(e.target.value.replace(/\D/g, '').slice(0, 2))} style={{ width: 88 }} disabled={busy} />
        </label>
        <label className="tf-field" htmlFor={`${ids}-g`}>
          <span>Трафик на весь срок</span>
          <span className="tf-inline">
            <input id={`${ids}-g`} className="field num" inputMode="numeric" placeholder="Без лимита" value={gb} onChange={(e) => setGb(e.target.value.replace(/\D/g, '').slice(0, 6))} style={{ width: 120 }} disabled={busy} />
            <span className="muted">ГБ</span>
          </span>
        </label>
      </div>
      <div className="field-row">
        <span style={{ fontWeight: 600 }}>Сквады</span>
        <div role="group" aria-label="Сквады пробного периода" className="chips">
          {squads.map((s) => {
            const on = chosen.includes(s.uuid);
            return (
              <button key={s.uuid} type="button" className="chip" aria-pressed={on} onClick={() => setChosen(on ? chosen.filter((x) => x !== s.uuid) : [...chosen, s.uuid])} disabled={busy}>
                <span>{s.name}</span>
              </button>
            );
          })}
          {squads.length === 0 ? <span className="muted">Remnawave не отдал сквады</span> : null}
        </div>
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}
