import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { count } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { PhoneHeader } from '../phone';
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
  type Filter,
  type PromoCode,
} from './model';
import { PromoForm } from './PromoForm';
import { usePromos } from './usePromos';
import '../form.css';
import '../home/phone.css';
import './promo.css';

function summary(items: PromoCode[] | null, error: string | null, filter: Filter): string {
  if (error) {
    return 'Список не загрузился';
  }
  if (items === null) {
    return 'Загружаем…';
  }
  if (filter === 'disabled') {
    return items.length ? count(items.length, 'выключенный', 'выключенных', 'выключенных') : 'Выключенных нет';
  }
  if (!items.length) {
    return 'Промокодов пока нет';
  }
  const live = items.filter((p) => stateOf(p).tone !== 'muted').length;
  return `${count(live, 'действует', 'действуют', 'действуют')} из ${items.length}`;
}

export function PromoPhone() {
  const { me } = useSession();
  const data = usePromos();
  const [open, setOpen] = useState<number | null>(null);
  const [asking, setAsking] = useState<number | null>(null);
  const [creating, setCreating] = useState(false);
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
        <PhoneHeader title="Промокоды" />
        <p className="hp-quiet hp-card">Раздел доступен администраторам с правом «Маркетинг».</p>
      </div>
    );
  }

  const flip = async (code: PromoCode, active: boolean) => {
    try {
      const text = await data.switchCode(code, active);
      setAsking(null);
      setToast(text);
    } catch (err) {
      setToast(failText(err, 'Не получилось переключить промокод'));
    }
  };

  const items = data.items ?? [];

  return (
    <div className="hp pr-phone">
      <PhoneHeader title="Промокоды">
        <Link to="/admin/modules/promo" className="ph-circ press" aria-label="Настроить промокоды">
          <span className="ph-dot">
            <Icon name="sliders" size={19} />
          </span>
        </Link>
      </PhoneHeader>
      <p className="np-sum" aria-live="polite">
        {summary(data.items, data.error, data.filter)}
      </p>

      {data.error ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{data.error}</span>
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

      {data.off ? null : creating ? (
        <section className="hp-group" aria-labelledby="pr-pnew">
          <h2 id="pr-pnew" className="hp-h2">
            Новый промокод
          </h2>
          <div className="hp-card pr-pcard">
            <PromoForm
              phone
              currency={me.currency}
              tariffs={data.tariffs}
              onCancel={() => setCreating(false)}
              onCreated={(code) => {
                setCreating(false);
                setToast(`Промокод ${code.code} создан. ${createdText(code)}`);
                data.added(code);
              }}
            />
          </div>
        </section>
      ) : (
        <button type="button" className="pr-padd press" onClick={() => setCreating(true)}>
          <Icon name="plus" size={20} />
          Новый промокод
        </button>
      )}

      <div role="group" aria-label="Какие промокоды показать" className="pf-psegs pr-pfilter">
        {FILTERS.map(([key, label]) => (
          <button
            key={key}
            type="button"
            aria-pressed={data.filter === key}
            onClick={() => {
              data.setFilter(key);
              setOpen(null);
              setAsking(null);
            }}
          >
            {label}
          </button>
        ))}
      </div>

      {data.items !== null && items.length === 0 && !data.error ? (
        <p className="hp-quiet hp-card">{data.filter === 'disabled' ? 'Выключенных промокодов нет' : 'Промокодов пока нет'}</p>
      ) : null}

      {items.length ? (
        <ul className="hp-card hp-list np-ul" aria-label="Промокоды">
          {items.map((p) => {
            const state = stateOf(p);
            const isOpen = open === p.id;
            const reserved = reservedText(p);
            return (
              <li key={p.id} className={p.active ? 'pr-pitem' : 'pr-pitem pr-dim'}>
                <button
                  type="button"
                  className="pr-pmain press"
                  aria-expanded={isOpen}
                  aria-controls={`pr-p-${p.id}`}
                  onClick={() => {
                    setOpen(isOpen ? null : p.id);
                    setAsking(null);
                  }}
                >
                  <span className="pr-ptop">
                    <span className="pr-code mono">{p.code}</span>
                    <strong className="num">{valueText(p.kind)}</strong>
                  </span>
                  <span className="hp-note muted">
                    {usedText(p)} · {termText(p).toLowerCase()} · <span className={`pr-state ${state.tone}`}>{state.text.toLowerCase()}</span>
                  </span>
                </button>
                {isOpen ? (
                  <div id={`pr-p-${p.id}`} className="pr-pmore">
                    <dl className="pr-pdl">
                      <div>
                        <dt>Что даёт</dt>
                        <dd>{purposeText(p.kind)}</dd>
                      </div>
                      <div>
                        <dt>Ограничения</dt>
                        <dd>{limitsText(p, data.tariffs)}</dd>
                      </div>
                      {reserved ? (
                        <div>
                          <dt>Сейчас</dt>
                          <dd>{reserved}</dd>
                        </div>
                      ) : null}
                      {p.note ? (
                        <div>
                          <dt>Заметка</dt>
                          <dd>{p.note}</dd>
                        </div>
                      ) : null}
                    </dl>
                    {asking === p.id ? (
                      <div className="np-ask pr-pask">
                        <p>
                          <strong>Выключить {p.code}?</strong> {offText(p)}
                        </p>
                        <div className="np-ask-btns">
                          <button type="button" className="np-ok press" disabled={data.busy !== null} onClick={() => void flip(p, false)}>
                            {data.busy === p.id ? 'Выключаем…' : 'Выключить'}
                          </button>
                          <button type="button" className="np-cancel press" disabled={data.busy !== null} onClick={() => setAsking(null)}>
                            Отменить
                          </button>
                        </div>
                      </div>
                    ) : (
                      <div className="pr-pacts">
                        {p.active ? (
                          <button type="button" className="hp-pill" disabled={data.busy !== null} onClick={() => setAsking(p.id)}>
                            <span>Выключить</span>
                          </button>
                        ) : (
                          <button type="button" className="hp-pill" disabled={data.busy !== null} onClick={() => void flip(p, true)}>
                            <span>{data.busy === p.id ? 'Включаем…' : 'Включить'}</span>
                          </button>
                        )}
                      </div>
                    )}
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
