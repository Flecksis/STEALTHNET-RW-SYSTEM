import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { dateLong } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { NoAccess } from '../promo/Promo';
import { emptyText, FILTERS, giftText, paidText, stateOf, summaryText, term, who, type AdminGift } from './model';
import { useGifts, useGiftSettings } from './useGifts';
import '../form.css';
import '../home/phone.css';
import '../promo/promo.css';
import './gifts.css';

export function Gifts() {
  const phone = usePhone();
  return phone ? <GiftsPhone /> : <GiftsDesk />;
}

function Person({ id, text }: { id: number | null; text: string }) {
  if (!id || !text) {
    return <span className="pr-sub">—</span>;
  }
  return (
    <Link to={`/admin/clients/${id}`} className="gf-who">
      {text}
    </Link>
  );
}

function Settings({ phone, onDone }: { phone?: boolean; onDone: (text: string) => void }) {
  const data = useGiftSettings(true);
  const [mode, setMode] = useState<'all' | 'some'>('all');
  const [picked, setPicked] = useState<number[]>([]);
  const [days, setDays] = useState('');
  const [fieldError, setFieldError] = useState<string | null>(null);

  useEffect(() => {
    if (!data.view) {
      return;
    }
    setMode(data.view.tariffs.length ? 'some' : 'all');
    setPicked(data.view.tariffs);
    setDays(String(data.view.code_days));
  }, [data.view]);

  if (!data.view) {
    return data.error ? (
      <p className="err" role="alert">
        {data.error}
      </p>
    ) : (
      <p className="muted">Загружаем настройки…</p>
    );
  }

  const tariffs = data.view.available;

  const submit = async () => {
    const raw = days.trim();
    const value = raw === '' ? 0 : Number(raw);
    if (!/^\d{1,4}$/.test(raw || '0') || value > 3650) {
      setFieldError('Срок действия кода — от 0 до 3650 дней');
      return;
    }
    if (mode === 'some' && picked.length === 0) {
      setFieldError('Отметьте хотя бы один тариф или выберите «Все тарифы в продаже»');
      return;
    }
    setFieldError(null);
    const ok = await data.save(mode === 'all' ? [] : picked, value);
    if (ok) {
      onDone(value === 0 ? 'Настройки сохранены: код подарка действует без срока.' : `Настройки сохранены: новые коды действуют ${term(value)}.`);
    }
  };

  return (
    <div className={phone ? 'pf pf-phone' : 'pf'}>
      <fieldset className="pf-tariffs">
        <legend className="pf-label">Какие тарифы можно дарить</legend>
        <div role="group" aria-label="Какие тарифы можно дарить" className={phone ? 'pf-psegs' : 'segs gf-mode'}>
          <button type="button" aria-pressed={mode === 'all'} onClick={() => setMode('all')}>
            Все тарифы в продаже
          </button>
          <button type="button" aria-pressed={mode === 'some'} onClick={() => setMode('some')}>
            Только выбранные
          </button>
        </div>
        {mode === 'some' ? (
          <div className="pf-chips">
            {tariffs.map((t) => (
              <label key={t.id} className="pf-chip">
                <input type="checkbox" checked={picked.includes(t.id)} onChange={(e) => setPicked(e.target.checked ? [...picked, t.id] : picked.filter((id) => id !== t.id))} />
                <span>
                  {t.name}
                  {t.giftable ? null : <span className="cap"> · не продаётся</span>}
                </span>
              </label>
            ))}
          </div>
        ) : (
          <p className="cap">Дарить можно любой тариф, который сейчас в продаже, на любой его срок.</p>
        )}
      </fieldset>
      <label className="pf-field gf-days">
        <span className="pf-label">Срок действия кода, дней</span>
        <input className="field num" inputMode="numeric" value={days} onChange={(e) => setDays(e.target.value.replace(/\D/g, ''))} maxLength={4} />
        <span className="cap">0 — код действует без срока. Новый срок касается только новых подарков.</span>
      </label>
      {fieldError || data.error ? (
        <p className="err" role="alert">
          {fieldError ?? data.error}
        </p>
      ) : null}
      <div className="pf-actions">
        <button type="button" className="btn-main btn" onClick={() => void submit()} disabled={data.busy}>
          {data.busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
      </div>
    </div>
  );
}

function GiftsDesk() {
  const data = useGifts();
  const [settings, setSettings] = useState(false);
  const [done, setDone] = useState<string | null>(null);

  if (!data.allowed) {
    return <NoAccess title="Подарки" text="Раздел доступен администраторам с правом «Маркетинг»." />;
  }

  const items = data.items ?? [];

  return (
    <div className="pr-page">
      <div className="pr-head">
        <div className="gf-title">
          <h1 className="page-title tight">Подарки</h1>
          {data.counts ? <p className="gf-sum">{summaryText(data.counts)}</p> : null}
        </div>
        {data.off ? null : (
          <span className="pr-head-acts">
            <button
              type="button"
              className="b40 hov"
              aria-expanded={settings}
              aria-controls="gf-settings"
              onClick={() => {
                setSettings(!settings);
                setDone(null);
              }}
            >
              Настройки
            </button>
          </span>
        )}
      </div>

      {settings ? (
        <section id="gf-settings" className="card pr-new" aria-labelledby="gf-settings-h">
          <h2 id="gf-settings-h" className="section-title">
            Настройки подарков
          </h2>
          <Settings
            onDone={(text) => {
              setSettings(false);
              setDone(text);
            }}
          />
        </section>
      ) : null}

      <section className="card pr-list" aria-labelledby="gf-h">
        <div className="pr-top">
          <h2 id="gf-h" className="section-title">
            Купленные подарки
          </h2>
          <div className="gf-tools">
            <label className="gf-search">
              <span className="sr-only">Поиск подарка</span>
              <Icon name="search" size={17} />
              <input className="field" value={data.search} onChange={(e) => data.setSearch(e.target.value)} placeholder="Код, клиент или тариф" />
            </label>
            <div role="group" aria-label="Какие подарки показать" className="segs">
              {FILTERS.map(([key, label]) => (
                <button key={key} type="button" aria-pressed={data.filter === key} onClick={() => data.setFilter(key)}>
                  {label}
                </button>
              ))}
            </div>
          </div>
        </div>

        {done ? (
          <p className="pr-note" role="status">
            {done}
          </p>
        ) : null}

        {data.error ? (
          <p className="pr-pad">
            <span className="err" role="alert">
              {data.error}
            </span>
            {data.off ? (
              <>
                {' '}
                <Link to="/admin/modules/gifts" className="pr-inline-link">
                  Включить модуль
                </Link>
              </>
            ) : null}
          </p>
        ) : null}
        {data.items === null ? <p className="muted pr-pad">Загружаем подарки…</p> : null}
        {data.items !== null && items.length === 0 && !data.error ? <p className="muted pr-pad">{emptyText(data.filter, data.search)}</p> : null}

        {items.length ? (
          <div role="table" aria-label="Подарки" className="pr-table">
            <div role="row" className="pr-row gf-row pr-th">
              <span role="columnheader">Код</span>
              <span role="columnheader">Подарок</span>
              <span role="columnheader">Подарил</span>
              <span role="columnheader">Активировал</span>
              <span role="columnheader" className="pr-right">
                {items.some((g) => g.paid !== undefined) ? 'Оплата' : ''}
              </span>
              <span role="columnheader">Состояние</span>
            </div>
            {items.map((g) => (
              <GiftRow key={g.id} g={g} />
            ))}
          </div>
        ) : null}

        {data.next !== null ? (
          <div className="pr-pad">
            <button type="button" className="b40 hov" onClick={() => void data.loadMore()} disabled={data.more}>
              {data.more ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>
    </div>
  );
}

function GiftRow({ g }: { g: AdminGift }) {
  const state = stateOf(g);
  const paid = paidText(g);
  return (
    <div role="row" className={g.status === 'active' || g.status === 'redeemed' ? 'pr-row gf-row' : 'pr-row gf-row pr-dim'}>
      <span role="cell" className="pr-cell">
        <span className="pr-code mono">{g.code}</span>
        <span className="pr-sub">куплен {dateLong(g.created_at, false)}</span>
      </span>
      <span role="cell" className="pr-cell">
        <span className="pr-val">{giftText(g)}</span>
      </span>
      <span role="cell" className="pr-cell">
        <span className="pr-sub gf-label">Подарил</span>
        <Person id={g.giver?.client_id ?? null} text={who(g.giver)} />
      </span>
      <span role="cell" className="pr-cell">
        <span className="pr-sub gf-label">Активировал</span>
        <Person id={g.recipient?.client_id ?? null} text={who(g.recipient)} />
      </span>
      <span role="cell" className="pr-cell pr-right">
        {g.paid === undefined ? null : (
          <>
            <span className="num pr-strong">{paid || '—'}</span>
            {g.refunded ? <span className="pr-sub">возвращено</span> : null}
          </>
        )}
      </span>
      <span role="cell" className={`pr-cell pr-state ${state.tone}`}>
        {state.text}
        {state.note ? <span className="pr-sub">{state.note}</span> : null}
      </span>
    </div>
  );
}

function GiftsPhone() {
  const data = useGifts();
  const [settings, setSettings] = useState(false);
  const [open, setOpen] = useState<number | null>(null);
  const [toast, setToast] = useState<string | null>(null);

  useEffect(() => {
    if (!toast) {
      return;
    }
    const timer = window.setTimeout(() => setToast(null), 6000);
    return () => window.clearTimeout(timer);
  }, [toast]);

  if (!data.allowed) {
    return (
      <div className="hp">
        <PhoneHeader title="Подарки" />
        <p className="hp-quiet hp-card">Раздел доступен администраторам с правом «Маркетинг».</p>
      </div>
    );
  }

  const items = data.items ?? [];

  return (
    <div className="hp pr-phone">
      <PhoneHeader title="Подарки">
        {data.off ? null : (
          <button type="button" className="ph-circ press" aria-label="Настройки подарков" aria-expanded={settings} onClick={() => setSettings(!settings)}>
            <span className="ph-dot">
              <Icon name="sliders" size={19} />
            </span>
          </button>
        )}
      </PhoneHeader>
      <p className="np-sum" aria-live="polite">
        {data.error ? 'Список не загрузился' : data.items === null ? 'Загружаем…' : summaryText(data.counts)}
      </p>

      {data.error ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{data.error}</span>
          {data.off ? (
            <>
              {' '}
              <Link to="/admin/modules/gifts" className="pr-inline-link">
                Включить модуль
              </Link>
            </>
          ) : null}
        </p>
      ) : null}

      {settings ? (
        <section className="hp-group" aria-labelledby="gf-psettings">
          <h2 id="gf-psettings" className="hp-h2">
            Настройки подарков
          </h2>
          <div className="hp-card pr-pcard">
            <Settings
              phone
              onDone={(text) => {
                setSettings(false);
                setToast(text);
              }}
            />
          </div>
        </section>
      ) : null}

      <label className="gf-psearch">
        <span className="sr-only">Поиск подарка</span>
        <input className="field" value={data.search} onChange={(e) => data.setSearch(e.target.value)} placeholder="Код, клиент или тариф" />
      </label>

      <div role="group" aria-label="Какие подарки показать" className="pf-psegs pr-pfilter gf-pfilter">
        {FILTERS.map(([key, label]) => (
          <button key={key} type="button" aria-pressed={data.filter === key} onClick={() => data.setFilter(key)}>
            {label}
          </button>
        ))}
      </div>

      {data.items !== null && items.length === 0 && !data.error ? <p className="hp-quiet hp-card">{emptyText(data.filter, data.search)}</p> : null}

      {items.length ? (
        <ul className="hp-card hp-list np-ul" aria-label="Подарки">
          {items.map((g) => {
            const state = stateOf(g);
            const expanded = open === g.id;
            return (
              <li key={g.id} className="pr-pitem">
                <button type="button" className="pr-pmain press" aria-expanded={expanded} onClick={() => setOpen(expanded ? null : g.id)}>
                  <span className="pr-ptop">
                    <span className="pr-code mono">{g.code}</span>
                    <strong className="num">{paidText(g)}</strong>
                  </span>
                  <span className="hp-note muted">
                    {giftText(g)} · <span className={`pr-state ${state.tone}`}>{state.text.toLowerCase()}</span>
                  </span>
                </button>
                {expanded ? (
                  <div className="pr-pmore">
                    <dl className="pr-pdl">
                      <div>
                        <dt>Подарил</dt>
                        <dd>
                          <Person id={g.giver?.client_id ?? null} text={who(g.giver)} />
                        </dd>
                      </div>
                      <div>
                        <dt>Активировал</dt>
                        <dd>
                          <Person id={g.recipient?.client_id ?? null} text={who(g.recipient)} />
                        </dd>
                      </div>
                      {g.paid === undefined ? null : (
                        <div>
                          <dt>Оплата</dt>
                          <dd>
                            {paidText(g) || '—'}
                            {g.refunded ? ', возвращено' : ''}
                          </dd>
                        </div>
                      )}
                      {state.note ? (
                        <div>
                          <dt>{g.status === 'active' ? 'Срок кода' : 'Когда'}</dt>
                          <dd>{state.note}</dd>
                        </div>
                      ) : null}
                    </dl>
                  </div>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}

      {data.next !== null ? (
        <button type="button" className="pr-padd press" onClick={() => void data.loadMore()} disabled={data.more}>
          {data.more ? 'Загружаем…' : 'Показать ещё'}
        </button>
      ) : null}

      {toast ? (
        <div className="np-toast-wrap" role="status">
          <p className="np-toast">{toast}</p>
        </div>
      ) : null}
    </div>
  );
}
