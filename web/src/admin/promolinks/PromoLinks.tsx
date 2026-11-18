import { Fragment, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { count } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { NoAccess } from '../promo/Promo';
import { useCopy } from '../referral/model';
import { boughtText, emptyText, failText, FILTERS, giveText, squadsText, stateOf, usedText, type PromoLink } from './model';
import { LinkForm } from './LinkForm';
import { PromoTabs } from './PromoTabs';
import { usePromoLinks } from './usePromoLinks';
import '../form.css';
import '../home/phone.css';
import '../promo/promo.css';
import './links.css';

export function PromoLinks() {
  const phone = usePhone();
  return phone ? <LinksPhone /> : <LinksDesk />;
}

function offText(l: PromoLink): string {
  return `По ссылке «${l.name}» больше не выдадут подписку, уже выданные продолжат работать.`;
}

function LinksDesk() {
  const data = usePromoLinks();
  const [copied, copy] = useCopy();
  const [creating, setCreating] = useState(false);
  const [asking, setAsking] = useState<number | null>(null);
  const [done, setDone] = useState<{ head: string; text: string } | null>(null);
  const [rowError, setRowError] = useState<{ id: number; text: string } | null>(null);

  if (!data.allowed) {
    return <NoAccess title="Промо-ссылки" text="Раздел доступен администраторам с правом «Маркетинг»." />;
  }

  const flip = async (link: PromoLink, enabled: boolean) => {
    setRowError(null);
    try {
      const text = await data.switchLink(link, enabled);
      setAsking(null);
      setDone({ head: `${text}.`, text: enabled ? 'Клиенты снова могут получить подписку по ней.' : offText(link) });
    } catch (err) {
      setRowError({ id: link.id, text: failText(err, 'Не получилось переключить ссылку') });
    }
  };

  const items = data.items ?? [];

  return (
    <div className="pr-page">
      <PromoTabs />
      <div className="pr-head">
        <h1 className="page-title tight">Промо-ссылки</h1>
        {data.off ? null : (
          <span className="pr-head-acts">
            <button
              type="button"
              className={creating ? 'b40 hov' : 'btn-main btn pr-new-btn'}
              aria-expanded={creating}
              aria-controls="pl-new"
              onClick={() => {
                setCreating(!creating);
                setDone(null);
              }}
            >
              Новая ссылка
            </button>
          </span>
        )}
      </div>

      {creating ? (
        <section id="pl-new" className="card pr-new" aria-labelledby="pl-new-h">
          <h2 id="pl-new-h" className="section-title">
            Новая промо-ссылка
          </h2>
          <p className="cap">Клиент открывает ссылку в Telegram и сразу получает подписку с этими настройками. Один клиент — один раз.</p>
          <LinkForm
            squads={data.squads}
            onCancel={() => setCreating(false)}
            onCreated={(link) => {
              setCreating(false);
              setDone({ head: `Ссылка «${link.name}» создана.`, text: link.bot_link ? 'Скопируйте её в строке списка.' : 'Ссылка появится, когда бот будет подключён.' });
              data.added(link);
            }}
          />
        </section>
      ) : null}

      <section className="card pr-list" aria-labelledby="pl-h">
        <div className="pr-top">
          <h2 id="pl-h" className="section-title">
            Ссылки
          </h2>
          <div role="group" aria-label="Какие ссылки показать" className="segs">
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
                <Link to="/admin/modules/promolinks" className="pr-inline-link">
                  Включить модуль
                </Link>
              </>
            ) : null}
          </p>
        ) : null}
        {data.items === null ? <p className="muted pr-pad">Загружаем ссылки…</p> : null}
        {data.items !== null && items.length === 0 && !data.error ? <p className="muted pr-pad">{emptyText(data.filter)}</p> : null}

        {items.length ? (
          <div role="table" aria-label="Промо-ссылки" className="pr-table">
            <div role="row" className="pr-row pl-row pr-th">
              <span role="columnheader">Ссылка</span>
              <span role="columnheader">Что даёт</span>
              <span role="columnheader" className="pr-right">
                Активаций
              </span>
              <span role="columnheader">Состояние</span>
              <span role="columnheader">
                <span className="sr-only">Действия</span>
              </span>
            </div>
            {items.map((l) => {
              const state = stateOf(l);
              const open = asking === l.id;
              return (
                <Fragment key={l.id}>
                  <div role="row" className={l.state === 'active' ? 'pr-row pl-row' : 'pr-row pl-row pr-dim'}>
                    <span role="cell" className="pr-cell">
                      <span className="pr-code">{l.name}</span>
                      <span className="pr-sub mono">pl_{l.code}</span>
                    </span>
                    <span role="cell" className="pr-cell">
                      <span className="pr-val">{giveText(l)}</span>
                      <span className="pr-sub">{squadsText(l, data.squads)}</span>
                    </span>
                    <span role="cell" className="pr-cell pr-right">
                      <span className="num pr-strong">{usedText(l)}</span>
                      <span className="pr-sub">{boughtText(l)}</span>
                    </span>
                    <span role="cell" className={`pr-cell pr-state ${state.tone}`}>
                      {state.text}
                    </span>
                    <span role="cell" className="pr-cell pr-act pl-acts">
                      {l.bot_link ? (
                        <button type="button" className="b34 hov" onClick={() => copy(`l${l.id}`, l.bot_link ?? '')} aria-label={`Скопировать ссылку «${l.name}»`}>
                          {copied === `l${l.id}` ? 'Скопировано' : 'Скопировать'}
                        </button>
                      ) : null}
                      {l.enabled ? (
                        <button
                          type="button"
                          className="b34 hov"
                          aria-expanded={open}
                          aria-label={`Выключить «${l.name}»`}
                          disabled={data.busy !== null}
                          onClick={() => {
                            setAsking(open ? null : l.id);
                            setRowError(null);
                          }}
                        >
                          Выключить
                        </button>
                      ) : (
                        <button type="button" className="b34 hov" aria-label={`Включить «${l.name}»`} disabled={data.busy !== null} onClick={() => void flip(l, true)}>
                          {data.busy === l.id ? 'Включаем…' : 'Включить'}
                        </button>
                      )}
                    </span>
                  </div>
                  {open ? (
                    <div role="row" className="pr-ask-row">
                      <div role="cell" className="pr-ask">
                        <span>
                          <b>Выключить «{l.name}»?</b> {offText(l)} Включить снова можно в любой момент.
                        </span>
                        <span className="pr-ask-btns">
                          <button type="button" className="b34 on-surface hov" disabled={data.busy !== null} onClick={() => void flip(l, false)}>
                            {data.busy === l.id ? 'Выключаем…' : 'Выключить'}
                          </button>
                          <button type="button" className="b34 on-surface hov" disabled={data.busy !== null} onClick={() => setAsking(null)}>
                            Отменить
                          </button>
                        </span>
                      </div>
                    </div>
                  ) : null}
                  {rowError?.id === l.id ? (
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

function LinksPhone() {
  const data = usePromoLinks();
  const [copied, copy] = useCopy();
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
        <PhoneHeader title="Промо-ссылки" />
        <p className="hp-quiet hp-card">Раздел доступен администраторам с правом «Маркетинг».</p>
      </div>
    );
  }

  const flip = async (link: PromoLink, enabled: boolean) => {
    try {
      const text = await data.switchLink(link, enabled);
      setAsking(null);
      setToast(text);
    } catch (err) {
      setToast(failText(err, 'Не получилось переключить ссылку'));
    }
  };

  const items = data.items ?? [];
  const working = items.filter((l) => l.state === 'active').length;

  return (
    <div className="hp pr-phone">
      <PhoneHeader title="Промо-ссылки" />
      <PromoTabs phone />
      <p className="np-sum" aria-live="polite">
        {data.error ? 'Список не загрузился' : data.items === null ? 'Загружаем…' : items.length ? `${count(working, 'работает', 'работают', 'работают')} из ${items.length}` : 'Ссылок пока нет'}
      </p>

      {data.error ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{data.error}</span>
          {data.off ? (
            <>
              {' '}
              <Link to="/admin/modules/promolinks" className="pr-inline-link">
                Включить модуль
              </Link>
            </>
          ) : null}
        </p>
      ) : null}

      {data.off ? null : creating ? (
        <section className="hp-group" aria-labelledby="pl-pnew">
          <h2 id="pl-pnew" className="hp-h2">
            Новая промо-ссылка
          </h2>
          <div className="hp-card pr-pcard">
            <LinkForm
              phone
              squads={data.squads}
              onCancel={() => setCreating(false)}
              onCreated={(link) => {
                setCreating(false);
                setToast(`Ссылка «${link.name}» создана`);
                data.added(link);
              }}
            />
          </div>
        </section>
      ) : (
        <button type="button" className="pr-padd press" onClick={() => setCreating(true)}>
          <Icon name="plus" size={20} />
          Новая ссылка
        </button>
      )}

      <div role="group" aria-label="Какие ссылки показать" className="pf-psegs pr-pfilter">
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

      {data.items !== null && items.length === 0 && !data.error ? <p className="hp-quiet hp-card">{emptyText(data.filter)}</p> : null}

      {items.length ? (
        <ul className="hp-card hp-list np-ul" aria-label="Промо-ссылки">
          {items.map((l) => {
            const state = stateOf(l);
            const expanded = open === l.id;
            return (
              <li key={l.id} className={l.state === 'active' ? 'pr-pitem' : 'pr-pitem pr-dim'}>
                <button type="button" className="pr-pmain press" aria-expanded={expanded} onClick={() => setOpen(expanded ? null : l.id)}>
                  <span className="pr-ptop">
                    <span className="pr-code">{l.name}</span>
                    <strong className="num">{usedText(l)}</strong>
                  </span>
                  <span className="hp-note muted">
                    {giveText(l)} · <span className={`pr-state ${state.tone}`}>{state.text.toLowerCase()}</span>
                  </span>
                </button>
                {expanded ? (
                  <div className="pr-pmore">
                    <dl className="pr-pdl">
                      <div>
                        <dt>Ссылка</dt>
                        <dd className="mono">{l.bot_link ?? `pl_${l.code}`}</dd>
                      </div>
                      <div>
                        <dt>Сквады</dt>
                        <dd>{squadsText(l, data.squads)}</dd>
                      </div>
                      <div>
                        <dt>После активации</dt>
                        <dd>{boughtText(l)}</dd>
                      </div>
                    </dl>
                    {asking === l.id ? (
                      <div className="np-ask pr-pask">
                        <span>{offText(l)}</span>
                        <div className="np-ask-btns">
                          <button type="button" className="np-ok press" disabled={data.busy !== null} onClick={() => void flip(l, false)}>
                            {data.busy === l.id ? 'Выключаем…' : 'Выключить'}
                          </button>
                          <button type="button" className="np-cancel press" disabled={data.busy !== null} onClick={() => setAsking(null)}>
                            Отменить
                          </button>
                        </div>
                      </div>
                    ) : (
                      <div className="pr-pacts">
                        {l.bot_link ? (
                          <button type="button" className="hp-pill" onClick={() => copy(`p${l.id}`, l.bot_link ?? '')}>
                            {copied === `p${l.id}` ? 'Скопировано' : 'Скопировать ссылку'}
                          </button>
                        ) : null}
                        {l.enabled ? (
                          <button type="button" className="hp-pill" disabled={data.busy !== null} onClick={() => setAsking(l.id)}>
                            Выключить
                          </button>
                        ) : (
                          <button type="button" className="hp-pill" disabled={data.busy !== null} onClick={() => void flip(l, true)}>
                            Включить
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
