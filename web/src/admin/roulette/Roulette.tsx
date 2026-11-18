import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import { count, dateLong, days as daysText, groupDigits, money, timeHM } from '../../lib/format';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import {
  BASE,
  blankSector,
  buildBody,
  chances,
  disabledModule,
  draftOf,
  failText,
  FREE_FOR,
  KINDS,
  PERIODS,
  type AdminView,
  type Draft,
  type PrizeKind,
  type SectorDraft,
  type SpinItem,
} from './model';
import '../form.css';
import '../home/phone.css';
import '../promo/promo.css';
import './roulette.css';

type Page<T> = { items: T[]; next: number | null };

function personName(s: SpinItem): string {
  return s.client?.name ?? (s.client?.telegram_username ? `@${s.client.telegram_username}` : (s.client?.email ?? `Клиент #${s.client_id}`));
}

function spinState(s: SpinItem): { text: string; tone: string } {
  switch (s.state) {
    case 'issued':
      return { text: s.code ? `код ${s.code}` : (s.note ?? 'выдан'), tone: 'ok' };
    case 'failed':
      return { text: s.error ?? 'не выдан', tone: 'bad' };
    case 'issuing':
      return { text: 'выдаётся', tone: 'warn' };
    default:
      return { text: 'без приза', tone: 'muted' };
  }
}

function Block({ title, lead, phone, children }: { title: string; lead?: string; phone: boolean; children: ReactNode }) {
  return phone ? (
    <section className="hp-group">
      <h2 className="hp-h2">{title}</h2>
      <div className="hp-card rl-pcard">
        {lead ? <p className="cap rl-lead">{lead}</p> : null}
        {children}
      </div>
    </section>
  ) : (
    <section className="card rl-card">
      <h2 className="section-title rl-title">{title}</h2>
      {lead ? <p className="cap rl-lead">{lead}</p> : null}
      {children}
    </section>
  );
}

export function Roulette() {
  const phone = usePhone();
  const { can, me } = useSession();
  const allowed = can('marketing');
  const [view, setView] = useState<AdminView | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [spins, setSpins] = useState<SpinItem[] | null>(null);

  const load = useCallback(async () => {
    try {
      const next = await api<AdminView>(`${BASE}/settings`);
      setView(next);
      setDraft(draftOf(next));
      setError(null);
      setOff(false);
      const page = await api<Page<SpinItem>>(`${BASE}/spins?limit=20`);
      setSpins(page.items);
    } catch (err) {
      setOff(disabledModule(err));
      setError(failText(err, 'Рулетка не загрузилась'));
    }
  }, []);

  useEffect(() => {
    if (allowed) {
      void load();
    }
  }, [allowed, load]);

  if (!allowed) {
    return phone ? (
      <div className="hp">
        <PhoneHeader title="Рулетка" />
        <p className="hp-quiet hp-card">Раздел доступен администраторам с правом «Маркетинг».</p>
      </div>
    ) : (
      <>
        <h1 className="page-title">Рулетка</h1>
        <section className="card card-pad">
          <p className="muted">Раздел доступен администраторам с правом «Маркетинг».</p>
        </section>
      </>
    );
  }

  const currency = view?.currency ?? me.currency;
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
    setErrors((e) => ({ ...e, [key]: '' }));
    setNote(null);
  };
  const setSector = (key: string, patch: Partial<SectorDraft>) => {
    setDraft((d) => (d ? { ...d, sectors: d.sectors.map((s) => (s.key === key ? { ...s, ...patch } : s)) } : d));
    setErrors((e) => ({ ...e, [key]: '', sectors: '' }));
    setNote(null);
  };

  const save = async () => {
    if (!draft || !view) {
      return;
    }
    const built = buildBody(draft, currency, view.version);
    const found = Object.values(built.errors).some(Boolean);
    setErrors(built.errors);
    if (found) {
      setNote('Исправьте отмеченные поля');
      return;
    }
    setBusy(true);
    try {
      const next = await api<AdminView>(`${BASE}/settings`, { method: 'PUT', body: built.body });
      setView(next);
      setDraft(draftOf(next));
      setNote(view.saved ? 'Сохранено. Клиенты увидят новые призы при следующем открытии рулетки.' : 'Рулетка настроена — клиенты уже могут крутить.');
    } catch (err) {
      setNote(failText(err, 'Не сохранилось'));
    } finally {
      setBusy(false);
    }
  };

  const head = phone ? (
    <PhoneHeader title="Рулетка" />
  ) : (
    <div className="pr-head">
      <h1 className="page-title tight">Рулетка</h1>
    </div>
  );

  if (error && !view) {
    return (
      <div className={phone ? 'hp rl-phone' : 'pr-page rl-page'}>
        {head}
        <p className={phone ? 'hp-quiet hp-card' : 'card card-pad'} role="alert">
          <span className="err">{error}</span>
          {off ? (
            <>
              {' '}
              <Link to="/admin/modules" className="pr-inline-link">
                Включить модуль
              </Link>
            </>
          ) : null}
        </p>
      </div>
    );
  }
  if (!view || !draft) {
    return <p className="muted rl-loading">Загружаем рулетку…</p>;
  }

  const odds = chances(draft.sectors);
  const unit = currency === 'RUB' ? '₽' : '$';
  const noEmpty = !draft.sectors.some((s) => s.kind === 'empty' && Number(s.weight) > 0);
  const budgetSet = draft.budgetBalance.trim() || draft.budgetDays.trim() || draft.budgetPromos.trim();
  const t = view.tally;
  const within = { day: 'за сутки', week: 'за 7 дней', month: 'за 30 дней' }[view.config.budget.period];

  const sectorRow = (s: SectorDraft, i: number) => (
    <div key={s.key} className={phone ? 'rl-sector phone' : 'rl-sector'}>
      <span className="rl-n num">{i + 1}</span>
      <select className="field rl-kind" value={s.kind} onChange={(e) => setSector(s.key, { kind: e.target.value as PrizeKind })} aria-label={`Вид приза ${i + 1}`}>
        {KINDS.map(([key, label]) => (
          <option key={key} value={key}>
            {label}
          </option>
        ))}
      </select>
      <span className="rl-value">
        {s.kind === 'days' ? (
          <span className="pf-unit">
            <input className="field num" inputMode="numeric" value={s.days} onChange={(e) => setSector(s.key, { days: e.target.value })} aria-label={`Дней в призе ${i + 1}`} maxLength={3} />
            <span className="muted">дн.</span>
          </span>
        ) : s.kind === 'balance' ? (
          <span className="pf-unit">
            {currency === 'USD' ? <span className="muted">{unit}</span> : null}
            <input className="field num" inputMode={currency === 'USD' ? 'decimal' : 'numeric'} value={s.amount} onChange={(e) => setSector(s.key, { amount: e.target.value })} aria-label={`Сумма в призе ${i + 1}`} maxLength={9} />
            {currency === 'RUB' ? <span className="muted">{unit}</span> : null}
          </span>
        ) : s.kind === 'promo' ? (
          <span className="pf-unit rl-promo">
            <input className="field num" inputMode="numeric" value={s.percent} onChange={(e) => setSector(s.key, { percent: e.target.value })} aria-label={`Скидка в призе ${i + 1}`} maxLength={3} />
            <span className="muted">% на</span>
            <input className="field num" inputMode="numeric" value={s.validDays} onChange={(e) => setSector(s.key, { validDays: e.target.value })} aria-label={`Срок промокода в призе ${i + 1}`} maxLength={3} />
            <span className="muted">дн.</span>
          </span>
        ) : (
          <span className="muted rl-empty">Ничего не выдаётся</span>
        )}
      </span>
      <span className="pf-unit rl-weight">
        <input className="field num" inputMode="numeric" value={s.weight} onChange={(e) => setSector(s.key, { weight: e.target.value })} aria-label={`Вес приза ${i + 1}`} maxLength={5} />
      </span>
      <span className="rl-chance num">{odds[i] ?? 0}%</span>
      <input className="field rl-label" value={s.label} onChange={(e) => setSector(s.key, { label: e.target.value })} placeholder="Подпись" maxLength={24} aria-label={`Подпись приза ${i + 1}`} />
      <button
        type="button"
        className="rl-remove hov"
        disabled={draft.sectors.length <= 2}
        onClick={() => set('sectors', draft.sectors.filter((x) => x.key !== s.key))}
        aria-label={`Удалить приз ${i + 1}`}
      >
        Удалить
      </button>
      {errors[s.key] ? <span className="err rl-err">{errors[s.key]}</span> : null}
    </div>
  );

  return (
    <div className={phone ? 'hp rl-phone' : 'pr-page rl-page'}>
      {head}
      {view.saved ? null : (
        <p className={phone ? 'hp-quiet hp-card' : 'rl-intro'} role="note">
          Рулетка ещё не настроена: ниже пример призов. Поправьте призы и нажмите «Сохранить» — после этого клиенты смогут крутить в мини-приложении.
        </p>
      )}

      <Block
        phone={phone}
        title="Призы"
        lead="Призы стоят карточками в ленте. Чем больше вес, тем чаще выпадает приз. Результат решает сервер: клиент ничего не выбирает. Дни выпадают только тем, у кого есть подписка, промокод — только при включённом модуле «Промокоды»."
      >
        {phone ? null : (
          <div className="rl-sector rl-th" aria-hidden="true">
            <span>№</span>
            <span>Приз</span>
            <span>Сколько</span>
            <span>Вес</span>
            <span>Шанс</span>
            <span>Подпись на карточке</span>
            <span />
          </div>
        )}
        {draft.sectors.map(sectorRow)}
        {errors.sectors ? <p className="err rl-pad">{errors.sectors}</p> : null}
        {!view.promo_enabled && draft.sectors.some((s) => s.kind === 'promo') ? <p className="err rl-pad">Модуль «Промокоды» выключен — промокод в рулетке не сохранится.</p> : null}
        {noEmpty && budgetSet ? <p className="rl-pad rl-hint">Добавьте «Пусто»: когда бюджет закончится, лента будет останавливаться на нём, а не отказывать во вращении.</p> : null}
        {draft.sectors.length < 12 ? (
          <div className="rl-pad">
            <button type="button" className={phone ? 'pf-quiet press' : 'b40 hov'} onClick={() => set('sectors', [...draft.sectors, blankSector()])}>
              Добавить приз
            </button>
          </div>
        ) : null}
      </Block>

      <Block phone={phone} title="Когда можно крутить">
        <div className="rl-form">
          <label className="pf-check">
            <input type="checkbox" checked={draft.free} onChange={(e) => set('free', e.target.checked)} />
            <span>
              Бесплатное вращение
              <span className="cap">Одно вращение раз в несколько дней</span>
            </span>
          </label>
          {draft.free ? (
            <div className="rl-indent rl-row">
              <div className="pf-field">
                <span className="pf-label">Раз в</span>
                <span className="pf-unit">
                  <input className="field num rl-short" inputMode="numeric" value={draft.freeEvery} onChange={(e) => set('freeEvery', e.target.value)} maxLength={3} aria-label="Раз в сколько дней" />
                  <span className="muted">дней</span>
                </span>
              </div>
              <div className="pf-field">
                <span className="pf-label">Кому</span>
                <div role="group" aria-label="Кому бесплатное вращение" className={phone ? 'pf-psegs rl-segs' : 'segs'}>
                  {FREE_FOR.map(([key, label]) => (
                    <button key={key} type="button" aria-pressed={draft.freeFor === key} onClick={() => set('freeFor', key)}>
                      {label}
                    </button>
                  ))}
                </div>
                <span className="cap">Защита от накруток: новые пустые аккаунты не получат призы бесплатно</span>
              </div>
            </div>
          ) : null}
          {errors.freeEvery ? <span className="err">{errors.freeEvery}</span> : null}
          <label className="pf-check">
            <input type="checkbox" checked={draft.perPurchase} onChange={(e) => set('perPurchase', e.target.checked)} />
            <span>
              Вращение за покупку
              <span className="cap">Одно вращение за каждую оплату подписки, продления или опций. Если оплату вернут, неиспользованное вращение сгорает</span>
            </span>
          </label>
          {draft.perPurchase ? (
            <div className="rl-indent pf-field rl-min">
              <span className="pf-label">Покупка от</span>
              <span className="pf-unit">
                {currency === 'USD' ? <span className="muted">{unit}</span> : null}
                <input className="field num" inputMode={currency === 'USD' ? 'decimal' : 'numeric'} value={draft.purchaseMin} onChange={(e) => set('purchaseMin', e.target.value)} placeholder="Любая сумма" maxLength={9} aria-label="Минимальная сумма покупки" />
                {currency === 'RUB' ? <span className="muted">{unit}</span> : null}
              </span>
              {errors.purchaseMin ? <span className="err">{errors.purchaseMin}</span> : null}
            </div>
          ) : null}
        </div>
      </Block>

      <Block phone={phone} title="Бюджет призов" lead="Сколько можно раздать за период. Когда бюджет на приз закончился, он перестаёт выпадать до конца периода. Пусто — без ограничения.">
        <div className="rl-form">
          <div role="group" aria-label="Период бюджета" className={phone ? 'pf-psegs' : 'segs rl-period'}>
            {PERIODS.map(([key, label]) => (
              <button key={key} type="button" aria-pressed={draft.period === key} onClick={() => set('period', key)}>
                {label}
              </button>
            ))}
          </div>
          <div className={phone ? 'rl-budget phone' : 'rl-budget'}>
            <div className="pf-field">
              <span className="pf-label">На баланс</span>
              <span className="pf-unit">
                {currency === 'USD' ? <span className="muted">{unit}</span> : null}
                <input className="field num" inputMode={currency === 'USD' ? 'decimal' : 'numeric'} value={draft.budgetBalance} onChange={(e) => set('budgetBalance', e.target.value)} placeholder="Без ограничения" maxLength={9} aria-label="Бюджет на баланс" />
                {currency === 'RUB' ? <span className="muted">{unit}</span> : null}
              </span>
              <span className={errors.budgetBalance ? 'err' : 'cap'}>{errors.budgetBalance || `Выдано ${money(view.spent.balance)} ${within}`}</span>
            </div>
            <div className="pf-field">
              <span className="pf-label">Дней подписки</span>
              <input className="field num" inputMode="numeric" value={draft.budgetDays} onChange={(e) => set('budgetDays', e.target.value)} placeholder="Без ограничения" maxLength={6} aria-label="Бюджет дней" />
              <span className={errors.budgetDays ? 'err' : 'cap'}>{errors.budgetDays || `Выдано ${daysText(view.spent.days)} ${within}`}</span>
            </div>
            <div className="pf-field">
              <span className="pf-label">Промокодов</span>
              <input className="field num" inputMode="numeric" value={draft.budgetPromos} onChange={(e) => set('budgetPromos', e.target.value)} placeholder="Без ограничения" maxLength={6} aria-label="Бюджет промокодов" />
              <span className={errors.budgetPromos ? 'err' : 'cap'}>{errors.budgetPromos || `Выдано ${count(view.spent.promos, 'код', 'кода', 'кодов')} ${within}`}</span>
            </div>
          </div>
        </div>
      </Block>

      <div className={phone ? 'rl-save phone' : 'rl-save'}>
        <button type="button" className={phone ? 'np-ok press' : 'btn-main btn'} onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : view.saved ? 'Сохранить' : 'Сохранить и включить'}
        </button>
        {note ? (
          <span className={note.startsWith('Сохранено') || note.startsWith('Рулетка настроена') ? 'rl-note' : 'err'} role="status">
            {note}
          </span>
        ) : null}
      </div>

      <Block phone={phone} title={`Выдача ${within}`}>
        <dl className={phone ? 'rl-stats phone' : 'rl-stats'}>
          <div>
            <dt>Вращений</dt>
            <dd className="num">{groupDigits(t.spins)}</dd>
            <span className="cap">
              бесплатных {t.free}, за покупки {t.purchase}
            </span>
          </div>
          <div>
            <dt>На баланс</dt>
            <dd className="num">{money({ minor: t.balance_minor, currency })}</dd>
            <span className="cap">{count(t.balance_count, 'приз', 'приза', 'призов')}</span>
          </div>
          <div>
            <dt>Дней подписки</dt>
            <dd className="num">{groupDigits(t.days)}</dd>
            <span className="cap">{count(t.days_count, 'приз', 'приза', 'призов')}</span>
          </div>
          <div>
            <dt>Промокодов</dt>
            <dd className="num">{groupDigits(t.promo)}</dd>
            <span className="cap">пусто выпало {t.empty}</span>
          </div>
        </dl>
        <p className="cap rl-pad">
          Крутили {count(t.clients, 'клиент', 'клиента', 'клиентов')}. Неиспользованных вращений за покупки: {view.tickets_open}.
          {t.failed ? ` Не выдано из-за сбоя: ${t.failed} — дни подписки, проверьте подписку клиента.` : ''}
        </p>
      </Block>

      <Block phone={phone} title="Последние вращения">
        {spins === null ? <p className="muted rl-pad">Загружаем…</p> : null}
        {spins !== null && spins.length === 0 ? <p className="muted rl-pad">Пока никто не крутил</p> : null}
        {spins && spins.length ? (
          <div className="rl-spins">
            {spins.map((s) => {
              const state = spinState(s);
              return (
                <div key={s.id} className={phone ? 'rl-spin phone' : 'rl-spin'}>
                  <Link to={`/admin/clients/${s.client_id}`} className="rl-who hov">
                    {personName(s)}
                  </Link>
                  <span className="rl-what">
                    <b>{s.title}</b>
                    <span className={`rl-st ${state.tone}`}>{state.text}</span>
                  </span>
                  <span className="rl-when">
                    {s.source === 'free' ? 'бесплатно' : 'за покупку'} · {dateLong(s.at)}, {timeHM(s.at)}
                  </span>
                </div>
              );
            })}
          </div>
        ) : null}
      </Block>
    </div>
  );
}
