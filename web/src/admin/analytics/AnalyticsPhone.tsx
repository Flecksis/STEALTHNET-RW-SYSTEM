import { Fragment, useState } from 'react';
import { Link } from 'react-router';
import { groupDigits } from '../../lib/format';
import { Icon } from '../../lib/icons';
import type { Currency } from '../../lib/types';
import { PhoneHeader } from '../phone';
import { clientFacts, clientSummary, cohortColumns, EMPTY_TEXT, isEmpty, moneyFacts, moneySummary, NOTES, SourceOptions, type View } from './Analytics';
import { CohortGrid, FlowChart, MoneyChart, ShareLine } from './charts';
import { CSV_PARTS, downloadCsv } from './csv';
import {
  avgCheck,
  monthAt,
  monthName,
  PERIODS,
  percent,
  share,
  sourceName,
  stars,
  sum,
  TABS,
  type Analytics as Data,
  type MonthRow,
  type ProviderRow,
  type Report,
  type SourceRow,
} from './model';
import '../home/phone.css';
import './analytics.css';

function lead(row: { revenue: number; stars: number }, currency: Currency): string {
  return row.revenue === 0 && row.stars > 0 ? stars(row.stars) : sum(row.revenue, currency);
}

function extra(row: { revenue: number; stars: number }): string {
  return row.revenue > 0 && row.stars > 0 ? `и ${stars(row.stars)} · ` : '';
}

function ProviderList({ rows, currency }: { rows: ProviderRow[]; currency: Currency }) {
  const [open, setOpen] = useState<string | null>(null);
  const whole = rows.reduce((acc, r) => acc + r.revenue, 0);
  return (
    <div className="hp-card hp-list">
      {rows.map((row) => {
        const many = row.methods.length > 1;
        const isOpen = open === row.provider;
        const body = (
          <>
            <span className="anp-row-main">
              <span className="anp-row-top">
                <span className="anp-row-name">{row.title}</span>
                <span className="num anp-row-sum">{lead(row, currency)}</span>
              </span>
              {row.revenue > 0 ? <ShareLine value={share(row.revenue, whole)} /> : null}
              <span className="anp-row-sub">
                {extra(row)}
                {row.revenue > 0 ? `${percent(row.revenue, whole)} оборота · ` : ''}
                {groupDigits(row.payments)} оплат · не оплатили {row.invoices ? percent(row.failed, row.invoices) : '—'}
                {row.refunds ? ` · возвратов ${groupDigits(row.refunds)}` : ''}
              </span>
            </span>
            {many ? <Icon name={isOpen ? 'chevron-down' : 'chevron-right'} size={17} stroke={2.2} className="anp-chev" /> : null}
          </>
        );
        return (
          <Fragment key={row.provider}>
            {many ? (
              <button type="button" className="anp-row" aria-expanded={isOpen} onClick={() => setOpen(isOpen ? null : row.provider)}>
                {body}
              </button>
            ) : (
              <div className="anp-row">{body}</div>
            )}
            {isOpen
              ? row.methods.map((m) => (
                  <div key={m.code ?? ''} className="anp-row sub">
                    <span className="anp-row-main">
                      <span className="anp-row-top">
                        <span>{m.title}</span>
                        <span className="num">{lead(m, currency)}</span>
                      </span>
                      <span className="anp-row-sub">
                        {extra(m)}
                        {groupDigits(m.payments)} оплат · не оплатили {m.invoices ? percent(m.failed, m.invoices) : '—'}
                      </span>
                    </span>
                  </div>
                ))
              : null}
          </Fragment>
        );
      })}
    </div>
  );
}

function SourceList({ rows, currency }: { rows: SourceRow[]; currency: Currency }) {
  const top = Math.max(1, ...rows.map((r) => r.revenue));
  return (
    <div className="hp-card hp-list">
      {rows.map((row) => (
        <div key={row.source ?? ''} className="anp-row">
          <span className="anp-row-main">
            <span className="anp-row-top">
              <span className={row.source ? 'anp-row-name' : 'anp-row-name muted'}>{sourceName(row.source)}</span>
              <span className="num anp-row-sum">{lead(row, currency)}</span>
            </span>
            <ShareLine value={share(row.revenue, top)} />
            <span className="anp-row-sub">
              {extra(row)}
              пришли {groupDigits(row.clients)} · купили {groupDigits(row.paying)} ({percent(row.paying, row.clients)})
            </span>
          </span>
        </div>
      ))}
    </div>
  );
}

function MonthList({ rows, currency, current, part }: { rows: MonthRow[]; currency: Currency; current: string; part: 'money' | 'clients' }) {
  return (
    <div className="hp-card hp-list">
      {[...rows].reverse().map((row) => (
        <div key={row.month} className="anp-row">
          <span className="anp-row-main">
            <span className="anp-row-top">
              <span className="anp-row-name">
                {monthName(row.month, true)}
                {row.month === current ? <span className="muted"> · идёт</span> : null}
              </span>
              <span className="num anp-row-sum">{part === 'money' ? sum(row.revenue, currency) : `+${groupDigits(row.new + row.returned)}`}</span>
            </span>
            <span className="anp-row-sub">
              {part === 'money'
                ? `MRR ${sum(row.mrr, currency)} · ${groupDigits(row.payments)} оплат${row.payments ? ` · чек ${sum(avgCheck(row.revenue, row.payments), currency)}` : ''}`
                : `новые ${groupDigits(row.new)} · продлили ${groupDigits(row.renewed)} · ушли ${groupDigits(row.churned)} · платящих ${groupDigits(row.paying)}`}
            </span>
          </span>
        </div>
      ))}
    </div>
  );
}

function CsvList({ report }: { report: Report }) {
  return (
    <div className="hp-group">
      <h2 className="hp-h2">Скачать CSV</h2>
      <div className="hp-card hp-list">
        {CSV_PARTS.map(([part, label]) => (
          <button key={part} type="button" className="anp-row link" onClick={() => downloadCsv(report, part)}>
            <span className="anp-row-main">{label}</span>
            <Icon name="arrow-down" size={17} stroke={2.1} className="anp-chev" />
          </button>
        ))}
      </div>
    </div>
  );
}

export function AnalyticsPhone({ data, view: state }: { data: Data; view: View }) {
  const { tab, setTab, active, setActive } = state;
  const [view, setView] = useState<'chart' | 'list'>('chart');
  const report = data.report;

  if (!data.allowed) {
    return (
      <div className="hp">
        <PhoneHeader title="Аналитика" />
        <p className="hp-quiet hp-card">Раздел доступен администраторам с правом «Деньги».</p>
      </div>
    );
  }
  if (data.off) {
    return (
      <div className="hp">
        <PhoneHeader title="Аналитика" />
        <div className="hp-card anp-pane">
          <p>Модуль «Бизнес-аналитика» выключен.</p>
          <Link to="/admin/modules" className="anp-switch">
            Открыть модули
          </Link>
        </div>
      </div>
    );
  }

  const c = report?.currency ?? 'RUB';
  const row = report ? monthAt(report.months, active) : null;
  const empty = report ? isEmpty(report, tab) : false;

  return (
    <div className="hp anp">
      <PhoneHeader title="Аналитика" />
      <div className="segs anp-segs" role="group" aria-label="Период">
        {PERIODS.map(([p, label]) => (
          <button key={p} type="button" aria-pressed={data.period === p} onClick={() => data.setPeriod(p)}>
            {label}
          </button>
        ))}
      </div>
      <label className="anp-chip wide">
        <span className="sr-only">Источник</span>
        <select value={data.source} onChange={(e) => data.setSource(e.target.value)}>
          <SourceOptions report={report} full />
        </select>
        <Icon name="chevron-down" size={14} stroke={2.4} />
      </label>
      <nav className="anp-tabs" aria-label="Разделы аналитики">
        {TABS.map(([key, label]) => (
          <button key={key} type="button" className={tab === key ? 'on' : undefined} aria-current={tab === key ? 'page' : undefined} onClick={() => setTab(key)}>
            {label}
          </button>
        ))}
      </nav>
      {data.error ? (
        <div className="hp-card anp-pane" role="alert">
          <p>{data.error}</p>
          <button type="button" className="anp-switch" onClick={data.reload}>
            Повторить
          </button>
        </div>
      ) : null}
      {!report ? data.error ? null : <p className="hp-quiet hp-card">Считаем…</p> : null}
      {report && empty ? <p className="hp-quiet hp-card">{EMPTY_TEXT[tab]}</p> : null}
      <div className={data.loading && report ? 'anp-body busy' : 'anp-body'}>
        {report && row && !empty && (tab === 'money' || tab === 'clients') ? (
          <>
            <div className="hp-card anp-pane">
              <div className="anp-pane-top">
                <span className="anp-month">{monthName(row.month, true)}</span>
                <button type="button" className="anp-switch" onClick={() => setView(view === 'chart' ? 'list' : 'chart')}>
                  {view === 'chart' ? 'Списком' : 'Графиком'}
                </button>
              </div>
              {view === 'chart' ? (
                <>
                  <dl className="hp-stats">
                    {(tab === 'money' ? moneyFacts(row, c) : clientFacts(row)).map(([k, v]) => (
                      <div key={k}>
                        <dt>{k}</dt>
                        <dd className="num">{v}</dd>
                      </div>
                    ))}
                  </dl>
                  {tab === 'money' ? (
                    <MoneyChart months={report.months} currency={c} current={report.current} active={active} onActive={setActive} height={190} />
                  ) : (
                    <FlowChart months={report.months} active={active} onActive={setActive} height={150} />
                  )}
                </>
              ) : null}
              <p className="anp-foot">{tab === 'money' ? moneySummary(report, data.period) : clientSummary(report, data.period)}</p>
            </div>
            {view === 'list' ? <MonthList rows={report.months} currency={c} current={report.current} part={tab} /> : null}
          </>
        ) : null}
        {report && !empty && tab === 'cohorts' ? (
          <div className="hp-group">
            <p className="anp-note">{NOTES.cohorts}</p>
            <div className="hp-card anp-coh">
              <CohortGrid cohorts={[...report.cohorts].reverse().filter((k) => k.size > 0)} columns={cohortColumns(report)} />
            </div>
          </div>
        ) : null}
        {report && !empty && tab === 'pay' ? (
          <div className="hp-group">
            <p className="anp-note">{NOTES.pay}</p>
            <ProviderList rows={report.providers} currency={c} />
          </div>
        ) : null}
        {report && !empty && tab === 'sources' ? (
          <div className="hp-group">
            <p className="anp-note">{NOTES.sources}</p>
            <SourceList rows={report.sources} currency={c} />
          </div>
        ) : null}
      </div>
      {report ? <CsvList report={report} /> : null}
    </div>
  );
}
