import { Fragment, useState } from 'react';
import { groupDigits } from '../../lib/format';
import { Icon } from '../../lib/icons';
import type { Currency } from '../../lib/types';
import { SeriesChart, ShareBar } from './charts';
import {
  change,
  compareText,
  dayText,
  format,
  monthTitle,
  periodText,
  type Bar,
  type Breakdown,
  type CohortRow,
  type Column,
  type ListRef,
  type Metric,
  type Query,
  type Row,
  type Step,
  type TabReport,
  shareOf,
  linkColumn,
} from './model';

export type OpenList = (ref: ListRef, title: string) => void;

type Props = {
  metric: Metric;
  report: TabReport;
  q: Query;
  setQ: (next: Partial<Query>) => void;
  onList: OpenList;
  compact?: boolean;
};

const STEPS: Array<[Step, string]> = [
  ['day', 'По дням'],
  ['week', 'По неделям'],
  ['month', 'По месяцам'],
];

const SHORT_STEPS: Record<Step, string> = {
  day: 'Дни',
  week: 'Недели',
  month: 'Месяцы',
};

function stepsFor(from: string, to: string): Array<[Step, string]> {
  const days = (Date.parse(to) - Date.parse(from)) / 86_400_000 + 1;
  return STEPS.filter(([s]) => (s === 'day' ? days <= 120 : s === 'week' ? days >= 14 && days <= 840 : days >= 60));
}

function SeriesDetail({ metric, report, setQ, onList, compact }: Props) {
  const d = metric.detail;
  const [active, setActive] = useState<number | null>(null);
  const [by, setBy] = useState<string | null>(null);
  if (d.type !== 'series') {
    return null;
  }
  const c = report.currency;
  const range = report.range;
  const live = range.to === range.today;
  const steps = stepsFor(range.from, range.to);
  const point = active !== null ? d.points[active] : null;
  const prevPoint = active !== null ? d.prev[active] : null;
  const breakdown = d.breakdowns.find((b) => b.key === by) ?? d.breakdowns[0];
  const empty = d.points.length === 0;
  return (
    <>
      {empty ? null : (
        <div className="ax-chart-top">
          <div className="an-keys">
            <span className="an-key">
              <i className="an-key-bar" /> {periodText(range.from, range.to)}
            </span>
            {d.prev.length && compareText(range) ? (
              <span className="an-key">
                <i className="ax-key-prev" /> {compareText(range)}
              </span>
            ) : null}
          </div>
          {steps.length > 1 ? (
            <div className="segs small" role="group" aria-label="Шаг графика">
              {steps.map(([s, label]) => (
                <button key={s} type="button" aria-pressed={range.step === s} onClick={() => setQ({ step: s })}>
                  {compact ? SHORT_STEPS[s] : label}
                </button>
              ))}
            </div>
          ) : null}
        </div>
      )}
      {point ? (
        <p className="ax-readout">
          <b>{dayText(point.at, false)}</b>: {format(point.value, metric.unit, c)}
          {prevPoint ? <span className="muted"> · было {format(prevPoint.value, metric.unit, c)}</span> : null}
        </p>
      ) : null}
      {empty ? (
        <p className="muted ax-empty">{metric.hint ?? 'Пока нет данных по дням'}</p>
      ) : (
        <SeriesChart
          points={d.points}
          prev={d.prev}
          unit={metric.unit}
          currency={c}
          step={range.step}
          live={live}
          height={compact ? 170 : 220}
          active={active}
          onActive={setActive}
          compact={compact}
        />
      )}
      {breakdown ? (
        <BreakdownTable
          all={d.breakdowns}
          breakdown={breakdown}
          setBy={setBy}
          metric={metric}
          currency={c}
          onList={onList}
          compact={compact}
          comparing={Boolean(range.prev)}
        />
      ) : null}
      {d.list ? (
        <button type="button" className={compact ? 'axp-open' : 'b36 hov ax-open'} onClick={() => onList(d.list as ListRef, metric.title)}>
          {listLabel(d.list)}
        </button>
      ) : null}
    </>
  );
}

function listLabel(ref: ListRef): string {
  return ref.kind === 'payments' ? 'Открыть список оплат' : 'Открыть список клиентов';
}

type BreakdownProps = {
  all: Breakdown[];
  breakdown: Breakdown;
  setBy: (key: string) => void;
  metric: Metric;
  currency: Currency;
  onList: OpenList;
  compact?: boolean;
  comparing: boolean;
};

function BreakdownTable({ all, breakdown, setBy, metric, currency, onList, compact, comparing: wanted }: BreakdownProps) {
  const comparing = wanted && breakdown.rows.some((r) => r.prev !== null && r.prev !== undefined);
  const total = breakdown.rows.reduce((acc, r) => acc + Math.max(0, r.value), 0);
  const counted = breakdown.rows.some((r) => r.count !== undefined);
  return (
    <div className="ax-break">
      <div className="ax-bhead">
        <h3 className="ax-h3">{all.length > 1 ? 'Из чего сложилось' : breakdown.title}</h3>
        {all.length > 1 ? (
          compact ? (
            <label className="anp-chip wide">
              <select value={breakdown.key} onChange={(e) => setBy(e.target.value)}>
                {all.map((b) => (
                  <option key={b.key} value={b.key}>
                    {`По: ${b.title.toLowerCase()}`}
                  </option>
                ))}
              </select>
              <Icon name="chevron-down" size={16} stroke={2.2} />
            </label>
          ) : (
            <div className="segs small" role="group" aria-label="Разбивка">
              {all.map((b) => (
                <button key={b.key} type="button" aria-pressed={b.key === breakdown.key} onClick={() => setBy(b.key)}>
                  {b.title}
                </button>
              ))}
            </div>
          )
        ) : null}
      </div>
      {compact ? (
        <div className="hp-card hp-list">
          {breakdown.rows.map((r) => {
            const part = total > 0 ? (Math.max(0, r.value) / total) * 100 : 0;
            const body = (
              <span className="anp-row-main">
                <span className="anp-row-top">
                  <span className="anp-row-name">{r.label}</span>
                  <span className="num anp-row-sum">{format(r.value, metric.unit, currency)}</span>
                </span>
                {total > 0 ? <ShareBar value={part} /> : null}
                <span className="anp-row-sub">
                  {total > 0 ? `${format(part, 'percent', currency)}` : ''}
                  {r.count !== undefined ? ` · ${groupDigits(r.count)}` : ''}
                  {comparing ? ` · ${change(r.value, r.prev ?? 0, metric.unit) ?? 'было 0'}` : ''}
                </span>
              </span>
            );
            return r.list ? (
              <button key={r.key} type="button" className="anp-row" onClick={() => onList(r.list as ListRef, `${metric.title} · ${r.label}`)}>
                {body}
                <Icon name="chevron-right" size={16} stroke={2.2} className="anp-chev" />
              </button>
            ) : (
              <div key={r.key} className="anp-row">
                {body}
              </div>
            );
          })}
        </div>
      ) : (
        <div className="an-tw">
          <table className="an-t">
            <thead>
              <tr>
                <th />
                <th className="r">{metric.unit === 'money' ? 'Сумма' : 'Значение'}</th>
                <th>Доля</th>
                {counted ? <th className="r">Штук</th> : null}
                {comparing ? <th className="r">К прошлому</th> : null}
              </tr>
            </thead>
            <tbody>
              {breakdown.rows.map((r) => (
                <tr key={r.key}>
                  <td className="strong">{r.label}</td>
                  <td className="r num">
                    {r.list ? (
                      <button type="button" className="ax-num" onClick={() => onList(r.list as ListRef, `${metric.title} · ${r.label}`)}>
                        {format(r.value, metric.unit, currency)}
                      </button>
                    ) : (
                      format(r.value, metric.unit, currency)
                    )}
                  </td>
                  <td className="an-t-share">
                    {total > 0 ? (
                      <span className="an-share-row">
                        <ShareBar value={(Math.max(0, r.value) / total) * 100} />
                        <span className="num">{format((Math.max(0, r.value) / total) * 100, 'percent', currency)}</span>
                      </span>
                    ) : null}
                  </td>
                  {counted ? <td className="r num">{r.count !== undefined ? groupDigits(r.count) : ''}</td> : null}
                  {comparing ? <td className="r num ax-dim">{change(r.value, r.prev ?? 0, metric.unit) ?? 'было 0'}</td> : null}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function cellText(value: number | null, column: Column, currency: Currency): string {
  return value === null ? '' : format(value, column.unit, currency);
}

function TableDetail({ metric, report, onList, compact }: Props) {
  const d = metric.detail;
  const [open, setOpen] = useState<Set<string>>(new Set());
  if (d.type !== 'table') {
    return null;
  }
  const c = report.currency;
  const toggle = (label: string) => {
    const next = new Set(open);
    if (next.has(label)) {
      next.delete(label);
    } else {
      next.add(label);
    }
    setOpen(next);
  };
  const lineTitle = (r: Row) => `${metric.title} · ${r.label}`;
  const link = linkColumn(d.columns);
  if (compact) {
    const item = (r: Row, sub: boolean) => {
      const facts = d.columns
        .map((col, i) => {
          if (r.cells[i] === null || r.cells[i] === undefined) {
            return null;
          }
          const part = shareOf(r.cells, col, i);
          const name = /^[A-Z0-9]+$/.test(col.title) ? col.title : col.title.toLowerCase();
          return `${name} ${cellText(r.cells[i] ?? null, col, c)}${part ? ` (${part})` : ''}`;
        })
        .filter(Boolean)
        .join(' · ');
      const kids = r.children?.length ?? 0;
      const content = (
        <span className="anp-row-main">
          <span className="anp-row-top">
            <span className="anp-row-name">{r.label}</span>
          </span>
          {r.sub ? <span className="anp-row-sub">{r.sub}</span> : null}
          <span className="anp-row-sub ax-facts-line">{facts}</span>
        </span>
      );
      if (kids && !sub) {
        return (
          <button key={r.label} type="button" className="anp-row" aria-expanded={open.has(r.label)} onClick={() => toggle(r.label)}>
            {content}
            <Icon name={open.has(r.label) ? 'chevron-down' : 'chevron-right'} size={16} stroke={2.2} className="anp-chev" />
          </button>
        );
      }
      return r.list ? (
        <button key={r.label} type="button" className={sub ? 'anp-row sub' : 'anp-row'} onClick={() => onList(r.list as ListRef, lineTitle(r))}>
          {content}
          <Icon name="chevron-right" size={16} stroke={2.2} className="anp-chev" />
        </button>
      ) : (
        <div key={r.label} className={sub ? 'anp-row sub' : 'anp-row'}>
          {content}
        </div>
      );
    };
    return (
      <>
        {d.rows.length ? (
          <div className="hp-card hp-list">
            {d.rows.map((r) => (
              <Fragment key={r.label}>
                {item(r, false)}
                {open.has(r.label) ? r.children?.map((ch) => item(ch, true)) : null}
              </Fragment>
            ))}
          </div>
        ) : (
          <p className="muted ax-empty">За этот период ничего нет</p>
        )}
        {d.note ? <p className="anp-note">{d.note}</p> : null}
      </>
    );
  }
  return (
    <>
      {d.rows.length ? (
        <div className="an-tw">
          <table className="an-t ax-table">
            <thead>
              <tr>
                <th />
                {d.columns.map((col) => (
                  <th key={col.title} className="r">
                    {col.title}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {d.rows.map((r) => (
                <Fragment key={r.label}>
                  <tr>
                    <td>
                      {r.children?.length ? (
                        <button type="button" className="an-exp" aria-expanded={open.has(r.label)} onClick={() => toggle(r.label)}>
                          <Icon name="chevron-right" size={15} stroke={2.4} />
                          {r.label}
                        </button>
                      ) : (
                        <span className={r.list ? 'strong' : 'strong'}>{r.label}</span>
                      )}
                      {r.sub ? <span className="ax-sub ax-rowsub">{r.sub}</span> : null}
                    </td>
                    {d.columns.map((col, i) => (
                      <td key={col.title} className="r num">
                        {i === link && r.list && r.cells[i] != null ? (
                          <button type="button" className="ax-num" onClick={() => onList(r.list as ListRef, lineTitle(r))}>
                            {cellText(r.cells[i] ?? null, col, c)}
                          </button>
                        ) : (
                          cellText(r.cells[i] ?? null, col, c)
                        )}
                        {shareOf(r.cells, col, i) ? <span className="ax-cshare">{shareOf(r.cells, col, i)}</span> : null}
                      </td>
                    ))}
                  </tr>
                  {open.has(r.label)
                    ? r.children?.map((ch) => (
                        <tr key={`${r.label}/${ch.label}`} className="an-sub">
                          <td className="an-exp-pad sub">
                            {ch.label}
                            {ch.sub ? <span className="ax-sub ax-rowsub">{ch.sub}</span> : null}
                          </td>
                          {d.columns.map((col, i) => (
                            <td key={col.title} className="r num">
                              {i === link && ch.list && ch.cells[i] != null ? (
                                <button type="button" className="ax-num" onClick={() => onList(ch.list as ListRef, lineTitle(ch))}>
                                  {cellText(ch.cells[i] ?? null, col, c)}
                                </button>
                              ) : (
                                cellText(ch.cells[i] ?? null, col, c)
                              )}
                              {shareOf(ch.cells, col, i) ? <span className="ax-cshare">{shareOf(ch.cells, col, i)}</span> : null}
                            </td>
                          ))}
                        </tr>
                      ))
                    : null}
                </Fragment>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <p className="muted ax-empty">За этот период ничего нет</p>
      )}
      {d.note ? <p className="ax-note muted">{d.note}</p> : null}
    </>
  );
}

function BarsDetail({ metric, report, onList, compact }: Props) {
  const d = metric.detail;
  if (d.type !== 'bars') {
    return null;
  }
  const max = Math.max(1, ...d.bars.map((b) => b.value));
  const total = d.bars.reduce((acc, b) => acc + b.value, 0);
  const line = (b: Bar) => (
    <>
      <span className="ax-bar-label">{b.label}</span>
      <span className="ax-bar-track">
        <span style={{ width: `${(b.value / max) * 100}%` }} />
      </span>
      <span className="num ax-bar-value">
        {groupDigits(b.value)}
        {total > 0 ? <span className="muted"> · {format((b.value / total) * 100, 'percent', report.currency)}</span> : null}
      </span>
    </>
  );
  return (
    <>
      <div className={compact ? 'hp-card ax-bars phone' : 'ax-bars'}>
        {d.bars.map((b) =>
          b.list ? (
            <button key={b.label} type="button" className="ax-bar-row" onClick={() => onList(b.list as ListRef, `${metric.title} · ${b.label}`)}>
              {line(b)}
            </button>
          ) : (
            <div key={b.label} className="ax-bar-row">
              {line(b)}
            </div>
          ),
        )}
      </div>
      {d.note ? <p className={compact ? 'anp-note' : 'ax-note muted'}>{d.note}</p> : null}
    </>
  );
}

function heat(part: number | null, size: number): string | undefined {
  if (part === null || !size) {
    return undefined;
  }
  const v = Math.max(0, Math.min(1, part / size));
  return `color-mix(in srgb, var(--accent) ${Math.round(8 + v * 86)}%, transparent)`;
}

function CohortDetail({ metric, report, compact }: Props) {
  const d = metric.detail;
  const [view, setView] = useState<'share' | 'money'>('share');
  if (d.type !== 'cohorts') {
    return null;
  }
  const rows: CohortRow[] = d.rows.filter((r) => r.size > 0);
  const used = Math.max(1, Math.min(d.columns, rows.reduce((acc, r) => Math.max(acc, r.kept.filter((v) => v !== null).length), 0)));
  const c = report.currency;
  return (
    <>
      <div className="ax-chart-top">
        {compact ? <p className="ax-coh-say">{metric.hint}</p> : null}
        <div className="segs small" role="group" aria-label="Что показывать">
          <button type="button" aria-pressed={view === 'share'} onClick={() => setView('share')}>
            Доля клиентов
          </button>
          <button type="button" aria-pressed={view === 'money'} onClick={() => setView('money')}>
            Деньги на клиента
          </button>
        </div>
      </div>
      {rows.length ? (
        <div className="an-coh-wrap">
          <table className="an-coh">
            <thead>
              <tr>
                <th className="an-coh-m" aria-hidden="true" />
                <th className="an-coh-n" aria-hidden="true" />
                <th className="an-coh-cap" colSpan={used}>
                  {view === 'share' ? 'Месяцев после первой оплаты, доля с оплаченной подпиской, %' : 'Месяцев после первой оплаты, принёс в среднем клиент'}
                </th>
              </tr>
              <tr>
                <th className="an-coh-m">Первая оплата</th>
                <th className="an-coh-n">Клиентов</th>
                {Array.from({ length: used }, (_, k) => (
                  <th key={k} className="an-coh-k">
                    {k + 1}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.month}>
                  <th className="an-coh-m" scope="row">
                    {monthTitle(r.month)}
                  </th>
                  <td className="an-coh-n num">{groupDigits(r.size)}</td>
                  {Array.from({ length: used }, (_, k) => {
                    const kept = r.kept[k] ?? null;
                    const cash = r.money[k] ?? null;
                    return view === 'share' ? (
                      <td key={k} className="an-coh-c num" style={{ background: heat(kept, r.size) }}>
                        {kept === null ? '' : Math.round((kept / r.size) * 100)}
                      </td>
                    ) : (
                      <td key={k} className="an-coh-c num ax-coh-money">
                        {cash === null ? '' : format(cash / r.size, 'money', c, true)}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <p className="muted ax-empty">За этот период никто не платил впервые</p>
      )}
    </>
  );
}

export function MetricDetail(props: Props) {
  switch (props.metric.detail.type) {
    case 'series':
      return <SeriesDetail {...props} />;
    case 'table':
      return <TableDetail {...props} />;
    case 'bars':
      return <BarsDetail {...props} />;
    case 'cohorts':
      return <CohortDetail {...props} />;
    default:
      return props.metric.hint ? <p className="ax-sub">{props.metric.hint}</p> : null;
  }
}
