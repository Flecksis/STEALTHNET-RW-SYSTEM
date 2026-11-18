import { Fragment, useState } from 'react';
import { groupDigits } from '../../lib/format';
import { Icon } from '../../lib/icons';
import type { Currency } from '../../lib/types';
import { ShareLine } from './charts';
import { avgCheck, moneyLine, monthName, percent, share, sourceName, stars, sum, type MethodRow, type MonthRow, type ProviderRow, type SourceRow } from './model';

function moneyCell(row: MethodRow, currency: Currency): string {
  return moneyLine(row.revenue, row.stars, currency);
}

function failRate(row: MethodRow): string {
  return row.invoices > 0 ? percent(row.failed, row.invoices) : '—';
}

function refundText(row: MethodRow, currency: Currency): string {
  if (!row.refunds) {
    return 'нет';
  }
  return row.refunded > 0 ? `${groupDigits(row.refunds)} на ${sum(row.refunded, currency)}` : groupDigits(row.refunds);
}

type ProvidersProps = { rows: ProviderRow[]; currency: Currency };

export function ProviderTable({ rows, currency }: ProvidersProps) {
  const [open, setOpen] = useState<string | null>(null);
  const whole = rows.reduce((acc, r) => acc + r.revenue, 0);
  return (
    <div className="an-tw">
      <table className="an-t">
        <thead>
          <tr>
            <th>Платёжка</th>
            <th className="r">Оборот</th>
            <th className="an-t-share">Доля</th>
            <th className="r">Оплат</th>
            <th className="r" title="Счета, которые выставили, но не оплатили">
              Не оплатили
            </th>
            <th className="r">Возвраты</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => {
            const part = share(row.revenue, whole);
            const expandable = row.methods.length > 1;
            const isOpen = open === row.provider;
            return (
              <Fragment key={row.provider}>
                <tr className={isOpen ? 'open' : undefined}>
                  <td>
                    {expandable ? (
                      <button type="button" className="an-exp" aria-expanded={isOpen} onClick={() => setOpen(isOpen ? null : row.provider)}>
                        <Icon name="chevron-right" size={15} stroke={2.2} />
                        {row.title}
                      </button>
                    ) : (
                      <span className="an-exp-pad">{row.title}</span>
                    )}
                  </td>
                  <td className="r num strong">{moneyCell(row, currency)}</td>
                  <td className="an-t-share">
                    {row.revenue > 0 ? (
                      <span className="an-share-row">
                        <ShareLine value={part} />
                        <span className="num">{percent(row.revenue, whole)}</span>
                      </span>
                    ) : (
                      <span className="muted">в звёздах</span>
                    )}
                  </td>
                  <td className="r num">{groupDigits(row.payments)}</td>
                  <td className="r num">{failRate(row)}</td>
                  <td className="r num">{refundText(row, currency)}</td>
                </tr>
                {isOpen
                  ? row.methods.map((m) => (
                      <tr key={`${row.provider}:${m.code ?? ''}`} className="an-sub">
                        <td>
                          <span className="an-exp-pad sub">{m.title}</span>
                        </td>
                        <td className="r num">{moneyCell(m, currency)}</td>
                        <td className="an-t-share">
                          {m.revenue > 0 ? (
                            <span className="an-share-row">
                              <ShareLine value={share(m.revenue, whole)} />
                              <span className="num">{percent(m.revenue, whole)}</span>
                            </span>
                          ) : null}
                        </td>
                        <td className="r num">{groupDigits(m.payments)}</td>
                        <td className="r num">{failRate(m)}</td>
                        <td className="r num">{refundText(m, currency)}</td>
                      </tr>
                    ))
                  : null}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

type SourcesProps = { rows: SourceRow[]; currency: Currency };

export function SourceTable({ rows, currency }: SourcesProps) {
  const top = Math.max(1, ...rows.map((r) => r.revenue));
  return (
    <div className="an-tw">
      <table className="an-t">
        <thead>
          <tr>
            <th>Источник</th>
            <th className="r">Пришли</th>
            <th className="r">Купили</th>
            <th className="r">Конверсия</th>
            <th className="r">Принесли</th>
            <th className="r">На клиента</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.source ?? ''}>
              <td>
                <span className={row.source ? 'an-src' : 'an-src none'}>{sourceName(row.source)}</span>
              </td>
              <td className="r num">{groupDigits(row.clients)}</td>
              <td className="r num">{groupDigits(row.paying)}</td>
              <td className="r num">{percent(row.paying, row.clients)}</td>
              <td className="r num strong">
                <span className="an-share-row end">
                  <ShareLine value={share(row.revenue, top)} />
                  {moneyLine(row.revenue, row.stars, currency)}
                </span>
              </td>
              <td className="r num">{row.paying ? sum(avgCheck(row.revenue, row.paying), currency) : '—'}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

type MonthsProps = { rows: MonthRow[]; currency: Currency; current: string; part: 'money' | 'clients' };

export function MonthTable({ rows, currency, current, part }: MonthsProps) {
  const list = [...rows].reverse();
  return (
    <div className="an-tw">
      <table className="an-t">
        <thead>
          {part === 'money' ? (
            <tr>
              <th>Месяц</th>
              <th className="r">Выручка</th>
              <th className="r">Звёзды</th>
              <th className="r">MRR</th>
              <th className="r">Оплат</th>
              <th className="r">Средний чек</th>
              <th className="r">Возвраты</th>
            </tr>
          ) : (
            <tr>
              <th>Месяц</th>
              <th className="r">Новые</th>
              <th className="r">Продлили</th>
              <th className="r">Вернулись</th>
              <th className="r">Ушли</th>
              <th className="r">Платящих</th>
              <th className="r">Пробный → покупка</th>
            </tr>
          )}
        </thead>
        <tbody>
          {list.map((row) => (
            <tr key={row.month}>
              <td>
                {monthName(row.month, true)}
                {row.month === current ? <span className="muted"> · идёт</span> : null}
              </td>
              {part === 'money' ? (
                <>
                  <td className="r num strong">{sum(row.revenue, currency)}</td>
                  <td className="r num">{row.stars ? stars(row.stars) : '—'}</td>
                  <td className="r num">{sum(row.mrr, currency)}</td>
                  <td className="r num">{groupDigits(row.payments)}</td>
                  <td className="r num">{row.payments ? sum(avgCheck(row.revenue, row.payments), currency) : '—'}</td>
                  <td className="r num">{row.refunds ? sum(row.refunds, currency) : '—'}</td>
                </>
              ) : (
                <>
                  <td className="r num strong">{groupDigits(row.new)}</td>
                  <td className="r num">{groupDigits(row.renewed)}</td>
                  <td className="r num">{groupDigits(row.returned)}</td>
                  <td className="r num">{groupDigits(row.churned)}</td>
                  <td className="r num">{groupDigits(row.paying)}</td>
                  <td className="r num">{row.trials ? `${percent(row.trials_bought, row.trials)} из ${groupDigits(row.trials)}` : '—'}</td>
                </>
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
