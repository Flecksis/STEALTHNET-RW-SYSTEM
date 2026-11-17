import { Fragment, useState } from 'react';
import { Link } from 'react-router';
import { usePhone } from '../../lib/media';
import { useSession } from '../session';
import {
  createdText,
  failText,
  FILTERS,
  limitsText,
  offText,
  purposeText,
  reservedText,
  stateOf,
  termText,
  usedText,
  valueText,
  type PromoCode,
} from './model';
import { PromoForm } from './PromoForm';
import { PromoPhone } from './PromoPhone';
import { usePromos } from './usePromos';
import '../form.css';
import './promo.css';

export function Promo() {
  const phone = usePhone();
  return phone ? <PromoPhone /> : <PromoDesk />;
}

export function NoAccess({ title, text }: { title: string; text: string }) {
  return (
    <>
      <h1 className="page-title">{title}</h1>
      <section className="card card-pad">
        <p className="muted">{text}</p>
      </section>
    </>
  );
}

function PromoDesk() {
  const { me } = useSession();
  const data = usePromos();
  const [creating, setCreating] = useState(false);
  const [asking, setAsking] = useState<number | null>(null);
  const [done, setDone] = useState<{ head: string; text: string } | null>(null);
  const [rowError, setRowError] = useState<{ id: number; text: string } | null>(null);

  if (!data.allowed) {
    return <NoAccess title="Промокоды" text="Раздел доступен администраторам с правом «Маркетинг»." />;
  }

  const created = (code: PromoCode) => {
    setCreating(false);
    setDone({ head: `Промокод ${code.code} создан.`, text: createdText(code) });
    data.added(code);
  };

  const flip = async (code: PromoCode, active: boolean) => {
    setRowError(null);
    try {
      const text = await data.switchCode(code, active);
      setAsking(null);
      setDone({ head: `${text}.`, text: active ? 'Клиенты снова могут его применять.' : offText(code) });
    } catch (err) {
      setRowError({ id: code.id, text: failText(err, 'Не получилось переключить промокод') });
    }
  };

  const items = data.items ?? [];

  return (
    <div className="pr-page">
      <div className="pr-head">
        <h1 className="page-title tight">Промокоды</h1>
        <span className="pr-head-acts">
          <Link to="/admin/modules/promo" className="b40 hov pr-link">
            Настроить
          </Link>
          {data.off ? null : (
            <button
              type="button"
              className={creating ? 'b40 hov' : 'btn-main btn pr-new-btn'}
              aria-expanded={creating}
              aria-controls="pr-new"
              onClick={() => {
                setCreating(!creating);
                setDone(null);
              }}
            >
              Новый промокод
            </button>
          )}
        </span>
      </div>

      {creating ? (
        <section id="pr-new" className="card pr-new" aria-labelledby="pr-new-h">
          <h2 id="pr-new-h" className="section-title">
            Новый промокод
          </h2>
          <PromoForm currency={me.currency} tariffs={data.tariffs} onCancel={() => setCreating(false)} onCreated={created} />
        </section>
      ) : null}

      <section className="card pr-list" aria-labelledby="pr-h">
        <div className="pr-top">
          <h2 id="pr-h" className="section-title">
            Коды
          </h2>
          <div role="group" aria-label="Какие промокоды показать" className="segs">
            {FILTERS.map(([key, label]) => (
              <button
                key={key}
                type="button"
                aria-pressed={data.filter === key}
                onClick={() => {
                  data.setFilter(key);
                  setAsking(null);
                }}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        {done ? (
          <p className="pr-note" role="status">
            <b>{done.head}</b> {done.text}
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
                <Link to="/admin/modules/promo" className="pr-inline-link">
                  Включить модуль
                </Link>
              </>
            ) : null}
          </p>
        ) : null}
        {data.items === null ? <p className="muted pr-pad">Загружаем промокоды…</p> : null}
        {data.items !== null && items.length === 0 && !data.error ? (
          <p className="muted pr-pad">{data.filter === 'disabled' ? 'Выключенных промокодов нет' : 'Промокодов пока нет'}</p>
        ) : null}

        {items.length ? (
          <div role="table" aria-label="Промокоды" className="pr-table">
            <div role="row" className="pr-row pr-th">
              <span role="columnheader">Код</span>
              <span role="columnheader">Что даёт</span>
              <span role="columnheader" className="pr-right">
                Использовано
              </span>
              <span role="columnheader">Срок</span>
              <span role="columnheader">Состояние</span>
              <span role="columnheader">
                <span className="sr-only">Действия</span>
              </span>
            </div>
            {items.map((p) => {
              const state = stateOf(p);
              const reserved = reservedText(p);
              const open = asking === p.id;
              return (
                <Fragment key={p.id}>
                  <div role="row" className={p.active ? 'pr-row' : 'pr-row pr-dim'}>
                    <span role="cell" className="pr-cell">
                      <span className="pr-code mono">{p.code}</span>
                      {p.note ? <span className="pr-sub">{p.note}</span> : null}
                    </span>
                    <span role="cell" className="pr-cell">
                      <span className="pr-val">
                        {valueText(p.kind)} <span className="pr-purpose">{purposeText(p.kind)}</span>
                      </span>
                      <span className="pr-sub">{limitsText(p, data.tariffs)}</span>
                    </span>
                    <span role="cell" className="pr-cell pr-right">
                      <span className="num pr-strong">{usedText(p)}</span>
                      {reserved ? <span className="pr-sub">{reserved}</span> : null}
                    </span>
                    <span role="cell" className="pr-cell">
                      {termText(p)}
                    </span>
                    <span role="cell" className={`pr-cell pr-state ${state.tone}`}>
                      {state.text}
                    </span>
                    <span role="cell" className="pr-cell pr-act">
                      {p.active ? (
                        <button
                          type="button"
                          className="b34 hov"
                          aria-expanded={open}
                          aria-label={`Выключить ${p.code}`}
                          disabled={data.busy !== null}
                          onClick={() => {
                            setAsking(open ? null : p.id);
                            setRowError(null);
                          }}
                        >
                          Выключить
                        </button>
                      ) : (
                        <button
                          type="button"
                          className="b34 hov"
                          aria-label={`Включить ${p.code}`}
                          disabled={data.busy !== null}
                          onClick={() => void flip(p, true)}
                        >
                          {data.busy === p.id ? 'Включаем…' : 'Включить'}
                        </button>
                      )}
                    </span>
                  </div>
                  {open ? (
                    <div role="row" className="pr-ask-row">
                      <div role="cell" className="pr-ask">
                        <span>
                          <b>Выключить {p.code}?</b> {offText(p)} Включить снова можно в любой момент.
                        </span>
                        <span className="pr-ask-btns">
                          <button type="button" className="b34 on-surface hov" disabled={data.busy !== null} onClick={() => void flip(p, false)}>
                            {data.busy === p.id ? 'Выключаем…' : 'Выключить'}
                          </button>
                          <button type="button" className="b34 on-surface hov" disabled={data.busy !== null} onClick={() => setAsking(null)}>
                            Отменить
                          </button>
                        </span>
                      </div>
                    </div>
                  ) : null}
                  {rowError?.id === p.id ? (
                    <div role="row" className="pr-ask-row">
                      <div role="cell" className="err pr-row-err">
                        {rowError.text}
                      </div>
                    </div>
                  ) : null}
                </Fragment>
              );
            })}
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
