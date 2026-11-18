import { useCallback, useEffect, useRef, useState } from 'react';
import { groupDigits } from '../../lib/format';
import type { Currency } from '../../lib/types';
import { compact, monthName, monthShort, percent, type Cohort, type MonthRow } from './model';

export function useWidth<T extends HTMLElement>(): [(node: T | null) => void, number] {
  const [width, setWidth] = useState(0);
  const observer = useRef<ResizeObserver | null>(null);
  const ref = useCallback((node: T | null) => {
    observer.current?.disconnect();
    if (!node) {
      return;
    }
    setWidth(node.clientWidth);
    observer.current = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (box) {
        setWidth(Math.round(box.width));
      }
    });
    observer.current.observe(node);
  }, []);
  useEffect(() => () => observer.current?.disconnect(), []);
  return [ref, width];
}

function niceStep(max: number, lines: number): number {
  const raw = max / lines;
  const power = 10 ** Math.floor(Math.log10(Math.max(raw, 1)));
  for (const k of [1, 2, 2.5, 5, 10]) {
    if (k * power >= raw) {
      return k * power;
    }
  }
  return 10 * power;
}

type MoneyChartProps = {
  months: MonthRow[];
  currency: Currency;
  current: string;
  active: number;
  onActive: (index: number) => void;
  height: number;
  showMrr?: boolean;
};

function spacing(slot: number, total: number): number {
  const gap = total > 12 ? 46 : 30;
  return slot < gap ? Math.ceil(gap / Math.max(slot, 1)) : 1;
}

function labelled(i: number, total: number, every: number): boolean {
  if (i === total - 1) {
    return true;
  }
  return i % every === 0 && total - 1 - i >= every;
}

function axisLabels(months: MonthRow[], every: number): Map<number, string> {
  const out = new Map<number, string>();
  let year = '';
  months.forEach((m, i) => {
    if (!labelled(i, months.length, every)) {
      return;
    }
    const current = m.month.slice(0, 4);
    out.set(i, months.length > 12 && current !== year ? monthShort(m.month, true) : monthShort(m.month));
    year = current;
  });
  return out;
}

export function MoneyChart({ months, currency, current, active, onActive, height, showMrr = true }: MoneyChartProps) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const left = 64;
  const bottom = 26;
  const top = 10;
  const inner = Math.max(0, width - left - 4);
  const plot = height - bottom - top;
  const max = Math.max(1, ...months.map((m) => Math.max(m.revenue, showMrr ? m.mrr : 0)));
  const step = niceStep(max, 4);
  const ceil = Math.ceil(max / step) * step;
  const lines = Array.from({ length: Math.round(ceil / step) + 1 }, (_, i) => i * step);
  const slot = months.length ? inner / months.length : 0;
  const bar = Math.max(4, Math.min(44, slot * 0.56));
  const y = (v: number) => top + plot - (v / ceil) * plot;
  const x = (i: number) => left + slot * i + slot / 2;
  const path = months.map((m, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(m.mrr).toFixed(1)}`).join(' ');
  const every = spacing(slot, months.length);
  const labels = axisLabels(months, every);
  return (
    <div className="an-chart" ref={ref} style={{ height }} onMouseLeave={() => onActive(months.length - 1)}>
      {width > 0 ? (
        <svg width={width} height={height} role="img" aria-label="Выручка и MRR по месяцам">
          {lines.map((v) => (
            <g key={v}>
              <line x1={left} x2={width} y1={y(v)} y2={y(v)} className="an-grid" />
              <text x={left - 10} y={y(v) + 4} textAnchor="end" className="an-axis">
                {v === 0 ? '0' : compact(v, currency)}
              </text>
            </g>
          ))}
          {months.map((m, i) => {
            const h = Math.max(m.revenue > 0 ? 2 : 0, plot - (y(m.revenue) - top));
            const live = m.month === current;
            return (
              <g key={m.month}>
                <rect
                  x={x(i) - bar / 2}
                  y={top + plot - h}
                  width={bar}
                  height={h}
                  rx={Math.min(5, bar / 4)}
                  className={`an-bar${i === active ? ' on' : ''}${live ? ' live' : ''}`}
                />
                {labels.has(i) ? (
                  <text x={x(i)} y={height - 6} textAnchor="middle" className={`an-axis${i === active ? ' on' : ''}`}>
                    {labels.get(i)}
                  </text>
                ) : null}
                <rect x={left + slot * i} y={0} width={slot} height={height} fill="transparent" onMouseEnter={() => onActive(i)} onClick={() => onActive(i)} />
              </g>
            );
          })}
          {showMrr && months.length > 1 ? <path d={path} className="an-line" /> : null}
          {showMrr
            ? months.map((m, i) => <circle key={m.month} cx={x(i)} cy={y(m.mrr)} r={i === active ? 4.5 : 2.5} className={`an-pt${i === active ? ' on' : ''}`} />)
            : null}
        </svg>
      ) : null}
    </div>
  );
}

type FlowProps = {
  months: MonthRow[];
  active: number;
  onActive: (index: number) => void;
  height: number;
};

export function FlowChart({ months, active, onActive, height }: FlowProps) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const left = 40;
  const bottom = 26;
  const top = 8;
  const inner = Math.max(0, width - left - 4);
  const plot = height - bottom - top;
  const up = Math.max(1, ...months.map((m) => m.new + m.returned));
  const down = Math.max(1, ...months.map((m) => m.churned));
  const unit = plot / (up + down);
  const zero = top + up * unit;
  const slot = months.length ? inner / months.length : 0;
  const bar = Math.max(4, Math.min(30, slot * 0.46));
  const x = (i: number) => left + slot * i + slot / 2;
  const every = spacing(slot, months.length);
  const labels = axisLabels(months, every);
  return (
    <div className="an-chart" ref={ref} style={{ height }} onMouseLeave={() => onActive(months.length - 1)}>
      {width > 0 ? (
        <svg width={width} height={height} role="img" aria-label="Пришли и ушли по месяцам">
          <line x1={left} x2={width} y1={zero} y2={zero} className="an-grid strong" />
          <text x={left - 8} y={top + 10} textAnchor="end" className="an-axis">
            +{groupDigits(up)}
          </text>
          <text x={left - 8} y={height - bottom} textAnchor="end" className="an-axis">
            −{groupDigits(down)}
          </text>
          {months.map((m, i) => {
            const fresh = m.new * unit;
            const back = m.returned * unit;
            const gone = m.churned * unit;
            return (
              <g key={m.month} className={i === active ? 'on' : ''}>
                <rect x={x(i) - bar / 2} y={zero - fresh} width={bar} height={fresh} className="an-up" />
                <rect x={x(i) - bar / 2} y={zero - fresh - back} width={bar} height={back} className="an-up back" />
                <rect x={x(i) - bar / 2} y={zero + 1} width={bar} height={gone} className="an-down" />
                {labels.has(i) ? (
                  <text x={x(i)} y={height - 6} textAnchor="middle" className={`an-axis${i === active ? ' on' : ''}`}>
                    {labels.get(i)}
                  </text>
                ) : null}
                <rect x={left + slot * i} y={0} width={slot} height={height} fill="transparent" onMouseEnter={() => onActive(i)} onClick={() => onActive(i)} />
              </g>
            );
          })}
        </svg>
      ) : null}
    </div>
  );
}

export function heat(part: number | null, size: number): string | undefined {
  if (part === null || !size) {
    return undefined;
  }
  const value = Math.max(0, Math.min(1, part / size));
  return `color-mix(in srgb, var(--accent) ${Math.round(8 + value * 86)}%, transparent)`;
}

type CohortProps = {
  cohorts: Cohort[];
  columns: number;
  money?: (minor: number) => string;
};

export function CohortGrid({ cohorts, columns, money }: CohortProps) {
  const shown = Math.max(1, Math.min(12, columns));
  return (
    <div className="an-coh-wrap">
      <table className="an-coh">
        <thead>
          <tr>
            <th className="an-coh-m" aria-hidden="true" />
            <th className="an-coh-n" aria-hidden="true" />
            <th className="an-coh-cap" colSpan={shown}>
              Месяцев после первой покупки
            </th>
            {money ? <th className="an-coh-n" aria-hidden="true" /> : null}
          </tr>
          <tr>
            <th className="an-coh-m">Первая покупка</th>
            <th className="an-coh-n">Клиентов</th>
            {Array.from({ length: shown }, (_, k) => (
              <th key={k} className="an-coh-k">
                {k + 1}
              </th>
            ))}
            {money ? <th className="an-coh-n">Принёс клиент</th> : null}
          </tr>
        </thead>
        <tbody>
          {cohorts.map((c) => (
            <tr key={c.month}>
              <th className="an-coh-m" scope="row">
                {monthName(c.month, true)}
              </th>
              <td className="an-coh-n num">{groupDigits(c.size)}</td>
              {Array.from({ length: shown }, (_, k) => {
                const v = c.retained[k] ?? null;
                return (
                  <td key={k} className="an-coh-c num" style={{ background: heat(v, c.size) }} title={v === null ? 'Ещё не наступило' : `${groupDigits(v)} из ${groupDigits(c.size)}`}>
                    {v === null ? '' : percent(v, c.size).replace(/\s%$/, '')}
                  </td>
                );
              })}
              {money ? <td className="an-coh-n num">{c.size ? money(Math.round(c.revenue / c.size)) : '—'}</td> : null}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function ShareLine({ value }: { value: number }) {
  return (
    <span className="an-share" aria-hidden="true">
      <span style={{ width: `${Math.max(value > 0 ? 2 : 0, Math.round(value * 100))}%` }} />
    </span>
  );
}
