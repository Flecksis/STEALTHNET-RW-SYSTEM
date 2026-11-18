import { useState } from 'react';
import { Icon } from '../../lib/icons';
import { NoAccess } from '../promo/Promo';
import { PhoneHeader } from '../phone';
import { Chips, CompareSelect, CustomDates, FilterFields, Off, PREPARING, PeriodSelect, comparison, rowChange, type Page } from './Analytics';
import { downloadRows, metricRows } from './csv';
import { MetricDetail } from './detail';
import { ListDrawer } from './ListDrawer';
import { format, periodText, TABS } from './model';
import '../home/phone.css';
import './analytics.css';

export function AnalyticsPhone({ page }: { page: Page }) {
  const { tab, setTab, data, metric, select } = page;
  const [detail, setDetail] = useState(false);
  const [filters, setFilters] = useState(false);
  const report = data.report;
  if (!data.allowed) {
    return <NoAccess title="Аналитика" text="Раздел доступен администраторам с правом «Деньги»." />;
  }
  if (detail && metric && report) {
    const sentence = comparison(metric, report);
    return (
      <div className="an anp">
        <header className="ph-head">
          <button type="button" className="ph-back press" onClick={() => setDetail(false)}>
            <Icon name="chevron-left" size={24} stroke={2.2} />
            {TABS.find((t) => t[0] === tab)?.[1]}
          </button>
          <h1 className="ph-title center">{metric.title}</h1>
          <button type="button" className="ph-circ press" aria-label="Скачать CSV" onClick={() => downloadRows(metric.title, metricRows(metric, report))}>
            <span className="ph-dot">
              <Icon name="arrow-down" size={18} stroke={2} />
            </span>
          </button>
        </header>
        <section className="hp-card axp-card">
          <span className="hp-cap">{periodText(report.range.from, report.range.to)}</span>
          {metric.detail.type !== 'cohorts' ? <strong className="axp-big num">{format(metric.value, metric.unit, report.currency)}</strong> : null}
          {sentence ? <span className="axp-vs">{sentence}</span> : metric.note ? <span className="axp-vs">{metric.note}</span> : null}
          {metric.hint && metric.detail.type !== 'cohorts' ? <span className="hp-cap">{metric.hint}</span> : null}
        </section>
        <div className={data.loading ? 'anp-body busy' : 'anp-body'}>
          <MetricDetail metric={metric} report={report} q={page.q} setQ={page.setQ} onList={(ref, title) => page.open({ ref, title })} compact />
        </div>
        <ListDrawer opened={page.opened} q={page.q} currency={report.currency} onClose={() => page.open(null)} phone />
      </div>
    );
  }
  return (
    <div className="an anp">
      <PhoneHeader title="Аналитика" />
      {data.off ? (
        <Off />
      ) : (
        <>
          <div className="axp-tools">
            <PeriodSelect page={page} phone />
            <CompareSelect page={page} phone />
            <button type="button" className="axp-filter" aria-label="Фильтры" aria-expanded={filters} onClick={() => setFilters(true)}>
              <Icon name="sliders" size={18} stroke={2} />
            </button>
          </div>
          <CustomDates page={page} />
          <Chips page={page} />
          <div className="anp-tabs">
            {TABS.map(([key, label]) => (
              <button key={key} type="button" className={tab === key ? 'on' : undefined} onClick={() => setTab(key)}>
                {label}
              </button>
            ))}
          </div>
          {report?.notes?.map((n) => (
            <p key={n} className="anp-note">
              {n}
            </p>
          ))}
          {data.error ? (
            <div className="hp-card ax-problem" role="alert">
              <span>{data.error}</span>
              <button type="button" className="hp-pill press" onClick={data.reload}>
                <span>Повторить</span>
              </button>
            </div>
          ) : null}
          {report?.preparing ? (
            <p className="anp-note">{PREPARING}</p>
          ) : report ? (
            <div className={data.loading ? 'anp-body busy' : 'anp-body'}>
              {report.groups.map((g, gi) => (
                <section key={g.title ?? `g${gi}`} className="hp-group">
                  {g.title ? <h2 className="hp-h2">{g.title}</h2> : null}
                  <div className="hp-card hp-list">
                    {g.metrics.map((m) => (
                      <button
                        key={m.key}
                        type="button"
                        className={m.sub ? 'axp-row sub' : 'axp-row'}
                        onClick={() => {
                          select(m.key);
                          setDetail(true);
                          window.scrollTo(0, 0);
                        }}
                      >
                        <span className="axp-name">{m.title}</span>
                        <span className="axp-right">
                          <span className="axp-val num">{format(m.value, m.unit, report.currency, true)}</span>
                          {rowChange(m) ? <span className="axp-chg">{rowChange(m)}</span> : null}
                        </span>
                        <Icon name="chevron-right" size={16} stroke={2.2} className="chev" />
                      </button>
                    ))}
                  </div>
                </section>
              ))}
            </div>
          ) : !data.error ? (
            <p className="anp-note">Считаем…</p>
          ) : null}
        </>
      )}
      {filters ? (
        <>
          <div className="sheet-back" onClick={() => setFilters(false)} />
          <div className="sheet" role="dialog" aria-label="Фильтры">
            <div className="sheet-grip" />
            <div className="sheet-head">
              <h2 className="dr-title">Фильтры</h2>
            </div>
            <div className="axp-sheet-body">
              <FilterFields page={page} wide />
            </div>
            <div className="sheet-foot">
              <button type="button" onClick={() => page.setQ({ filters: {} })}>
                Сбросить
              </button>
              <button type="button" onClick={() => setFilters(false)}>
                Готово
              </button>
            </div>
          </div>
        </>
      ) : null}
    </div>
  );
}
