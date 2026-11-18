import type { Currency } from '../../lib/types';
import { format, type Metric, type TabReport } from './model';

type Cell = string | number | null;

function cell(value: Cell): string {
  if (value === null) {
    return '';
  }
  const text = String(value);
  return /[;"\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

function plain(value: number | null | undefined, unit: Metric['unit'], currency: Currency): string {
  if (value === null || value === undefined) {
    return '';
  }
  if (unit === 'money') {
    const major = value / 100;
    return currency === 'RUB' ? String(Math.round(major)) : major.toFixed(2).replace('.', ',');
  }
  if (unit === 'percent') {
    return value.toFixed(1).replace('.', ',');
  }
  return format(value, unit, currency).replace(/\s/g, '');
}

export function downloadRows(title: string, rows: Cell[][]): void {
  const text = `﻿${rows.map((r) => r.map(cell).join(';')).join('\r\n')}\r\n`;
  const blob = new Blob([text], { type: 'text/csv;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `${title.replace(/[\\/:*?"<>|]+/g, ' ').trim() || 'analytics'}.csv`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

export function metricRows(metric: Metric, report: TabReport): Cell[][] {
  const c = report.currency;
  const d = metric.detail;
  const out: Cell[][] = [[metric.title, plain(metric.value, metric.unit, c), metric.note ?? '']];
  switch (d.type) {
    case 'series':
      out.push([]);
      out.push(['Дата', 'Значение', d.prev.length ? 'Прошлый период' : '']);
      d.points.forEach((p, i) => out.push([p.at, plain(p.value, metric.unit, c), d.prev[i] ? plain(d.prev[i].value, metric.unit, c) : '']));
      for (const b of d.breakdowns) {
        out.push([]);
        out.push([b.title, 'Значение', 'Штук', 'Прошлый период']);
        b.rows.forEach((r) => out.push([r.label, plain(r.value, metric.unit, c), r.count ?? '', r.prev === undefined ? '' : plain(r.prev, metric.unit, c)]));
      }
      break;
    case 'table':
      out.push([]);
      out.push(['', ...d.columns.map((col) => col.title)]);
      for (const r of d.rows) {
        out.push([r.label, ...r.cells.map((v, i) => plain(v, d.columns[i]?.unit ?? 'count', c))]);
        for (const ch of r.children ?? []) {
          out.push([`  ${ch.label}`, ...ch.cells.map((v, i) => plain(v, d.columns[i]?.unit ?? 'count', c))]);
        }
      }
      break;
    case 'bars':
      out.push([]);
      d.bars.forEach((b) => out.push([b.label, b.value]));
      break;
    case 'cohorts':
      out.push([]);
      out.push(['Месяц первой оплаты', 'Клиентов', ...Array.from({ length: d.columns }, (_, k) => `Через ${k + 1} мес., клиентов`)]);
      d.rows.forEach((r) => out.push([r.month, r.size, ...r.kept.map((v) => v ?? '')]));
      out.push([]);
      out.push(['Месяц первой оплаты', 'Клиентов', ...Array.from({ length: d.columns }, (_, k) => `Через ${k + 1} мес., выручка`)]);
      d.rows.forEach((r) => out.push([r.month, r.size, ...r.money.map((v) => (v === null ? '' : plain(v, 'money', c)))]));
      break;
    default:
      break;
  }
  return out;
}

export function tabRows(report: TabReport): Cell[][] {
  const c = report.currency;
  const out: Cell[][] = [['Показатель', 'Значение', 'Прошлый период', 'Заметка']];
  for (const g of report.groups) {
    for (const m of g.metrics) {
      out.push([g.title ? `${g.title}: ${m.title}` : m.title, plain(m.value, m.unit, c), plain(m.prev ?? null, m.unit, c), m.note ?? '']);
    }
  }
  return out;
}
