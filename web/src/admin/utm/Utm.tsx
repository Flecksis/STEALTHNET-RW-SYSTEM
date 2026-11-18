import { useEffect, useRef, useState } from 'react';
import { Link as RouterLink, useNavigate } from 'react-router';
import { ApiError } from '../../lib/api';
import { dateLong, groupDigits, money } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { QrCode } from '../../lib/qr';
import type { Currency } from '../../lib/types';
import { downloadRows } from '../analytics/csv';
import { PhoneHeader } from '../phone';
import { NoAccess } from '../promo/Promo';
import { useSession } from '../session';
import {
  archive,
  dates,
  draftOf,
  draftOfStat,
  EMPTY,
  FIELDS,
  links,
  param,
  PERIODS,
  save,
  tag,
  tags,
  useLinks,
  type Built,
  type Data,
  type Draft,
  type LinkRow,
  type Period,
  type Stat,
} from './model';
import '../analytics/analytics.css';
import '../home/phone.css';
import './utm.css';

type Mode = { kind: 'link'; id: number } | { kind: 'stray'; index: number } | { kind: 'new'; draft: Draft; id: number | null } | null;

function percent(part: number, whole: number): string {
  if (!whole) {
    return '—';
  }
  const v = Math.round((part / whole) * 1000) / 10;
  return `${String(v).replace('.', ',')} %`;
}

function useCopy(): [string | null, (text: string) => void] {
  const [copied, setCopied] = useState<string | null>(null);
  const copy = (text: string) => {
    void navigator.clipboard?.writeText(text).then(() => {
      setCopied(text);
      window.setTimeout(() => setCopied(null), 1500);
    });
  };
  return [copied, copy];
}

function savePng(holder: HTMLDivElement | null, name: string) {
  const svg = holder?.querySelector('svg');
  if (!svg) {
    return;
  }
  const xml = new XMLSerializer().serializeToString(svg);
  const img = new Image();
  img.onload = () => {
    const size = 600;
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext('2d');
    if (!ctx) {
      return;
    }
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(img, 0, 0, size, size);
    canvas.toBlob((blob) => {
      if (!blob) {
        return;
      }
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = `${name || 'qr'}.png`;
      a.click();
      URL.revokeObjectURL(a.href);
    });
  };
  img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(xml)}`;
}

function LinksBox({ built, title, phone }: { built: Built[]; title: string; phone: boolean }) {
  const [copied, copy] = useCopy();
  const [qr, setQr] = useState(0);
  const holder = useRef<HTMLDivElement | null>(null);
  const shown = built[qr]?.url ?? built.find((b) => b.url)?.url ?? null;
  return (
    <div className={phone ? 'ut-links-box phone' : 'ut-links-box'}>
      <div className="ut-links">
        {built.map((b, i) => (
          <div key={b.name} className="ut-link">
            <span className="ut-ltext">
              <span className="ut-lname">{b.name}</span>
              {b.url ? <span className="ut-url mono">{b.url}</span> : null}
              {b.hint ? <span className="ut-hint">{b.hint}</span> : null}
            </span>
            {b.url ? (
              <span className="ut-lbtn">
                <button type="button" className="b34 hov" onClick={() => copy(b.url as string)}>
                  {copied === b.url ? 'Скопировано' : 'Скопировать'}
                </button>
                <button type="button" className="b34 hov ut-qr-btn" aria-pressed={qr === i} onClick={() => setQr(i)}>
                  QR
                </button>
              </span>
            ) : null}
          </div>
        ))}
      </div>
      {shown ? (
        <div className="ut-qr" ref={holder}>
          <QrCode text={shown} size={phone ? 168 : 132} label={`QR: ${built[qr]?.name ?? ''}`} />
          <span>{`QR: ${(built[qr]?.name ?? '').toLowerCase()}`}</span>
          <button type="button" className="b34 hov" onClick={() => savePng(holder.current, title)}>
            Скачать PNG
          </button>
        </div>
      ) : null}
    </div>
  );
}

function Facts({ s, currency }: { s: Stat; currency: Currency }) {
  const facts: Array<[string, string]> = [
    ['Перешли', groupDigits(s.clicks)],
    ['Регистрации', groupDigits(s.registrations)],
    ['Пробные', groupDigits(s.trials)],
    ['Впервые заплатили', groupDigits(s.paid)],
    ['Конверсия в оплату', percent(s.paid, s.registrations)],
    ['Выручка', money({ minor: s.revenue, currency })],
    ['На платящего', s.paid ? money({ minor: Math.round(s.revenue_all / s.paid), currency }) : '—'],
    ['Платят через месяц', percent(s.kept, s.kept_base)],
  ];
  return (
    <dl className="an-facts ut-facts">
      {facts.map(([k, v]) => (
        <div key={k}>
          <dt>{k}</dt>
          <dd className="num">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

function sentence(s: Stat): string {
  const head = s.clicks ? `Из ${groupDigits(s.clicks)} перешедших зарегистрировались ${groupDigits(s.registrations)} (${percent(s.registrations, s.clicks)})` : `Зарегистрировались ${groupDigits(s.registrations)}`;
  return `${head}, пробный период взяли ${groupDigits(s.trials)}, заплатили ${groupDigits(s.paid)} — ${percent(s.paid, s.registrations)} от регистраций.`;
}

function analyticsLink(s: { source_key: string; campaign: string }): string {
  const src = s.campaign ? `utm:${s.source_key}/${s.campaign}` : `utm:${s.source_key}`;
  return `/admin/m/analytics?tab=sources&source=${encodeURIComponent(src)}`;
}

function Builder({
  data,
  draft,
  setDraft,
  id,
  onSaved,
  onCancel,
  phone,
}: {
  data: Data;
  draft: Draft;
  setDraft: (d: Draft) => void;
  id: number | null;
  onSaved: (id: number) => void;
  onCancel: () => void;
  phone: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [more, setMore] = useState(Boolean(draft.content || draft.term));
  const p = param(draft);
  const built = links(data.page, draft);
  const tooLong = p.length > 64;
  const ready = Boolean(tag(draft.source)) && Boolean(draft.title.trim()) && !tooLong;
  const submit = () => {
    setBusy(true);
    setError(null);
    save(draft, id)
      .then((l) => onSaved(l.id))
      .catch((err: unknown) => setError(err instanceof ApiError ? err.message : 'Не получилось сохранить'))
      .finally(() => setBusy(false));
  };
  const field = ([key, label, cap, ph]: (typeof FIELDS)[number]) => {
    const value = draft[key];
    const cleaned = tag(value);
    return (
      <label key={key} className={phone ? 'ut-pf' : 'ut-f'}>
        <span className="ut-flabel">{label}</span>
        <input className="field mono" value={value} placeholder={ph} onChange={(e) => setDraft({ ...draft, [key]: e.target.value })} />
        {!phone ? <span className="cap">{value && cleaned !== value.trim() ? `в ссылке: ${cleaned || 'пусто'}` : cap}</span> : null}
      </label>
    );
  };
  const visible = phone && !more ? FIELDS.slice(0, 3) : FIELDS;
  return (
    <div className={phone ? 'ut-pbuilder hp-card' : 'ax-detail'}>
      {phone ? null : (
        <div className="ax-dhead">
          <div>
            <h2 className="ax-h2">{id === null ? 'Новая ссылка' : 'Изменить ссылку'}</h2>
            <span className="ax-sub">Латиница, цифры, «-» и «_». Кириллицу переводим в латиницу сами.</span>
          </div>
        </div>
      )}
      <div className={phone ? 'ut-pfields' : 'ut-form'}>
        {visible.map(field)}
        <label className={phone ? 'ut-pf' : 'ut-f wide'}>
          <span className="ut-flabel">Название для себя</span>
          <input className="field" value={draft.title} placeholder="например, Осенние шортсы" onChange={(e) => setDraft({ ...draft, title: e.target.value })} />
          {!phone ? <span className="cap">видно только в панели</span> : null}
        </label>
      </div>
      {phone && !more ? (
        <button type="button" className="ut-more-f" onClick={() => setMore(true)}>
          Объявление и ключевое слово <Icon name="chevron-down" size={14} stroke={2.4} />
        </button>
      ) : null}
      {p ? <LinksBox built={built} title={draft.title || p} phone={phone} /> : null}
      <span className={tooLong ? 'ut-count bad' : 'ut-count'}>
        Параметр для Telegram: <b className="num">{p.length}</b> из 64 знаков
        {tooLong ? '. Длиннее Telegram не примет — сократите кампанию или уберите объявление.' : ''}
      </span>
      {error ? (
        <p className="ut-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="ut-actions">
        <button type="button" className="btn-main" disabled={!ready || busy} onClick={submit}>
          {busy ? 'Сохраняем…' : 'Сохранить ссылку'}
        </button>
        <button type="button" className="btn-line" onClick={onCancel}>
          Отмена
        </button>
      </div>
    </div>
  );
}

function LinkDetail({ row, data, currency, onEdit, onArchived, phone }: { row: LinkRow; data: Data; currency: Currency; onEdit: () => void; onArchived: () => void; phone: boolean }) {
  const navigate = useNavigate();
  const [busy, setBusy] = useState(false);
  const l = row.link;
  const built = links(data.page, draftOf(l));
  return (
    <div className={phone ? 'ut-pdetail' : 'ax-detail'}>
      <div className="ax-dhead">
        <div>
          <h2 className="ax-h2">{l.title}</h2>
          <span className="ax-sub">
            <span className="mono">{tags([l.source, l.medium, l.campaign, l.content, l.term])}</span> · создана {dateLong(l.created_at)}
          </span>
        </div>
        <div className="ut-head-btns">
          <button type="button" className="b36 hov" onClick={onEdit}>
            Изменить
          </button>
          <button
            type="button"
            className="b36 hov"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void archive(l.id)
                .then(onArchived)
                .finally(() => setBusy(false));
            }}
          >
            В архив
          </button>
        </div>
      </div>
      <LinksBox built={built} title={l.title} phone={phone} />
      <div className="ut-period">
        <h3 className="ax-h3">{data.page ? `${dateLong(data.page.from)} — ${dateLong(data.page.to)}` : 'За период'}</h3>
        <Facts s={row.stats} currency={currency} />
        <p className="ut-say">{sentence(row.stats)}</p>
      </div>
      <p className="ax-note muted">
        Перешли — нажали «Старт» в боте, открыли мини-приложение или сайт по этой ссылке; считаются с обновления панели. Выручка — оплаты этих клиентов за период, как в «Аналитика → Источники».{' '}
        <button type="button" className="ut-more" onClick={() => navigate(analyticsLink(row.stats))}>
          Подробно по дням, удержание и LTV — в аналитике <Icon name="chevron-right" size={14} stroke={2.4} />
        </button>
      </p>
    </div>
  );
}

function StrayDetail({ s, currency, onMake, phone }: { s: Stat; currency: Currency; onMake: () => void; phone: boolean }) {
  return (
    <div className={phone ? 'ut-pdetail' : 'ax-detail'}>
      <div className="ax-dhead">
        <div>
          <h2 className="ax-h2 mono">{tags([s.source_key, s.medium, s.campaign]) || 'Метка без источника'}</h2>
          <span className="ax-sub">Ссылку сделали без конструктора — например, вручную в рекламном кабинете</span>
        </div>
        <div className="ut-head-btns">
          <button type="button" className="b36 hov" onClick={onMake}>
            Сделать ссылку
          </button>
        </div>
      </div>
      <div className="ut-period">
        <Facts s={s} currency={currency} />
        <p className="ut-say">{sentence(s)}</p>
      </div>
    </div>
  );
}

function csv(data: Data, currency: Currency) {
  const page = data.page;
  if (!page) {
    return;
  }
  const money = (minor: number) => (currency === 'RUB' ? String(Math.round(minor / 100)) : (minor / 100).toFixed(2).replace('.', ','));
  const row = (title: string, s: Stat) => [title, s.source_key, s.medium, s.campaign, s.clicks, s.registrations, s.trials, s.paid, money(s.revenue), money(s.revenue_all)];
  downloadRows(`UTM-метки ${page.from} ${page.to}`, [
    ['Ссылка', 'Источник', 'Канал', 'Кампания', 'Перешли', 'Регистрации', 'Пробные', 'Заплатили', 'Выручка за период', 'За всё время'],
    ...page.links.map((r) => row(r.link.title, r.stats)),
    ...page.stray.map((s) => row('не из конструктора', s)),
  ]);
}

export function Utm() {
  const phone = usePhone();
  const { me } = useSession();
  const currency = (me.currency ?? 'RUB') as Currency;
  const [period, setPeriod] = useState<Period>('month');
  const data = useLinks(period);
  const [mode, setMode] = useState<Mode>(null);
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const page = data.page;
  useEffect(() => {
    if (mode === null && page && page.links.length && !phone) {
      const first = page.links[0];
      if (first) {
        setMode({ kind: 'link', id: first.link.id });
      }
    }
  }, [page, mode, phone]);
  if (!data.allowed) {
    return <NoAccess title="UTM-метки" text="Раздел доступен администраторам с правом «Маркетинг»." />;
  }
  if (data.off) {
    return (
      <section className="card card-pad an-off">
        <p>Модуль «UTM-метки» выключен.</p>
        <RouterLink to="/admin/modules" className="b40 hov an-off-link">
          Открыть модули
        </RouterLink>
      </section>
    );
  }
  const startNew = (d: Draft = EMPTY, id: number | null = null) => {
    setDraft(d);
    setMode({ kind: 'new', draft: d, id });
  };
  const selectedLink = mode?.kind === 'link' ? page?.links.find((r) => r.link.id === mode.id) : undefined;
  const selectedStray = mode?.kind === 'stray' ? page?.stray[mode.index] : undefined;
  const right = () => {
    if (mode?.kind === 'new') {
      return (
        <Builder
          data={data}
          draft={draft}
          setDraft={setDraft}
          id={mode.id}
          phone={phone}
          onCancel={() => setMode(null)}
          onSaved={(id) => {
            setMode({ kind: 'link', id });
            data.reload();
          }}
        />
      );
    }
    if (selectedLink) {
      return (
        <LinkDetail
          row={selectedLink}
          data={data}
          currency={currency}
          phone={phone}
          onEdit={() => startNew(draftOf(selectedLink.link), selectedLink.link.id)}
          onArchived={() => {
            setMode(null);
            data.reload();
          }}
        />
      );
    }
    if (selectedStray) {
      return <StrayDetail s={selectedStray} currency={currency} phone={phone} onMake={() => startNew(draftOfStat(selectedStray))} />;
    }
    return phone ? null : (
      <div className="ax-detail">
        <p className="muted">{page?.links.length ? 'Выберите ссылку слева.' : 'Ссылок ещё нет. Сделайте первую — кнопка «Новая ссылка» справа вверху.'}</p>
      </div>
    );
  };
  const sources = Array.from(new Set((page?.links ?? []).map((r) => r.link.source)));
  const list = (
    <>
      {sources.map((src) => {
        const rows = (page?.links ?? []).filter((r) => r.link.source === src);
        const regs = rows.reduce((acc, r) => acc + r.stats.registrations, 0);
        return phone ? (
          <section key={src} className="hp-group">
            <h2 className="hp-h2">{src}</h2>
            <div className="hp-card hp-list">
              {rows.map((r) => (
                <button key={r.link.id} type="button" className="axp-row" onClick={() => setMode({ kind: 'link', id: r.link.id })}>
                  <span className="ut-ptext">
                    <span className="axp-name">{r.link.title}</span>
                    <span className="ut-tags">{tags([r.link.medium, r.link.campaign])}</span>
                  </span>
                  <span className="axp-right">
                    <span className="axp-val num">{groupDigits(r.stats.registrations)} рег.</span>
                    <span className="axp-chg">
                      {money({ minor: r.stats.revenue, currency })} · {percent(r.stats.paid, r.stats.registrations)}
                    </span>
                  </span>
                  <Icon name="chevron-right" size={16} stroke={2.2} className="chev" />
                </button>
              ))}
            </div>
          </section>
        ) : (
          <div key={src} className="ut-group">
            <div className="ax-cap ut-cap">
              <span>{src}</span>
              <span className="num">{groupDigits(regs)}</span>
            </div>
            {rows.map((r) => (
              <button key={r.link.id} type="button" className={`ut-row${selectedLink?.link.id === r.link.id ? ' on' : ''}`} onClick={() => setMode({ kind: 'link', id: r.link.id })}>
                <span className="ut-title">{r.link.title}</span>
                <span className="ut-num num">{groupDigits(r.stats.registrations)}</span>
                <span className="ut-tags">{tags([r.link.medium, r.link.campaign]) || 'без кампании'}</span>
                <span className="ut-sub num">{money({ minor: r.stats.revenue, currency })}</span>
              </button>
            ))}
          </div>
        );
      })}
      {page?.stray.length ? (
        phone ? (
          <section className="hp-group">
            <h2 className="hp-h2">Метки не из конструктора</h2>
            <div className="hp-card hp-list">
              {page.stray.map((s, i) => (
                <button key={`${s.source_key}/${s.medium}/${s.campaign}`} type="button" className="axp-row" onClick={() => setMode({ kind: 'stray', index: i })}>
                  <span className="ut-ptext">
                    <span className="axp-name">{tags([s.source_key, s.medium, s.campaign]) || 'без источника'}</span>
                    <span className="ut-tags">ссылку сделали вручную</span>
                  </span>
                  <span className="axp-right">
                    <span className="axp-val num">{groupDigits(s.registrations)} рег.</span>
                    <span className="axp-chg">{money({ minor: s.revenue, currency })}</span>
                  </span>
                  <Icon name="chevron-right" size={16} stroke={2.2} className="chev" />
                </button>
              ))}
            </div>
          </section>
        ) : (
          <div className="ut-group">
            <div className="ax-cap ut-cap">
              <span>Метки не из конструктора</span>
              <span className="num">{groupDigits(page.stray.reduce((a, s) => a + s.registrations, 0))}</span>
            </div>
            {page.stray.map((s, i) => (
              <button
                key={`${s.source_key}/${s.medium}/${s.campaign}`}
                type="button"
                className={`ut-row${mode?.kind === 'stray' && mode.index === i ? ' on' : ''}`}
                onClick={() => setMode({ kind: 'stray', index: i })}
              >
                <span className="ut-title">{tags([s.source_key, s.medium, s.campaign]) || 'без источника'}</span>
                <span className="ut-num num">{groupDigits(s.registrations)}</span>
                <span className="ut-tags">ссылку сделали вручную</span>
                <span className="ut-sub num">{money({ minor: s.revenue, currency })}</span>
              </button>
            ))}
          </div>
        )
      ) : null}
    </>
  );
  const periodSelect = phone ? (
    <label className="anp-chip">
      <select value={period} onChange={(e) => setPeriod(e.target.value as Period)}>
        {PERIODS.map(([p, label]) => (
          <option key={p} value={p}>
            {label}
          </option>
        ))}
      </select>
      <Icon name="chevron-down" size={16} stroke={2.2} />
    </label>
  ) : (
    <label className="an-select">
      <span>Период</span>
      <select value={period} onChange={(e) => setPeriod(e.target.value as Period)}>
        {PERIODS.map(([p, label]) => (
          <option key={p} value={p}>
            {label}
          </option>
        ))}
      </select>
      <Icon name="chevron-down" size={15} stroke={2.2} />
    </label>
  );
  const notice = page?.preparing ? 'Считаем цифры в первый раз — займёт пару минут, страница обновится сама.' : data.error;
  if (phone) {
    if (mode) {
      const title = mode.kind === 'new' ? (mode.id === null ? 'Новая ссылка' : 'Изменить ссылку') : mode.kind === 'link' ? (selectedLink?.link.title ?? '') : 'Метка';
      return (
        <div className="an anp">
          <header className="ph-head">
            <button type="button" className="ph-back press" onClick={() => setMode(null)}>
              <Icon name="chevron-left" size={24} stroke={2.2} />
              Метки
            </button>
            <h1 className="ph-title center">{title}</h1>
            <span style={{ width: 72 }} />
          </header>
          {right()}
        </div>
      );
    }
    return (
      <div className="an anp">
        <PhoneHeader title="UTM-метки" />
        <div className="axp-tools">
          {periodSelect}
          <button type="button" className="ut-pnew" onClick={() => startNew()}>
            Новая ссылка
          </button>
        </div>
        {notice ? <p className="anp-note">{notice}</p> : <p className="anp-note">Регистрации, выручка и доля заплативших за период.</p>}
        {list}
        {!page?.links.length && !page?.stray.length && !data.loading ? <p className="anp-note">Ссылок ещё нет.</p> : null}
      </div>
    );
  }
  return (
    <div className="an">
      <header className="an-head">
        <h1 className="page-title tight">UTM-метки</h1>
        <div className="an-tools">
          {periodSelect}
          <button type="button" className="b40 hov an-csv-btn" onClick={() => csv(data, currency)} disabled={!page}>
            <Icon name="arrow-down" size={17} stroke={2.1} />
            CSV
          </button>
          <button type="button" className="btn-main ut-new" onClick={() => startNew()}>
            Новая ссылка
          </button>
        </div>
      </header>
      {notice ? <p className="ax-note muted">{notice}</p> : null}
      <section className={data.loading && page ? 'card ax-pane ut-pane busy' : 'card ax-pane ut-pane'}>
        <div className="ut-list">
          {mode?.kind === 'new' ? (
            <div className="ut-row on">
              <span className="ut-title">{mode.id === null ? 'Новая ссылка' : 'Изменение ссылки'}</span>
              <span className="ut-num">—</span>
              <span className="ut-tags">заполняется справа</span>
              <span className="ut-sub" />
            </div>
          ) : null}
          {list}
          <p className="ut-list-note">{`Числа — регистрации и выручка за ${dateLong(dates(period).from)} — ${dateLong(dates(period).to)}`}</p>
        </div>
        {right()}
      </section>
    </div>
  );
}
