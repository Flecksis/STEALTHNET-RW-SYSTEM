import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, dateTime, groupDigits } from '../../lib/format';
import '../form.css';
import './import.css';
import './remna-sync.css';
import { SettingsLayout } from './SettingsLayout';

export const SYNC_PATH = '/admin/settings/remnawave/sync';

type Direction = 'push' | 'pull';
type Mode = 'create' | 'fix';
type State = 'checking' | 'ready' | 'running' | 'paused' | 'done' | 'failed' | 'interrupted' | 'discarded';

type Example = { who: string; details: string[] };
type Bucket = { count: number; examples: Example[] };
type Reason = { code: string; text: string; count: number; examples: string[] };
type RemnaSquad = { uuid: string; name: string };
type SquadPlan = { old: string; name: string | null; subscriptions: number; tariffs: string[]; suggested: string | null };

type Plan = {
  direction: Direction;
  remna_users: number;
  scanned: number;
  sub_domain: string | null;
  create: Bucket;
  fix: Bucket;
  fields: Record<string, number>;
  same: number;
  relink: Bucket;
  attach: Bucket;
  matched_by: Record<string, number>;
  skipped: Reason[];
  link_change: Bucket;
  keys_kept: number;
  keys_new: number;
  later_in_remna: number;
  expired: number;
  disabled: number;
  tariff_matched: number;
  tariff_none: number;
  several: number;
  no_contact: number;
  squads: SquadPlan[];
  remna_squads: RemnaSquad[];
  external_dropped: number;
  secrets: boolean;
  snapshots: number;
  warnings: string[];
};

type Progress = {
  total: number;
  done: number;
  created: number;
  fixed: number;
  relinked: number;
  attached: number;
  clients: number;
  same: number;
  differs: number;
  skipped: number;
  failed: number;
  link_changed: number;
  tariffs: number;
};

type Run = {
  id: string;
  direction: Direction;
  state: State;
  mode: Mode | null;
  plan: Plan | null;
  progress: Progress | null;
  outcome: Progress | null;
  error: string | null;
  pause_requested: boolean;
  notes: number;
  link_changed: number;
  notified_at: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
};

type SyncView = { remna: boolean; secrets: boolean; run: Run | null; history: Run[] };

const ACTIVE: State[] = ['checking', 'running'];
const DROP = '';

const DIRECTIONS: Array<{ id: Direction; name: string; what: string }> = [
  {
    id: 'push',
    name: 'Перенести в Remnawave',
    what: 'Создать в Remnawave пользователей, которых там нет, с прежними ссылками. Например, после переезда на новую Remnawave или если она потеряла данные.',
  },
  {
    id: 'pull',
    name: 'Забрать из Remnawave',
    what: 'Добавить в панель пользователей Remnawave, которых в ней нет. Например, если панель восстановили из старой копии.',
  },
];

const BY_LABEL: Record<string, string> = {
  telegram: 'по Telegram ID',
  email: 'по почте',
  name: 'по имени в Telegram',
  marker: 'по отметке панели',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function title(direction: Direction): string {
  return direction === 'push' ? 'Перенос в Remnawave' : 'Забор из Remnawave';
}

function stateLine(run: Run): string {
  switch (run.state) {
    case 'done':
      return 'закончен';
    case 'paused':
      return 'остановлен';
    case 'interrupted':
      return 'прервался при перезапуске';
    case 'failed':
      return 'остановился с ошибкой';
    case 'running':
      return 'идёт';
    default:
      return 'проверка';
  }
}

function outcomeLine(direction: Direction, p: Progress): string {
  const parts: string[] = [];
  if (direction === 'push') {
    parts.push(`создано ${groupDigits(p.created)}`);
    if (p.fixed) parts.push(`исправлено ${groupDigits(p.fixed)}`);
  } else {
    parts.push(`новых подписок ${groupDigits(p.attached)}`);
    if (p.clients) parts.push(`новых клиентов ${groupDigits(p.clients)}`);
  }
  if (p.relinked) parts.push(`привязано заново ${groupDigits(p.relinked)}`);
  if (p.skipped) parts.push(`пропущено ${groupDigits(p.skipped)}`);
  if (p.failed) parts.push(`ошибок ${groupDigits(p.failed)}`);
  return parts.join(', ');
}

function keysLine(plan: Plan): string {
  if (plan.keys_kept === 0) {
    return 'Ключей протоколов панель ещё не запомнила, Remnawave выпустит новые. Приложения получат их, когда обновят подписку.';
  }
  if (plan.keys_new === 0) {
    return 'Ключи протоколов тоже прежние.';
  }
  return `Ключи протоколов прежние у ${groupDigits(plan.keys_kept)}, у ${groupDigits(plan.keys_new)} будут новые: приложения получат их, когда обновят подписку.`;
}

function tariffLine(plan: Plan): string {
  const why = 'совпали сквады, лимит и сброс трафика, устройства';
  if (plan.tariff_matched === 0) {
    return 'Ни один тариф панели не подошёл по сквадам и лимитам: подписки добавятся без тарифа, они отмечены в отчёте.';
  }
  if (plan.tariff_none === 0) {
    return `Тариф подобран у всех: ${why}.`;
  }
  return `Тариф подобран у ${groupDigits(plan.tariff_matched)}: ${why}. Без тарифа — ${groupDigits(plan.tariff_none)}, они отмечены в отчёте.`;
}

function Warnings({ list }: { list: string[] }) {
  if (!list.length) {
    return null;
  }
  return (
    <div className="imp-block" role="alert">
      <span className="strong">Проверьте до запуска</span>
      {list.map((warning) => (
        <span key={warning}>{warning}</span>
      ))}
    </div>
  );
}

function Examples({ label, bucket }: { label: string; bucket: Bucket }) {
  if (!bucket.examples.length) {
    return null;
  }
  return (
    <details className="rs-examples">
      <summary>
        {label}
        {bucket.count > bucket.examples.length ? `: первые ${bucket.examples.length} из ${groupDigits(bucket.count)}` : ''}
      </summary>
      <ul>
        {bucket.examples.map((example, i) => (
          <li key={`${example.who}-${i}`}>
            <span className="strong">{example.who}</span>
            {example.details.map((line, j) => (
              <span key={j} className="rs-line">
                {line}
              </span>
            ))}
          </li>
        ))}
      </ul>
    </details>
  );
}

function Counts({ items }: { items: Array<[string, number]> }) {
  return (
    <dl className="imp-counts">
      {items.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd>{groupDigits(value)}</dd>
        </div>
      ))}
    </dl>
  );
}

function Skipped({ plan, run }: { plan: Plan; run: Run }) {
  if (!plan.skipped.length) {
    return null;
  }
  return (
    <div className="imp-skips">
      <h3 className="strong">Что пропустим</h3>
      <ul>
        {plan.skipped.map((reason) => (
          <li key={reason.code}>
            <span className="num">{groupDigits(reason.count)}</span> — {reason.text}
            {reason.examples.length ? (
              <span className="rs-line muted">{reason.examples.slice(0, 2).join('; ')}</span>
            ) : null}
          </li>
        ))}
      </ul>
      <a className="imp-link" href={`/api/admin/remna-sync/${run.id}/report`} download>
        Скачать список
      </a>
    </div>
  );
}

type Choice = { mode: Mode; squads: Record<string, string>; tariffs: boolean };

function SquadMap({ plan, choice, onChange, busy }: { plan: Plan; choice: Choice; onChange: (c: Choice) => void; busy: boolean }) {
  if (!plan.squads.length) {
    return null;
  }
  return (
    <div className="imp-block">
      <span className="strong">Сквадов из панели нет в этой Remnawave</span>
      <span>
        У новой Remnawave свои сквады. Выберите, каким заменить каждый старый. Совпавшие по имени уже подставлены. «Не добавлять» — пользователи
        создадутся без этого сквада.
      </span>
      <div className="rs-squads">
        {plan.squads.map((squad) => {
          const value = choice.squads[squad.old] ?? squad.suggested ?? DROP;
          return (
            <label key={squad.old} className="rs-squad">
              <span className="rs-squad-name">
                <span className="strong">{squad.name ? `«${squad.name}»` : 'Сквад без имени'}</span>
                <span className="cap">
                  {count(squad.subscriptions, 'подписка', 'подписки', 'подписок')}
                  {squad.tariffs.length ? ` · тарифы: ${squad.tariffs.join(', ')}` : ''}
                </span>
              </span>
              <select
                className="field"
                value={value}
                disabled={busy}
                onChange={(e) => onChange({ ...choice, squads: { ...choice.squads, [squad.old]: e.target.value } })}
              >
                <option value={DROP}>Не добавлять</option>
                {plan.remna_squads.map((target) => (
                  <option key={target.uuid} value={target.uuid}>
                    {target.name}
                  </option>
                ))}
              </select>
            </label>
          );
        })}
      </div>
      <label className="imp-choice">
        <input type="checkbox" checked={choice.tariffs} disabled={busy} onChange={(e) => onChange({ ...choice, tariffs: e.target.checked })} />
        Заменить эти сквады и в тарифах
      </label>
    </div>
  );
}

function PushPlan({ run, plan, choice, onChange, busy }: { run: Run; plan: Plan; choice: Choice; onChange: (c: Choice) => void; busy: boolean }) {
  const skipped = plan.skipped.reduce((sum, r) => sum + r.count, 0);
  return (
    <div className="imp-check">
      <Counts
        items={[
          ['Создать', plan.create.count],
          ['Расходятся', plan.fix.count],
          ['Совпадают', plan.same],
          ['Пропустим', skipped],
        ]}
      />
      <ul className="imp-facts">
        <li>
          В Remnawave сейчас {count(plan.remna_users, 'пользователь', 'пользователя', 'пользователей')}
          {plan.sub_domain ? `, адрес подписок — ${plan.sub_domain}` : ''}.
        </li>
        {plan.create.count > 0 ? (
          <li>
            Ссылки сохранятся: у новых пользователей тот же код ссылки, что в панели. {keysLine(plan)}
          </li>
        ) : null}
        {plan.expired > 0 ? (
          <li>Срок уже истёк: {groupDigits(plan.expired)}. Такие тоже создаём, чтобы клиент мог продлить подписку по прежней ссылке.</li>
        ) : null}
        {plan.disabled > 0 ? <li>Отключённых в панели: {groupDigits(plan.disabled)} — создадим отключёнными.</li> : null}
        {plan.relink.count > 0 ? (
          <li>{count(plan.relink.count, 'подписку', 'подписки', 'подписок')} привяжем заново: пользователь в Remnawave есть, но под другим номером.</li>
        ) : null}
        {plan.later_in_remna > 0 ? (
          <li>У {count(plan.later_in_remna, 'подписки', 'подписок', 'подписок')} срок в Remnawave позже, чем в панели. Его не сокращаем.</li>
        ) : null}
        {plan.external_dropped > 0 ? (
          <li className="imp-warn-line">
            Внешнего сквада нет в этой Remnawave у {count(plan.external_dropped, 'подписки', 'подписок', 'подписок')} — создадим без него.
          </li>
        ) : null}
        <li className="muted">Номер пользователя в Remnawave задать нельзя: панель сама запомнит новый. На ссылку это не влияет.</li>
      </ul>
      <Warnings list={plan.warnings} />
      {plan.link_change.count > 0 ? (
        <div className="imp-block">
          <span className="strong">У {count(plan.link_change.count, 'клиента', 'клиентов', 'клиентов')} ссылка сменится</span>
          <span>В панели нет кода их ссылки, Remnawave выдаст новую. После переноса здесь появится кнопка, чтобы разослать им новую ссылку в бот.</span>
        </div>
      ) : null}
      <SquadMap plan={plan} choice={choice} onChange={onChange} busy={busy} />
      <Examples label="Кого создадим" bucket={plan.create} />
      {plan.fix.count > 0 ? (
        <div className="imp-block">
          <span className="strong">Данные расходятся у {count(plan.fix.count, 'подписки', 'подписок', 'подписок')}</span>
          <span>{Object.entries(plan.fields).map(([field, n]) => `${field.toLowerCase()} — ${groupDigits(n)}`).join(', ')}.</span>
          <Examples label="Что отличается" bucket={plan.fix} />
          <div className="rs-modes" role="radiogroup" aria-label="Что делать с расхождениями">
            <label className="imp-choice">
              <input type="radio" name="rs-mode" checked={choice.mode === 'create'} disabled={busy} onChange={() => onChange({ ...choice, mode: 'create' })} />
              Только создать недостающих
            </label>
            <label className="imp-choice">
              <input type="radio" name="rs-mode" checked={choice.mode === 'fix'} disabled={busy} onChange={() => onChange({ ...choice, mode: 'fix' })} />
              Ещё и поправить расхождения
            </label>
          </div>
          {choice.mode === 'fix' ? (
            <span className="cap">
              В Remnawave запишем данные из панели. Срок только продлеваем. Если отличается ссылка, вернём ссылку из панели, а ключи протоколов
              Remnawave выпустит новые.
            </span>
          ) : null}
        </div>
      ) : null}
      <Skipped plan={plan} run={run} />
    </div>
  );
}

function PullPlan({ run, plan }: { run: Run; plan: Plan }) {
  const skipped = plan.skipped.reduce((sum, r) => sum + r.count, 0);
  const by = Object.entries(plan.matched_by).filter(([, n]) => n > 0);
  return (
    <div className="imp-check">
      <Counts
        items={[
          ['Новые клиенты', plan.create.count],
          ['К найденным клиентам', plan.attach.count],
          ['Уже в панели', plan.same],
          ['Пропустим', skipped],
        ]}
      />
      <ul className="imp-facts">
        <li>В Remnawave {count(plan.remna_users, 'пользователь', 'пользователя', 'пользователей')}.</li>
        {by.length ? <li>Клиенты найдены {by.map(([key, n]) => `${BY_LABEL[key] ?? key} — ${groupDigits(n)}`).join(', ')}.</li> : null}
        {plan.relink.count > 0 ? (
          <li>{count(plan.relink.count, 'подписку', 'подписки', 'подписок')} привяжем заново: ссылка та же, а номер пользователя в Remnawave другой.</li>
        ) : null}
        {plan.create.count + plan.attach.count > 0 ? <li>{tariffLine(plan)}</li> : null}
        {plan.several > 0 ? <li>У {count(plan.several, 'клиента', 'клиентов', 'клиентов')} станет несколько подписок.</li> : null}
        <li className="muted">Ссылки клиентов не меняются: подписки привязываются к тем же пользователям Remnawave. В саму Remnawave ничего не пишем.</li>
      </ul>
      <Warnings list={plan.warnings} />
      <Examples label="Кого добавим новыми клиентами" bucket={plan.create} />
      <Examples label="Кому добавим подписку" bucket={plan.attach} />
      <Examples label="Что привяжем заново" bucket={plan.relink} />
      <Skipped plan={plan} run={run} />
    </div>
  );
}

function Running({ run, onPause, busy }: { run: Run; onPause: () => void; busy: boolean }) {
  const p = run.progress;
  const pct = p && p.total > 0 ? Math.min(100, Math.floor((p.done / p.total) * 100)) : 0;
  return (
    <div className="imp-run">
      <div className="imp-bar" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label="Ход переноса">
        <span style={{ width: `${pct}%` }} />
      </div>
      <span>
        {p && p.total > 0 ? `Просмотрено ${groupDigits(p.done)} из ${groupDigits(p.total)}` : 'Читаем пользователей Remnawave'}
        {p ? ` · ${outcomeLine(run.direction, p)}` : ''}
      </span>
      <p className="cap">Перенос идёт на сервере пачками, с паузами. Страницу можно закрыть. Остановить можно в любой момент и потом продолжить — дублей не будет.</p>
      <div className="imp-actions-inline">
        <button type="button" className="btn-line hov" onClick={onPause} disabled={busy || run.pause_requested}>
          {run.pause_requested ? 'Останавливаем после текущей пачки…' : 'Остановить'}
        </button>
      </div>
    </div>
  );
}

function Done({ run, onNotify, busy }: { run: Run; onNotify: () => void; busy: boolean }) {
  const o = run.outcome ?? run.progress;
  if (!o) {
    return null;
  }
  const items: Array<[string, number]> =
    run.direction === 'push'
      ? [
          ['Создано', o.created],
          ['Исправлено', o.fixed],
          ['Без изменений', o.same],
          ['Пропущено', o.skipped + o.failed],
        ]
      : [
          ['Новых подписок', o.attached],
          ['Новых клиентов', o.clients],
          ['Уже были', o.same],
          ['Пропущено', o.skipped + o.failed],
        ];
  return (
    <div className="imp-check">
      <Counts items={items} />
      <ul className="imp-facts">
        {o.relinked > 0 ? <li>{count(o.relinked, 'подписка привязана', 'подписки привязаны', 'подписок привязаны')} заново.</li> : null}
        {run.direction === 'push' && o.differs > 0 ? (
          <li>С расхождениями оставлено как есть: {groupDigits(o.differs)}. Чтобы поправить, запустите перенос с выбором «Ещё и поправить расхождения».</li>
        ) : null}
        {o.tariffs > 0 ? <li>В тарифах заменены сквады: {groupDigits(o.tariffs)}.</li> : null}
        {o.failed > 0 ? <li className="imp-warn-line">Не получилось: {groupDigits(o.failed)}. Причины — в отчёте.</li> : null}
        <li>
          {run.notes > 0 ? (
            <a className="imp-link" href={`/api/admin/remna-sync/${run.id}/report`} download>
              Скачать отчёт
            </a>
          ) : (
            'Всё прошло без замечаний.'
          )}
        </li>
      </ul>
      {run.link_changed > 0 ? (
        <div className="imp-block">
          <span className="strong">У {count(run.link_changed, 'клиента', 'клиентов', 'клиентов')} новая ссылка подписки</span>
          {run.notified_at ? (
            <span>Новые ссылки отправлены в бот {dateTime(run.notified_at).toLowerCase()}.</span>
          ) : (
            <>
              <span>Старая ссылка у них больше не работает. Отправьте каждому новую в бот — сообщение с кнопкой «Подписка».</span>
              <button type="button" className="btn-main btn" onClick={onNotify} disabled={busy}>
                Отправить новую ссылку
              </button>
            </>
          )}
        </div>
      ) : null}
    </div>
  );
}

function Problem({ run }: { run: Run }) {
  if (run.state === 'paused') {
    return (
      <div className="imp-block">
        <span className="strong">Перенос остановлен</span>
        <span>Уже сделанное сохранено. Нажмите «Продолжить» — повторов не будет.</span>
      </div>
    );
  }
  if (run.state === 'interrupted' || run.state === 'failed') {
    return (
      <div className="imp-block" role="alert">
        <span className="strong">{run.state === 'interrupted' ? 'Перенос прервался' : 'Остановился с ошибкой'}</span>
        <span>{run.error ?? 'Сервер перезапускался во время переноса. Уже сделанное сохранено — продолжите, дублей не будет.'}</span>
      </div>
    );
  }
  return null;
}

function Pick({ value, onPick, disabled }: { value: Direction; onPick: (d: Direction) => void; disabled: boolean }) {
  return (
    <div className="imp-sources" role="radiogroup" aria-label="Направление">
      {DIRECTIONS.map((d) => (
        <button
          key={d.id}
          type="button"
          role="radio"
          aria-checked={value === d.id}
          className="imp-source hov"
          onClick={() => onPick(d.id)}
          disabled={disabled}
        >
          <span className="imp-radio" aria-hidden="true" />
          <span className="imp-source-text">
            <span className="strong">{d.name}</span>
            <span className="cap">{d.what}</span>
          </span>
        </button>
      ))}
    </div>
  );
}

function History({ items }: { items: Run[] }) {
  if (!items.length) {
    return null;
  }
  return (
    <section className="sec card-pad" aria-labelledby="rs-history-h">
      <h2 id="rs-history-h" className="section-title">
        Прошлые запуски
      </h2>
      <ul className="rs-history">
        {items.map((run) => {
          const p = run.outcome ?? run.progress;
          return (
            <li key={run.id}>
              <span className="strong">
                {title(run.direction)} {stateLine(run)}
              </span>
              <span className="muted">
                {dateTime(run.finished_at ?? run.started_at ?? run.created_at)}
                {p ? ` · ${outcomeLine(run.direction, p)}` : ''}
              </span>
              {run.notes > 0 ? (
                <a className="imp-link" href={`/api/admin/remna-sync/${run.id}/report`} download>
                  Отчёт
                </a>
              ) : null}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function freshChoice(): Choice {
  return { mode: 'create', squads: {}, tariffs: true };
}

export function SettingsRemnaSync() {
  const [view, setView] = useState<SyncView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [direction, setDirection] = useState<Direction>('push');
  const [choice, setChoice] = useState<Choice>(freshChoice);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [anew, setAnew] = useState(false);

  const load = useCallback(async () => {
    try {
      setView(await api<SyncView>('/api/admin/remna-sync'));
      setLoadError(null);
    } catch (err) {
      setLoadError(failText(err, 'Раздел не загрузился'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const state = view?.run?.state;
  useEffect(() => {
    if (!state || !ACTIVE.includes(state)) {
      return;
    }
    const timer = window.setInterval(() => void load(), 1500);
    return () => window.clearInterval(timer);
  }, [state, load]);

  const act = async (work: () => Promise<unknown>, fallback: string) => {
    setBusy(true);
    setError(null);
    try {
      await work();
      await load();
    } catch (err) {
      setError(failText(err, fallback));
    } finally {
      setBusy(false);
    }
  };

  const run = view?.run && !(anew && view.run.state === 'done') ? view.run : null;
  const lost = Boolean(run && (run.state === 'failed' || run.state === 'interrupted') && !run.plan);

  const check = (dir: Direction) =>
    act(async () => {
      setChoice(freshChoice());
      setAnew(false);
      await api<Run>('/api/admin/remna-sync/check', { method: 'POST', body: { direction: dir } });
    }, 'Проверка не запустилась');

  const start = () =>
    act(async () => {
      if (!run) return;
      const squads: Record<string, string | null> = {};
      for (const squad of run.plan?.squads ?? []) {
        const value = choice.squads[squad.old] ?? squad.suggested ?? DROP;
        squads[squad.old] = value === DROP ? null : value;
      }
      await api<Run>(`/api/admin/remna-sync/${run.id}/start`, {
        method: 'POST',
        body: { mode: run.direction === 'push' ? choice.mode : 'create', squads, tariffs: choice.tariffs },
      });
    }, 'Перенос не запустился');

  let body: ReactNode = null;
  let actions: ReactNode = null;
  if (view && !view.remna) {
    body = (
      <div className="imp-block" role="alert">
        <span className="strong">Remnawave не подключена</span>
        <span>
          Задайте адрес панели и токен в <Link to="/admin/settings/remnawave">«Настройки → Remnawave»</Link>, затем вернитесь сюда.
        </span>
      </div>
    );
  } else if (view && (!run || lost)) {
    body = (
      <>
        <Pick value={direction} onPick={setDirection} disabled={busy} />
        {!view.secrets && direction === 'push' ? (
          <p className="cap">
            Ключ шифрования панели не задан, поэтому ключи протоколов не сохраняются. Ссылки при переносе сохранятся, а ключи будут новыми.
          </p>
        ) : null}
        {run && (run.state === 'failed' || run.state === 'interrupted') && run.error ? (
          <p className="err" role="alert">
            {run.error}
          </p>
        ) : null}
      </>
    );
    actions = (
      <>
        <button type="button" className="btn-main btn" onClick={() => void check(direction)} disabled={busy}>
          Проверить
        </button>
        <span className="cap">Проверка ничего не меняет ни в панели, ни в Remnawave.</span>
      </>
    );
  } else if (run?.state === 'checking') {
    body = (
      <div className="imp-run">
        <span className="strong">{title(run.direction)}: проверяем</span>
        <div className="imp-bar is-wait" aria-hidden="true">
          <span />
        </div>
        <span className="cap">Читаем всех пользователей Remnawave и подписки панели. На большой базе это займёт пару минут, страницу можно закрыть.</span>
      </div>
    );
  } else if (run?.state === 'running') {
    body = (
      <>
        <span className="strong">{title(run.direction)}</span>
        <Running run={run} onPause={() => void act(() => api(`/api/admin/remna-sync/${run.id}/pause`, { method: 'POST', body: {} }), 'Не остановилось')} busy={busy} />
      </>
    );
  } else if (run?.state === 'done') {
    body = (
      <>
        <span className="strong">
          {title(run.direction)} закончен {run.finished_at ? dateTime(run.finished_at).toLowerCase() : ''}
        </span>
        <Done run={run} busy={busy} onNotify={() => void act(() => api(`/api/admin/remna-sync/${run.id}/notify`, { method: 'POST', body: {} }), 'Не отправилось')} />
      </>
    );
    actions = (
      <button
        type="button"
        className="btn-line hov"
        onClick={() => {
          setAnew(true);
          setError(null);
        }}
        disabled={busy}
      >
        Новая проверка
      </button>
    );
  } else if (run?.plan) {
    const plan = run.plan;
    const resumed = run.state !== 'ready';
    const nothing = plan.direction === 'push' ? plan.create.count + plan.fix.count + plan.relink.count === 0 : plan.create.count + plan.attach.count + plan.relink.count === 0;
    body = (
      <>
        <span className="strong">{title(run.direction)}: проверка {dateTime(run.created_at).toLowerCase()}</span>
        <Problem run={run} />
        {plan.direction === 'push' ? (
          <PushPlan run={run} plan={plan} choice={choice} onChange={setChoice} busy={busy} />
        ) : (
          <PullPlan run={run} plan={plan} />
        )}
      </>
    );
    actions = (
      <>
        <button type="button" className="btn-main btn" onClick={() => void start()} disabled={busy || (nothing && !resumed)}>
          {resumed ? 'Продолжить' : plan.direction === 'push' ? 'Перенести в Remnawave' : 'Забрать из Remnawave'}
        </button>
        <button type="button" className="btn-line hov" onClick={() => void check(run.direction)} disabled={busy}>
          Проверить снова
        </button>
        <button
          type="button"
          className="imp-link"
          onClick={() =>
            void act(async () => {
              await api(`/api/admin/remna-sync/${run.id}`, { method: 'DELETE' });
              setDirection(run.direction === 'push' ? 'pull' : 'push');
              setAnew(true);
            }, 'Не получилось')
          }
          disabled={busy}
        >
          Другое направление
        </button>
        {nothing && !resumed ? <span className="cap">Переносить нечего: всё уже совпадает.</span> : null}
      </>
    );
  }

  return (
    <SettingsLayout>
      <div className="head-stack">
        <h2 className="section-title">Синхронизация с Remnawave</h2>
        <p className="muted">
          Перенести клиентов и подписки панели в Remnawave или забрать в панель пользователей Remnawave. Сначала проверка без записи, потом одна
          кнопка. Из Remnawave ничего не удаляется.
        </p>
      </div>
      {loadError && !view ? (
        <section className="sec card-pad" role="alert">
          <h2 className="section-title">Раздел не загрузился</h2>
          <p className="muted">{loadError}</p>
        </section>
      ) : null}
      {view ? (
        <section className="sec imp-card" aria-label="Синхронизация">
          <div className="imp-step-body">{body}</div>
          {actions ? <div className="imp-actions">{actions}</div> : null}
        </section>
      ) : null}
      {error ? (
        <p className="err imp-error" role="alert">
          {error}
        </p>
      ) : null}
      {view ? <History items={view.history.filter((h) => h.id !== run?.id)} /> : null}
    </SettingsLayout>
  );
}
