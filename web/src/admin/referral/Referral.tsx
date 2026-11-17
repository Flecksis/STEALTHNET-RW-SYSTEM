import { Fragment, useState } from 'react';
import { Link } from 'react-router';
import { count, dateTime, money } from '../../lib/format';
import { usePhone } from '../../lib/media';
import { Avatar } from '../Avatar';
import { personName } from '../home/model';
import { NoAccess } from '../promo/Promo';
import { useSession } from '../session';
import {
  contactLine,
  detailsText,
  failText,
  HISTORY,
  methodName,
  parsePercent,
  percentText,
  queueText,
  recentText,
  statusText,
  termsText,
  totalsText,
  useCopy,
  useReferral,
  type Referrer,
  type Verdict,
  type Withdrawal,
} from './model';
import { ReferralPhone } from './ReferralPhone';
import '../form.css';
import '../promo/promo.css';
import './referral.css';

export function Referral() {
  const phone = usePhone();
  return phone ? <ReferralPhone /> : <ReferralDesk />;
}

function Who({ id, person }: { id: number; person: Withdrawal['client'] }) {
  const name = personName(person, id);
  const contact = contactLine(person);
  return (
    <Link to={`/admin/clients/${id}`} className="rf-who">
      <Avatar id={id} name={name} className="rf-ava" />
      <span className="pr-cell">
        <span className="rf-name">{name}</span>
        {contact ? <span className="pr-sub">{contact}</span> : null}
      </span>
    </Link>
  );
}

function Details({ w, copied, onCopy }: { w: Withdrawal; copied: boolean; onCopy: () => void }) {
  return (
    <span className="pr-cell">
      <span className="pr-sub">{methodName(w.method)}</span>
      <span className="rf-details">
        <span className="mono rf-mono">{detailsText(w)}</span>
        <button type="button" className="rf-copy hov" onClick={onCopy} aria-label={`Скопировать реквизиты: ${personName(w.client, w.client_id)}`}>
          {copied ? 'Скопировано' : 'Скопировать'}
        </button>
      </span>
    </span>
  );
}

type Ask = { id: number; verdict: Verdict };

function PercentEditor({
  r,
  general,
  onSave,
  onClose,
}: {
  r: Referrer;
  general: number | null;
  onSave: (p: number | null) => Promise<void>;
  onClose: () => void;
}) {
  const [raw, setRaw] = useState(r.personal_percent === null ? '' : String(r.personal_percent));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const save = async () => {
    const value = parsePercent(raw);
    if (value === 'bad') {
      setError('Целое число от 0 до 100 или пусто');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSave(value);
      onClose();
    } catch (err) {
      setError(failText(err, 'Процент не сохранился'));
    } finally {
      setBusy(false);
    }
  };
  const name = personName(r.client, r.client_id);
  return (
    <form
      className="pr-ask rf-ask"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <span>
        <b>Личный процент: {name}.</b> Действует вместо общих {general ?? '—'}% на оплаты тех, кого пригласил этот участник. Пусто — общий процент.
      </span>
      <span className="rf-ask-tools">
        <span className="pf-unit rf-percent">
          <input
            className="field num"
            inputMode="numeric"
            value={raw}
            onChange={(e) => setRaw(e.target.value)}
            maxLength={4}
            aria-label={`Личный процент: ${name}`}
            aria-invalid={error ? true : undefined}
            disabled={busy}
            autoFocus
          />
          <span className="muted">%</span>
        </span>
        <span className="pr-ask-btns">
          <button type="submit" className="b34 on-surface hov" disabled={busy}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
          <button type="button" className="b34 on-surface hov" disabled={busy} onClick={onClose}>
            Отменить
          </button>
        </span>
      </span>
      {error ? (
        <span className="err rf-ask-err" role="alert">
          {error}
        </span>
      ) : null}
    </form>
  );
}

function ReferralDesk() {
  const { can } = useSession();
  const data = useReferral();
  const [copied, copy] = useCopy();
  const [ask, setAsk] = useState<Ask | null>(null);
  const [comment, setComment] = useState('');
  const [busy, setBusy] = useState(false);
  const [askError, setAskError] = useState<string | null>(null);
  const [done, setDone] = useState<{ head: string; text: string } | null>(null);
  const [editing, setEditing] = useState<number | null>(null);

  if (!data.allowed) {
    return <NoAccess title="Рефералы" text="Раздел доступен администраторам с правом «Деньги»." />;
  }

  const open = (w: Withdrawal, verdict: Verdict) => {
    const same = ask?.id === w.id && ask.verdict === verdict;
    setAsk(same ? null : { id: w.id, verdict });
    setComment('');
    setAskError(null);
  };

  const confirm = async (w: Withdrawal, verdict: Verdict) => {
    if (verdict === 'rejected' && !comment.trim()) {
      setAskError('Напишите причину: клиент увидит её в сообщении');
      return;
    }
    setBusy(true);
    setAskError(null);
    try {
      const decided = await data.decide(w, verdict, comment);
      const name = personName(w.client, w.client_id);
      setDone(
        decided.status === 'paid'
          ? { head: `Выплата ${money(w.amount)} подтверждена.`, text: `${name} получит сообщение о выплате.` }
          : { head: `Заявка на ${money(w.amount)} отклонена.`, text: `Деньги вернулись на реферальный баланс, ${name} получит сообщение с причиной.` },
      );
      setAsk(null);
      setComment('');
    } catch (err) {
      setAskError(failText(err, 'Решение не сохранилось'));
    } finally {
      setBusy(false);
    }
  };

  if (data.off) {
    return (
      <div className="pr-page">
        <h1 className="page-title tight">Рефералы</h1>
        <section className="card card-pad">
          <p>
            <span className="muted">{data.error}. </span>
            <Link to="/admin/modules/referral" className="pr-inline-link">
              Включить модуль
            </Link>
          </p>
        </section>
      </div>
    );
  }

  const queue = data.queue ?? [];
  const history = data.history ?? [];
  const referrers = data.referrers ?? [];
  const general = data.summary ? data.summary.levels[0] : null;
  const canPercent = can('marketing');

  return (
    <div className="pr-page">
      <div className="pr-head">
        <h1 className="page-title tight">Рефералы</h1>
        <span className="pr-head-acts">
          <Link to="/admin/modules/referral" className="btn-line hov pr-link">
            Настроить
          </Link>
        </span>
      </div>

      {data.summary ? (
        <p className="rf-lead">
          {totalsText(data.summary)} <span className="rf-terms">{termsText(data.summary)}</span>
        </p>
      ) : null}

      {data.error ? (
        <p className="rf-lead" role="alert">
          <span className="err rf-err">{data.error}</span>
        </p>
      ) : null}

      <section className="card pr-list" aria-labelledby="rf-q-h">
        <div className="pr-top">
          <h2 id="rf-q-h" className="section-title">
            Заявки на вывод
          </h2>
          <span className="muted">{data.queue === null ? '' : queueText(data.summary, data.queue)}</span>
        </div>
        {done ? (
          <p className="pr-note" role="status">
            <b>{done.head}</b> {done.text}
          </p>
        ) : null}
        {data.queue === null ? <p className="muted pr-pad">Загружаем заявки…</p> : null}
        {data.queue !== null && queue.length === 0 && !data.error ? (
          <p className="muted pr-pad">Заявок нет: новые появятся здесь, когда участники попросят вывод</p>
        ) : null}
        {queue.length ? (
          <div role="table" aria-label="Заявки на вывод">
            <div role="row" className="rf-row rf-qrow pr-th">
              <span role="columnheader">Клиент</span>
              <span role="columnheader" className="pr-right">
                Сумма
              </span>
              <span role="columnheader">Куда перевести</span>
              <span role="columnheader">Создана</span>
              <span role="columnheader">
                <span className="sr-only">Решение</span>
              </span>
            </div>
            {queue.map((w) => {
              const asking = ask?.id === w.id ? ask.verdict : null;
              return (
                <Fragment key={w.id}>
                  <div role="row" className="rf-row rf-qrow">
                    <span role="cell" className="pr-cell">
                      <Who id={w.client_id} person={w.client} />
                    </span>
                    <span role="cell" className="pr-cell pr-right">
                      <strong className="num rf-sum">{money(w.amount)}</strong>
                    </span>
                    <span role="cell" className="pr-cell">
                      <Details w={w} copied={copied === `q${w.id}`} onCopy={() => copy(`q${w.id}`, w.details)} />
                    </span>
                    <span role="cell" className="pr-cell">
                      <span>{dateTime(w.created_at)}</span>
                      {recentText(w.created_at) ? <span className="pr-sub">{recentText(w.created_at)}</span> : null}
                    </span>
                    <span role="cell" className="pr-cell rf-acts">
                      <button type="button" className="b34 hov" aria-expanded={asking === 'paid'} disabled={busy} onClick={() => open(w, 'paid')}>
                        Выплачено
                      </button>
                      <button type="button" className="b34 hov" aria-expanded={asking === 'rejected'} disabled={busy} onClick={() => open(w, 'rejected')}>
                        Отклонить
                      </button>
                    </span>
                  </div>
                  {asking ? (
                    <div role="row" className="pr-ask-row">
                      <form
                        role="cell"
                        className="pr-ask rf-ask"
                        onSubmit={(e) => {
                          e.preventDefault();
                          void confirm(w, asking);
                        }}
                      >
                        <span>
                          {asking === 'paid' ? (
                            <>
                              <b>Отметить выплату {money(w.amount)}?</b> Сначала переведите деньги по реквизитам: клиент сразу получит сообщение о выплате.
                            </>
                          ) : (
                            <>
                              <b>Отклонить заявку на {money(w.amount)}?</b> Деньги вернутся на реферальный баланс клиента, причину он увидит в сообщении.
                            </>
                          )}
                        </span>
                        <span className="rf-ask-tools">
                          <input
                            className="field rf-comment"
                            value={comment}
                            onChange={(e) => {
                              setComment(e.target.value);
                              setAskError(null);
                            }}
                            maxLength={500}
                            placeholder={asking === 'paid' ? 'Комментарий для клиента, необязательно' : 'Причина отказа'}
                            aria-label={asking === 'paid' ? 'Комментарий к выплате' : 'Причина отказа'}
                            aria-invalid={askError && asking === 'rejected' ? true : undefined}
                            disabled={busy}
                            autoFocus
                          />
                          <span className="pr-ask-btns">
                            <button type="submit" className={asking === 'rejected' ? 'b34 on-surface hov rf-bad' : 'b34 on-surface hov'} disabled={busy}>
                              {busy ? 'Сохраняем…' : asking === 'paid' ? 'Выплачено' : 'Отклонить'}
                            </button>
                            <button type="button" className="b34 on-surface hov" disabled={busy} onClick={() => setAsk(null)}>
                              Отменить
                            </button>
                          </span>
                        </span>
                        {askError ? (
                          <span className="err rf-ask-err" role="alert">
                            {askError}
                          </span>
                        ) : null}
                      </form>
                    </div>
                  ) : null}
                </Fragment>
              );
            })}
          </div>
        ) : null}
        {data.queueNext !== null ? (
          <div className="pr-pad">
            <button type="button" className="b40 hov" onClick={() => void data.more('queue')} disabled={data.loading !== null}>
              {data.loading === 'queue' ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>

      <section className="card pr-list" aria-labelledby="rf-h-h">
        <div className="pr-top">
          <h2 id="rf-h-h" className="section-title">
            История решений
          </h2>
          <div role="group" aria-label="Какие решения показать" className="segs">
            {HISTORY.map(([key, label]) => (
              <button key={key} type="button" aria-pressed={data.filter === key} onClick={() => data.setFilter(key)}>
                {label}
              </button>
            ))}
          </div>
        </div>
        {data.history === null ? <p className="muted pr-pad">Загружаем историю…</p> : null}
        {data.history !== null && history.length === 0 ? <p className="muted pr-pad">Решений пока нет</p> : null}
        {history.length ? (
          <div role="table" aria-label="История решений">
            <div role="row" className="rf-row rf-hrow pr-th">
              <span role="columnheader">Клиент</span>
              <span role="columnheader" className="pr-right">
                Сумма
              </span>
              <span role="columnheader">Куда</span>
              <span role="columnheader">Решение</span>
              <span role="columnheader">Комментарий</span>
            </div>
            {history.map((w) => {
              const state = statusText(w);
              return (
                <div key={w.id} role="row" className="rf-row rf-hrow">
                  <span role="cell" className="pr-cell">
                    <Who id={w.client_id} person={w.client} />
                  </span>
                  <span role="cell" className="pr-cell pr-right">
                    <strong className="num rf-sum">{money(w.amount)}</strong>
                  </span>
                  <span role="cell" className="pr-cell">
                    <Details w={w} copied={copied === `h${w.id}`} onCopy={() => copy(`h${w.id}`, w.details)} />
                  </span>
                  <span role="cell" className="pr-cell">
                    <span className={`pr-state ${state.tone}`}>{state.text}</span>
                    <span className="pr-sub">{dateTime(w.decided_at ?? w.created_at)}</span>
                  </span>
                  <span role="cell" className="pr-cell">
                    {w.comment ? <span className="rf-comment-text">{w.comment}</span> : <span className="muted">—</span>}
                    {w.decided_by ? <span className="pr-sub">{w.decided_by}</span> : null}
                  </span>
                </div>
              );
            })}
          </div>
        ) : null}
        {data.historyNext !== null ? (
          <div className="pr-pad">
            <button type="button" className="b40 hov" onClick={() => void data.more('history')} disabled={data.loading !== null}>
              {data.loading === 'history' ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>

      <section className="card pr-list" aria-labelledby="rf-r-h">
        <div className="pr-top">
          <h2 id="rf-r-h" className="section-title">
            Участники программы
          </h2>
          <span className="muted">По заработку, сверху самые активные</span>
        </div>
        {data.referrers === null ? <p className="muted pr-pad">Загружаем участников…</p> : null}
        {data.referrers !== null && referrers.length === 0 ? (
          <p className="muted pr-pad">Участников пока нет: они появятся, когда клиенты начнут приглашать друзей</p>
        ) : null}
        {referrers.length ? (
          <div role="table" aria-label="Участники программы">
            <div role="row" className="rf-row rf-rrow pr-th">
              <span role="columnheader">Участник</span>
              <span role="columnheader" className="pr-right">
                Пригласил
              </span>
              <span role="columnheader" className="pr-right">
                Заработал
              </span>
              <span role="columnheader" className="pr-right">
                Выплачено
              </span>
              <span role="columnheader" className="pr-right">
                На балансе
              </span>
              <span role="columnheader">Процент</span>
            </div>
            {referrers.map((r) => {
              const percent = percentText(r, data.summary);
              const name = personName(r.client, r.client_id);
              return (
                <Fragment key={r.client_id}>
                  <div role="row" className="rf-row rf-rrow">
                    <span role="cell" className="pr-cell">
                      <Who id={r.client_id} person={r.client} />
                    </span>
                    <span role="cell" className={r.invited ? 'pr-cell pr-right num' : 'pr-cell pr-right num muted'}>
                      {count(r.invited, 'друг', 'друга', 'друзей')}
                    </span>
                    <span role="cell" className={r.earned.minor ? 'pr-cell pr-right num rf-strong' : 'pr-cell pr-right num muted'}>
                      {money(r.earned)}
                    </span>
                    <span role="cell" className={r.paid_out.minor ? 'pr-cell pr-right num' : 'pr-cell pr-right num muted'}>
                      {money(r.paid_out)}
                    </span>
                    <span role="cell" className={r.balance.minor ? 'pr-cell pr-right num' : 'pr-cell pr-right num muted'}>
                      {money(r.balance)}
                    </span>
                    <span role="cell" className="pr-cell rf-pct">
                      <span className="rf-pct-line">
                        <span className={percent.own ? 'rf-strong' : 'muted'}>
                          {percent.text}
                          {percent.own ? '' : ', общий'}
                        </span>
                        {canPercent ? (
                          <button
                            type="button"
                            className="rf-copy hov"
                            aria-expanded={editing === r.client_id}
                            aria-label={`Изменить процент: ${name}`}
                            onClick={() => setEditing(editing === r.client_id ? null : r.client_id)}
                          >
                            Изменить
                          </button>
                        ) : null}
                      </span>
                    </span>
                  </div>
                  {editing === r.client_id ? (
                    <div role="row" className="pr-ask-row">
                      <div role="cell">
                        <PercentEditor r={r} general={general} onSave={(p) => data.setPercent(r, p)} onClose={() => setEditing(null)} />
                      </div>
                    </div>
                  ) : null}
                </Fragment>
              );
            })}
          </div>
        ) : null}
        {data.referrersNext !== null ? (
          <div className="pr-pad">
            <button type="button" className="b40 hov" onClick={() => void data.more('referrers')} disabled={data.loading !== null}>
              {data.loading === 'referrers' ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>
    </div>
  );
}
