import { useEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import { groupDigits } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import type { Currency } from '../../lib/types';
import { NoAccess } from '../promo/Promo';
import { AnalyticsPhone } from './AnalyticsPhone';
import { CohortGrid, FlowChart, MoneyChart } from './charts';
import { CSV_PARTS, downloadCsv } from './csv';
import {
  avgCheck,
  monthAt,
  monthName,
  moneyLine,
  PERIODS,
  people,
  percent,
  periodText,
  sum,
  TABS,
  useActive,
  useAnalytics,
  type Analytics as Data,
  type MonthRow,
  type Period,
  type Report,
  type Tab,
} from './model';
import { MonthTable, ProviderTable, SourceTable } from './tables';
import './analytics.css';

export type View = {
  tab: Tab;
  setTab: (tab: Tab) => void;
  active: number;
  setActive: (index: number) => void;
};

export function Analytics() {
  const phone = usePhone();
  const data = useAnalytics();
  const [tab, setTab] = useState<Tab>('money');
  const [active, setActive] = useActive(data.report);
  const view: View = { tab, setTab, active, setActive };
  return phone ? <AnalyticsPhone data={data} view={view} /> : <AnalyticsDesk data={data} view={view} />;
}

export function moneyFacts(row: MonthRow, currency: Currency): Array<[string, string]> {
  return [
    ['Выручка', moneyLine(row.revenue, row.stars, currency)],
    ['MRR', sum(row.mrr, currency)],
    ['Оплат', groupDigits(row.payments)],
    ['Средний чек', row.payments ? sum(avgCheck(row.revenue, row.payments), currency) : '—'],
    ['Возвраты', row.refunds ? sum(row.refunds, currency) : 'нет'],
  ];
}

export function clientFacts(row: MonthRow): Array<[string, string]> {
  const facts: Array<[string, string]> = [
    ['Новые', groupDigits(row.new)],
    ['Продлили', groupDigits(row.renewed)],
    ['Вернулись', groupDigits(row.returned)],
    ['Ушли', groupDigits(row.churned)],
    ['Платящих', groupDigits(row.paying)],
  ];
  if (row.trials) {
    facts.push(['Купили после пробного', `${percent(row.trials_bought, row.trials)} из ${groupDigits(row.trials)}`]);
  }
  return facts;
}

export function moneySummary(report: Report, period: Period): string {
  const t = report.totals;
  const c = report.currency;
  const parts = [`За ${periodText(period, report.months.length)}: ${moneyLine(t.revenue, t.stars, c)}`];
  if (t.payments) {
    parts.push(`средний чек ${sum(avgCheck(t.revenue, t.payments), c)}`);
  }
  if (t.ltv_clients) {
    parts.push(`платящий клиент за всё время принёс в среднем ${sum(t.ltv, c)}`);
  }
  if (t.refunds) {
    parts.push(`вернули ${sum(t.refunds, c)} (${percent(t.refunds, t.revenue + t.refunds)})`);
  }
  return `${parts.join(', ')}.`;
}

export function clientSummary(report: Report, period: Period): string {
  const t = report.totals;
  const head = `За ${periodText(period, report.months.length)} впервые заплатили ${people(t.new)}, вернулись ${groupDigits(t.returned)}, ушли ${groupDigits(t.churned)}. Платящих сейчас ${groupDigits(t.paying)}.`;
  return t.trials ? `${head} Из пробного периода покупают ${percent(t.trials_bought, t.trials)} — ${groupDigits(t.trials_bought)} из ${groupDigits(t.trials)}.` : head;
}

export function isEmpty(report: Report, tab: Tab): boolean {
  const t = report.totals;
  switch (tab) {
    case 'money':
      return t.revenue === 0 && t.stars === 0 && report.months.every((m) => m.mrr === 0);
    case 'clients':
      return report.months.every((m) => !m.new && !m.renewed && !m.returned && !m.churned && !m.paying && !m.trials);
    case 'cohorts':
      return report.cohorts.every((c) => c.size === 0);
    case 'pay':
      return report.providers.length === 0;
    case 'sources':
      return report.sources.length === 0;
  }
}

export const EMPTY_TEXT: Record<Tab, string> = {
  money: 'За этот период оплат не было.',
  clients: 'За этот период платящих клиентов не было.',
  cohorts: 'За этот период никто не покупал впервые.',
  pay: 'За этот период счетов не было.',
  sources: 'За этот период новые клиенты не приходили.',
};

export const NOTES: Record<'cohorts' | 'pay' | 'sources', string> = {
  cohorts: 'Доля клиентов, у которых через 1, 2, 3… месяца после первой покупки есть оплаченная подписка. Пустые клетки — этот срок ещё не наступил.',
  pay: '«Не оплатили» — доля счетов, которые клиенты открыли, но не оплатили. Нажмите на платёжку, чтобы увидеть способы оплаты внутри неё.',
  sources: 'Клиенты, пришедшие за период, по UTM-метке в ссылке. «Принесли» — все их оплаты до сегодняшнего дня.',
};

export function cohortColumns(report: Report): number {
  const used = report.cohorts.reduce((acc, c) => Math.max(acc, c.retained.filter((v) => v !== null).length), 0);
  return Math.max(1, Math.min(12, used));
}

function CsvMenu({ report }: { report: Report }) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) {
      return;
    }
    const close = (event: MouseEvent) => {
      if (box.current && !box.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', key);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', key);
    };
  }, [open]);
  return (
    <div className="an-csv" ref={box}>
      <button type="button" className="b40 hov an-csv-btn" aria-expanded={open} aria-haspopup="menu" onClick={() => setOpen(!open)}>
        <Icon name="arrow-down" size={17} stroke={2.1} />
        CSV
      </button>
      {open ? (
        <div className="an-csv-menu" role="menu">
          {CSV_PARTS.map(([part, label]) => (
            <button
              key={part}
              type="button"
              role="menuitem"
              className="hov"
              onClick={() => {
                downloadCsv(report, part);
                setOpen(false);
              }}
            >
              {label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

export function SourceOptions({ report, full = false }: { report: Report | null; full?: boolean }) {
  const head = full ? 'Клиенты ' : '';
  return (
    <>
      <option value="all">{`${head}из всех источников`}</option>
      <option value="none">{`${head}без метки`}</option>
      {(report?.source_options ?? []).map((s) => (
        <option key={s} value={s}>
          {`${head}из ${s}`}
        </option>
      ))}
    </>
  );
}

export function Off() {
  return (
    <section className="card card-pad an-off">
      <p>Модуль «Бизнес-аналитика» выключен.</p>
      <Link to="/admin/modules" className="b40 hov an-off-link">
        Открыть модули
      </Link>
    </section>
  );
}

function AnalyticsDesk({ data, view: state }: { data: Data; view: View }) {
  const { tab, setTab, active, setActive } = state;
  const [view, setView] = useState<'chart' | 'table'>('chart');

  if (!data.allowed) {
    return <NoAccess title="Аналитика" text="Раздел доступен администраторам с правом «Деньги»." />;
  }

  return (
    <div className="an">
      <header className="an-head">
        <h1 className="page-title tight">Аналитика</h1>
        {data.off ? null : (
          <div className="an-tools">
            <div className="segs" role="group" aria-label="Период">
              {PERIODS.map(([p, label]) => (
                <button key={p} type="button" aria-pressed={data.period === p} onClick={() => data.setPeriod(p)}>
                  {label}
                </button>
              ))}
            </div>
            <label className="an-select">
              <span>Клиенты</span>
              <select value={data.source} onChange={(e) => data.setSource(e.target.value)}>
                <SourceOptions report={data.report} />
              </select>
              <Icon name="chevron-down" size={15} stroke={2.2} />
            </label>
            {data.report ? <CsvMenu report={data.report} /> : null}
          </div>
        )}
      </header>

      {data.off ? (
        <Off />
      ) : (
        <>
          <nav className="an-tabs" aria-label="Разделы аналитики">
            {TABS.map(([key, label]) => (
              <button key={key} type="button" className={tab === key ? 'an-tab on' : 'an-tab'} aria-current={tab === key ? 'page' : undefined} onClick={() => setTab(key)}>
                {label}
              </button>
            ))}
          </nav>
          <section className={data.loading && data.report ? 'card an-pane busy' : 'card an-pane'} aria-busy={data.loading}>
            {data.error ? (
              <div className="an-problem" role="alert">
                <span>{data.error}</span>
                <button type="button" className="b36 hov" onClick={data.reload}>
                  Повторить
                </button>
              </div>
            ) : null}
            {data.report ? (
              <DeskPane data={data} report={data.report} tab={tab} view={view} setView={setView} active={active} setActive={setActive} />
            ) : data.error ? null : (
              <p className="an-wait muted">Считаем…</p>
            )}
          </section>
        </>
      )}
    </div>
  );
}

type PaneProps = {
  data: Data;
  report: Report;
  tab: Tab;
  view: 'chart' | 'table';
  setView: (view: 'chart' | 'table') => void;
  active: number;
  setActive: (index: number) => void;
};

function DeskPane({ data, report, tab, view, setView, active, setActive }: PaneProps) {
  const c = report.currency;
  if (isEmpty(report, tab)) {
    return <p className="an-empty muted">{EMPTY_TEXT[tab]}</p>;
  }
  if (tab === 'money' || tab === 'clients') {
    const row = monthAt(report.months, active);
    return (
      <>
        <div className="an-pane-top">
          {view === 'chart' ? (
            <div className="an-month">
              <span className="an-month-name">{monthName(row.month, true)}</span>
              {row.month === report.current ? <span className="an-month-live">месяц идёт</span> : null}
            </div>
          ) : (
            <div className="an-month">
              <span className="an-month-name">{report.from === report.to ? monthName(report.from, true) : `${monthName(report.from, true)} — ${monthName(report.to, true)}`}</span>
            </div>
          )}
          <div className="segs small" role="group" aria-label="Вид">
            <button type="button" aria-pressed={view === 'chart'} onClick={() => setView('chart')}>
              График
            </button>
            <button type="button" aria-pressed={view === 'table'} onClick={() => setView('table')}>
              Таблица
            </button>
          </div>
        </div>
        {view === 'chart' ? (
          <>
            <dl className="an-facts">
              {(tab === 'money' ? moneyFacts(row, c) : clientFacts(row)).map(([k, v]) => (
                <div key={k}>
                  <dt>{k}</dt>
                  <dd className="num">{v}</dd>
                </div>
              ))}
            </dl>
            {tab === 'money' ? (
              <>
                <div className="an-keys">
                  <span className="an-key">
                    <i className="an-key-bar" /> выручка за месяц
                  </span>
                  <span className="an-key">
                    <i className="an-key-line" /> MRR на конец месяца: подписки в пересчёте на месяц
                  </span>
                </div>
                <MoneyChart months={report.months} currency={c} current={report.current} active={active} onActive={setActive} height={300} />
              </>
            ) : (
              <>
                <div className="an-keys">
                  <span className="an-key">
                    <i className="an-key-bar" /> пришли: новые и вернувшиеся
                  </span>
                  <span className="an-key">
                    <i className="an-key-down" /> ушли
                  </span>
                </div>
                <FlowChart months={report.months} active={active} onActive={setActive} height={280} />
              </>
            )}
          </>
        ) : (
          <MonthTable rows={report.months} currency={c} current={report.current} part={tab} />
        )}
        <p className="an-foot">{tab === 'money' ? moneySummary(report, data.period) : clientSummary(report, data.period)}</p>
      </>
    );
  }
  if (tab === 'cohorts') {
    return (
      <>
        <p className="an-pane-say">{NOTES.cohorts}</p>
        <CohortGrid cohorts={[...report.cohorts].reverse().filter((k) => k.size > 0)} columns={cohortColumns(report)} money={(v) => sum(v, c)} />
      </>
    );
  }
  if (tab === 'pay') {
    return (
      <>
        <p className="an-pane-say">{NOTES.pay}</p>
        <ProviderTable rows={report.providers} currency={c} />
      </>
    );
  }
  return (
    <>
      <p className="an-pane-say">{NOTES.sources}</p>
      <SourceTable rows={report.sources} currency={c} />
    </>
  );
}
