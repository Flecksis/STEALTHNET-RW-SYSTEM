import type { Currency } from '../../lib/types';
import { avgCheck, monthName, sourceName, type Report } from './model';

export type CsvPart = 'months' | 'cohorts' | 'providers' | 'sources';

export const CSV_PARTS: Array<[CsvPart, string]> = [
  ['months', 'Деньги и клиенты по месяцам'],
  ['cohorts', 'Удержание по когортам'],
  ['providers', 'Платёжки и способы оплаты'],
  ['sources', 'Источники'],
];

function amount(minor: number, currency: Currency): string {
  if (currency === 'RUB') {
    return String(Math.round(minor / 100));
  }
  const sign = minor < 0 ? '-' : '';
  const abs = Math.abs(minor);
  return `${sign}${Math.floor(abs / 100)},${String(abs % 100).padStart(2, '0')}`;
}

function cell(value: string | number | null): string {
  if (value === null) {
    return '';
  }
  const text = String(value);
  return /[;"\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

function lines(rows: Array<Array<string | number | null>>): string {
  return `﻿${rows.map((row) => row.map(cell).join(';')).join('\r\n')}\r\n`;
}

export function buildCsv(report: Report, part: CsvPart): string {
  const c = report.currency;
  const unit = c === 'RUB' ? 'руб.' : 'USD';
  switch (part) {
    case 'months':
      return lines([
        ['Месяц', `Выручка, ${unit}`, 'Звёзды', `Возвраты, ${unit}`, 'Оплат', `Средний чек, ${unit}`, `MRR, ${unit}`, 'Новые', 'Продлили', 'Вернулись', 'Ушли', 'Платящих на конец месяца', 'Взяли пробный', 'Купили после пробного'],
        ...report.months.map((m) => [
          monthName(m.month, true),
          amount(m.revenue, c),
          m.stars,
          amount(m.refunds, c),
          m.payments,
          amount(avgCheck(m.revenue, m.payments), c),
          amount(m.mrr, c),
          m.new,
          m.renewed,
          m.returned,
          m.churned,
          m.paying,
          m.trials,
          m.trials_bought,
        ]),
      ]);
    case 'cohorts': {
      const width = Math.max(0, ...report.cohorts.map((k) => k.retained.length));
      return lines([
        ['Месяц первой покупки', 'Клиентов', `Принёс клиент, ${unit}`, ...Array.from({ length: width }, (_, k) => `Через ${k + 1} мес., клиентов`)],
        ...report.cohorts.map((k) => [
          monthName(k.month, true),
          k.size,
          amount(k.size ? Math.round(k.revenue / k.size) : 0, c),
          ...Array.from({ length: width }, (_, i) => k.retained[i] ?? null),
        ]),
      ]);
    }
    case 'providers':
      return lines([
        ['Платёжка', 'Способ', `Оборот, ${unit}`, 'Звёзды', 'Оплат', 'Счетов', 'Не оплатили', 'Возвратов', `Возвращено, ${unit}`],
        ...report.providers.flatMap((p) => [
          [p.title, '', amount(p.revenue, c), p.stars, p.payments, p.invoices, p.failed, p.refunds, amount(p.refunded, c)],
          ...(p.methods.length > 1 ? p.methods : []).map((m) => [p.title, m.title, amount(m.revenue, c), m.stars, m.payments, m.invoices, m.failed, m.refunds, amount(m.refunded, c)]),
        ]),
      ]);
    case 'sources':
      return lines([
        ['Источник', 'Пришли', 'Купили', `Принесли, ${unit}`, 'Звёзды'],
        ...report.sources.map((s) => [sourceName(s.source), s.clients, s.paying, amount(s.revenue, c), s.stars]),
      ]);
  }
}

export function downloadCsv(report: Report, part: CsvPart) {
  const blob = new Blob([buildCsv(report, part)], { type: 'text/csv;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = `analytics-${part}-${report.from}-${report.to}.csv`;
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}
