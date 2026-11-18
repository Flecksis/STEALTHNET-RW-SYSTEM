import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { count, dateLong, days, money, plural, timeHM, type Money } from '../lib/format';
import { Icon } from '../lib/icons';
import { BackHead, useMini } from './MiniApp';

type Prize =
  | { kind: 'days'; days: number }
  | { kind: 'balance'; amount: Money }
  | { kind: 'promo'; percent: number; valid_days: number }
  | { kind: 'empty' };

type Sector = { index: number; prize: Prize; kind: string; title: string; custom: boolean; weight: number; chance: number };

type HistoryItem = { id: number; sector: number; title: string; prize: Prize; state: string; note: string | null; code: string | null; at: string };

type RouletteState = {
  ready: boolean;
  version: number;
  sectors: Sector[];
  spins: { free_ready: boolean; free_next_at: string | null; free_blocked: string | null; purchase: number; total: number };
  free_every_days: number | null;
  free_for: 'all' | 'subscribers' | 'paid';
  per_purchase: boolean;
  purchase_min: Money | null;
  history: HistoryItem[];
};

type SpinResult = { spin: HistoryItem; state: RouletteState };

const TILE = 96;
const GAP = 10;
const EDGE = 10;
const COPIES = 10;
const SPIN_MS = 3600;

function tile(s: Sector): { big: string; small: string; empty: boolean; long: boolean } {
  if (s.custom) {
    return { big: s.title, small: '', empty: s.prize.kind === 'empty', long: s.title.length > 7 };
  }
  switch (s.prize.kind) {
    case 'days':
      return { big: `+${s.prize.days}`, small: plural(s.prize.days, 'день', 'дня', 'дней'), empty: false, long: false };
    case 'balance': {
      const text = money(s.prize.amount, true);
      return { big: text, small: 'на баланс', empty: false, long: text.length > 6 };
    }
    case 'promo':
      return { big: `−${s.prize.percent}%`, small: 'скидка', empty: false, long: false };
    default:
      return { big: 'Пусто', small: '', empty: true, long: false };
  }
}

function left(iso: string): string {
  const ms = new Date(iso).getTime() - Date.now();
  if (ms <= 0) {
    return 'совсем скоро';
  }
  const hours = Math.floor(ms / 3_600_000);
  const whole = Math.floor(hours / 24);
  if (whole >= 1) {
    const rest = hours % 24;
    return `через ${count(whole, 'день', 'дня', 'дней')}${rest ? ` ${count(rest, 'час', 'часа', 'часов')}` : ''}`;
  }
  if (hours >= 1) {
    return `через ${count(hours, 'час', 'часа', 'часов')}`;
  }
  return `через ${count(Math.max(1, Math.floor(ms / 60_000)), 'минуту', 'минуты', 'минут')}`;
}

function spinsLine(state: RouletteState): { head: string; sub: string } {
  const s = state.spins;
  if (s.total > 0) {
    const parts: string[] = [];
    if (s.free_ready) {
      parts.push('1 бесплатное');
    }
    if (s.purchase) {
      parts.push(`${s.purchase} за ${plural(s.purchase, 'покупку', 'покупки', 'покупки')}`);
    }
    return { head: plural(s.total, 'вращение есть', 'вращения есть', 'вращений есть'), sub: parts.join(' и ') };
  }
  if (s.free_next_at) {
    return { head: 'вращений нет', sub: `Бесплатное — ${left(s.free_next_at)}` };
  }
  if (s.free_blocked) {
    return { head: 'вращений нет', sub: s.free_blocked };
  }
  return { head: 'вращений нет', sub: state.per_purchase ? 'Вращение даётся за каждую покупку' : 'Загляните позже' };
}

function rules(state: RouletteState): string[] {
  const out: string[] = [];
  if (state.free_every_days) {
    const who = state.free_for === 'paid' ? ', клиентам с оплаченной подпиской' : state.free_for === 'subscribers' ? ', клиентам с подпиской' : '';
    out.push(`Бесплатно — раз в ${days(state.free_every_days)}${who}.`);
  }
  if (state.per_purchase) {
    out.push(`За каждую оплату${state.purchase_min ? ` от ${money(state.purchase_min)}` : ''} — ещё одно вращение.`);
  }
  const chances = new Map<string, number>();
  for (const s of state.sectors) {
    if (s.chance <= 0) {
      continue;
    }
    const t = tile(s);
    const name = t.empty ? 'пусто' : `${t.big}${t.small ? ` ${t.small}` : ''}`;
    chances.set(name, Math.round(((chances.get(name) ?? 0) + s.chance) * 10) / 10);
  }
  if (chances.size) {
    out.push(`Шансы: ${[...chances.entries()].map(([name, chance]) => `${name} — ${chance}%`).join(', ')}.`);
  }
  return out;
}

function errorText(err: unknown): string {
  if (err instanceof ApiError) {
    return err.code === 'module.disabled' ? 'Рулетка сейчас выключена' : err.message;
  }
  return 'Сервер не ответил';
}

function reduced(): boolean {
  try {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  } catch {
    return false;
  }
}

function requestId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  const hex = Array.from({ length: 32 }, () => Math.floor(Math.random() * 16).toString(16)).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-4${hex.slice(13, 16)}-a${hex.slice(17, 20)}-${hex.slice(20, 32)}`;
}

function Reel({ sectors, at, lit, moving, fast }: { sectors: Sector[]; at: number; lit: number | null; moving: boolean; fast: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(343);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) {
      return;
    }
    const measure = () => setWidth(el.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const n = Math.max(sectors.length, 1);
  const shift = width / 2 - (EDGE + at * (TILE + GAP) + TILE / 2);
  const tiles = Array.from({ length: n * COPIES }, (_, i) => i);
  return (
    <div ref={box} className="mr-reel" aria-hidden="true">
      <span className="mr-mark top" />
      <span className="mr-mark bottom" />
      <div className={moving ? (fast ? 'mr-strip fast' : 'mr-strip moving') : 'mr-strip'} style={{ transform: `translateX(${shift}px)` }}>
        {tiles.map((i) => {
          const s = sectors[i % n];
          if (!s) {
            return null;
          }
          const t = tile(s);
          const cls = ['mr-tile', t.empty ? 'empty' : '', t.long ? 'long' : '', lit === i ? 'lit' : ''].filter(Boolean).join(' ');
          return (
            <span key={i} className={cls}>
              <b>{t.big}</b>
              {t.small ? <span>{t.small}</span> : null}
            </span>
          );
        })}
      </div>
    </div>
  );
}

export function MiniRoulette() {
  const { view, reload } = useMini();
  const [state, setState] = useState<RouletteState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [at, setAt] = useState(0);
  const [moving, setMoving] = useState(false);
  const [busy, setBusy] = useState(false);
  const [lit, setLit] = useState<number | null>(null);
  const [won, setWon] = useState<HistoryItem | null>(null);
  const [fast] = useState(reduced);
  const timer = useRef(0);

  useEffect(() => {
    let alive = true;
    api<RouletteState>('/api/client/m/roulette/state')
      .then((next) => {
        if (alive) {
          setState(next);
          setAt(Math.max(next.sectors.length, 1) * 2);
        }
      })
      .catch((err) => alive && setError(errorText(err)));
    return () => {
      alive = false;
      window.clearTimeout(timer.current);
    };
  }, []);

  const land = useCallback(
    (result: SpinResult, target: number) => {
      setMoving(false);
      setBusy(false);
      setLit(target);
      setWon(result.spin);
      setState(result.state);
      if (result.spin.prize.kind === 'balance' || result.spin.prize.kind === 'days') {
        void reload();
      }
    },
    [reload],
  );

  const spin = async () => {
    if (!state || busy) {
      return;
    }
    setError(null);
    setWon(null);
    setLit(null);
    setBusy(true);
    try {
      const result = await api<SpinResult>('/api/client/m/roulette/spin', {
        method: 'POST',
        body: { request_id: requestId(), subscription_id: view?.subscription.id ?? null },
      });
      const sectors = result.state.sectors.length ? result.state.sectors : state.sectors;
      const n = Math.max(sectors.length, 1);
      setState((s) => (s ? { ...s, sectors } : s));
      setMoving(false);
      setAt((current) => n * 2 + (((current % n) + n) % n));
      const target = n * (COPIES - 3) + result.spin.sector;
      window.requestAnimationFrame(() =>
        window.requestAnimationFrame(() => {
          setMoving(true);
          setAt(target);
        }),
      );
      timer.current = window.setTimeout(() => land(result, target), fast ? 700 : SPIN_MS + 150);
    } catch (err) {
      setBusy(false);
      setError(errorText(err));
      try {
        setState(await api<RouletteState>('/api/client/m/roulette/state'));
      } catch {
        return;
      }
    }
  };

  if (error && !state) {
    return (
      <main className="mn-page mr-page">
        <BackHead title="Рулетка" />
        <p className="mn-note">{error}</p>
      </main>
    );
  }
  if (!state) {
    return (
      <main className="mn-page mr-page" aria-busy="true">
        <BackHead title="Рулетка" />
      </main>
    );
  }
  if (!state.ready) {
    return (
      <main className="mn-page mr-page">
        <BackHead title="Рулетка" />
        <p className="mn-note">Рулетка скоро заработает — загляните позже.</p>
      </main>
    );
  }

  const line = spinsLine(state);
  const total = state.spins.total;

  return (
    <main className="mn-page mr-page">
      <BackHead title="Рулетка" />

      <section className="mr-spins" aria-live="polite">
        <span className="mr-count num">{total}</span>
        <span className="mr-spins-text">
          <b>{line.head}</b>
          {line.sub ? <span>{line.sub}</span> : null}
        </span>
      </section>

      <Reel sectors={state.sectors} at={at} lit={busy ? null : lit} moving={moving} fast={fast} />

      {won ? (
        <section className={won.prize.kind === 'empty' ? 'mr-won empty' : 'mr-won'} aria-live="polite">
          <Icon name="gift" size={24} />
          <span className="mr-won-text">
            <b>{won.prize.kind === 'empty' ? 'В этот раз пусто' : won.title}</b>
            {won.prize.kind === 'empty' ? <span>Повезёт в следующий раз</span> : null}
            {won.code ? (
              <span>
                Промокод <b className="mono mr-code">{won.code}</b> — введите при оплате
              </span>
            ) : won.note ? (
              <span>{won.note}</span>
            ) : null}
            {won.state === 'failed' ? <span className="mn-error">Приз не выдался — напишите в поддержку, мы проверим</span> : null}
          </span>
        </section>
      ) : null}

      {error ? <p className="mn-error">{error}</p> : null}

      <button type="button" className="mn-main press" disabled={busy || total < 1} onClick={() => void spin()}>
        {busy ? 'Крутится…' : won && total > 0 ? 'Крутить ещё' : 'Крутить'}
      </button>

      <div className="mr-rules">
        {rules(state).map((r) => (
          <span key={r}>{r}</span>
        ))}
      </div>

      {state.history.length ? (
        <>
          <p className="mr-title">История</p>
          <div className="mn-list">
            {state.history.map((h) => (
              <div key={h.id} className="mn-row mr-row">
                <span className="mn-row-name">
                  {h.prize.kind === 'empty' ? 'Пусто' : h.title}
                  {h.code ? <span className="mr-hcode mono">{h.code}</span> : null}
                </span>
                <span className="mn-row-val">
                  {dateLong(h.at)}, {timeHM(h.at)}
                </span>
              </div>
            ))}
          </div>
        </>
      ) : null}
    </main>
  );
}
