import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { ago, count, dateTime, money } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { Avatar } from '../Avatar';
import { personName } from '../home/model';
import { PhoneHeader } from '../phone';
import {
  detailsText,
  failText,
  HISTORY,
  methodName,
  percentText,
  queueText,
  statusText,
  termsText,
  totalsText,
  useCopy,
  useReferral,
  type Verdict,
  type Withdrawal,
} from './model';
import '../form.css';
import '../home/phone.css';
import '../promo/promo.css';
import './referral.css';

type Ask = { id: number; verdict: Verdict };

export function ReferralPhone() {
  const data = useReferral();
  const [copied, copy] = useCopy();
  const [open, setOpen] = useState<string | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [comment, setComment] = useState('');
  const [busy, setBusy] = useState(false);
  const [askError, setAskError] = useState<string | null>(null);
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
        <PhoneHeader title="Рефералы" />
        <p className="hp-quiet hp-card">Раздел доступен администраторам с правом «Деньги».</p>
      </div>
    );
  }

  const toggle = (key: string) => {
    setOpen(open === key ? null : key);
    setAsk(null);
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
      setToast(decided.status === 'paid' ? `Выплата ${money(w.amount)} подтверждена` : `Заявка на ${money(w.amount)} отклонена, деньги вернулись на баланс`);
      setAsk(null);
      setComment('');
      setOpen(null);
    } catch (err) {
      setAskError(failText(err, 'Решение не сохранилось'));
    } finally {
      setBusy(false);
    }
  };

  if (data.off) {
    return (
      <div className="hp">
        <PhoneHeader title="Рефералы" />
        <p className="hp-quiet hp-card">
          {data.error}.{' '}
          <Link to="/admin/modules/referral" className="pr-inline-link">
            Включить модуль
          </Link>
        </p>
      </div>
    );
  }

  const queue = data.queue ?? [];
  const history = data.history ?? [];
  const referrers = data.referrers ?? [];
  const summary = data.queue === null ? 'Загружаем…' : queueText(data.summary, data.queue);

  return (
    <div className="hp rf-phone">
      <PhoneHeader title="Рефералы">
        <Link to="/admin/modules/referral" className="ph-circ press" aria-label="Настроить рефералов">
          <span className="ph-dot">
            <Icon name="sliders" size={19} />
          </span>
        </Link>
      </PhoneHeader>
      {summary ? (
        <p className="np-sum" aria-live="polite">
          {summary}
        </p>
      ) : null}

      {data.error ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{data.error}</span>
        </p>
      ) : null}

      <section className="hp-group" aria-labelledby="rf-pq">
        <h2 id="rf-pq" className="hp-h2">
          Заявки на вывод
        </h2>
        {data.queue !== null && queue.length === 0 ? (
          <p className="hp-quiet hp-card">Заявок нет: новые появятся здесь, когда участники попросят вывод</p>
        ) : null}
        {queue.length ? (
          <ul className="hp-card hp-list np-ul">
            {queue.map((w) => {
              const key = `q${w.id}`;
              const isOpen = open === key;
              const name = personName(w.client, w.client_id);
              const asking = ask?.id === w.id ? ask.verdict : null;
              return (
                <li key={w.id} className="rf-pitem">
                  <button type="button" className="rf-pmain press" aria-expanded={isOpen} aria-controls={`rf-${key}`} onClick={() => toggle(key)}>
                    <Avatar id={w.client_id} name={name} className="hp-ava" />
                    <span className="rf-pbody">
                      <span className="hp-pay-top">
                        <span className="hp-title hp-ell">{name}</span>
                        <strong className="num">{money(w.amount)}</strong>
                      </span>
                      <span className="hp-note muted">
                        {methodName(w)} · {ago(w.created_at).toLowerCase()}
                      </span>
                    </span>
                  </button>
                  {isOpen ? (
                    <div id={`rf-${key}`} className="rf-pmore">
                      <div className="rf-pdetails">
                        <span className="mono rf-mono">{detailsText(w)}</span>
                        <button type="button" className="hp-pill" onClick={() => copy(key, w.details)}>
                          <span>{copied === key ? 'Скопировано' : 'Скопировать'}</span>
                        </button>
                      </div>
                      <Link to={`/admin/clients/${w.client_id}`} className="rf-plink">
                        Открыть клиента
                      </Link>
                      {asking ? (
                        <form
                          className="np-ask pr-pask rf-pask"
                          onSubmit={(e) => {
                            e.preventDefault();
                            void confirm(w, asking);
                          }}
                        >
                          <p>
                            {asking === 'paid' ? (
                              <>
                                <strong>Отметить выплату?</strong> Сначала переведите деньги: клиент сразу получит сообщение.
                              </>
                            ) : (
                              <>
                                <strong>Отклонить заявку?</strong> Деньги вернутся на реферальный баланс, причину клиент увидит в сообщении.
                              </>
                            )}
                          </p>
                          <input
                            className="field rf-pcomment"
                            value={comment}
                            onChange={(e) => {
                              setComment(e.target.value);
                              setAskError(null);
                            }}
                            maxLength={500}
                            placeholder={asking === 'paid' ? 'Комментарий, необязательно' : 'Причина отказа'}
                            aria-label={asking === 'paid' ? 'Комментарий к выплате' : 'Причина отказа'}
                            aria-invalid={askError && asking === 'rejected' ? true : undefined}
                            disabled={busy}
                          />
                          {askError ? (
                            <span className="err" role="alert">
                              {askError}
                            </span>
                          ) : null}
                          <div className="np-ask-btns">
                            <button type="submit" className="np-ok press" disabled={busy}>
                              {busy ? 'Сохраняем…' : asking === 'paid' ? 'Выплачено' : 'Отклонить'}
                            </button>
                            <button type="button" className="np-cancel press" disabled={busy} onClick={() => setAsk(null)}>
                              Отменить
                            </button>
                          </div>
                        </form>
                      ) : (
                        <div className="pr-pacts">
                          <button type="button" className="hp-pill" onClick={() => setAsk({ id: w.id, verdict: 'paid' })}>
                            <span>Выплачено</span>
                          </button>
                          <button type="button" className="hp-pill" onClick={() => setAsk({ id: w.id, verdict: 'rejected' })}>
                            <span>Отклонить</span>
                          </button>
                        </div>
                      )}
                    </div>
                  ) : null}
                </li>
              );
            })}
          </ul>
        ) : null}
        {data.queueNext !== null ? (
          <button type="button" className="pr-padd press" onClick={() => void data.more('queue')} disabled={data.loading !== null}>
            {data.loading === 'queue' ? 'Загружаем…' : 'Показать ещё'}
          </button>
        ) : null}
      </section>

      <section className="hp-group" aria-labelledby="rf-ph">
        <h2 id="rf-ph" className="hp-h2">
          История решений
        </h2>
        <div role="group" aria-label="Какие решения показать" className="pf-psegs">
          {HISTORY.map(([key, label]) => (
            <button key={key} type="button" aria-pressed={data.filter === key} onClick={() => data.setFilter(key)}>
              {label}
            </button>
          ))}
        </div>
        {data.history !== null && history.length === 0 ? <p className="hp-quiet hp-card">Решений пока нет</p> : null}
        {history.length ? (
          <ul className="hp-card hp-list np-ul">
            {history.map((w) => {
              const name = personName(w.client, w.client_id);
              const state = statusText(w);
              return (
                <li key={w.id} className="hp-pay">
                  <Avatar id={w.client_id} name={name} className="hp-ava" />
                  <Link to={`/admin/clients/${w.client_id}`} className="hp-pay-body rf-pplain">
                    <span className="hp-pay-top">
                      <span className="hp-title hp-ell">{name}</span>
                      <strong className="num">{money(w.amount)}</strong>
                    </span>
                    <span className="hp-note">
                      <span className={`pr-state ${state.tone}`}>{state.text}</span>
                      <span className="muted"> · {dateTime(w.decided_at ?? w.created_at)}</span>
                    </span>
                    {w.comment ? <span className="hp-note muted rf-pclamp">{w.comment}</span> : null}
                  </Link>
                </li>
              );
            })}
          </ul>
        ) : null}
        {data.historyNext !== null ? (
          <button type="button" className="pr-padd press" onClick={() => void data.more('history')} disabled={data.loading !== null}>
            {data.loading === 'history' ? 'Загружаем…' : 'Показать ещё'}
          </button>
        ) : null}
      </section>

      <section className="hp-group" aria-labelledby="rf-pr">
        <h2 id="rf-pr" className="hp-h2">
          Участники программы
        </h2>
        {data.referrers !== null && referrers.length === 0 ? (
          <p className="hp-quiet hp-card">Участников пока нет: они появятся, когда клиенты начнут приглашать друзей</p>
        ) : null}
        {referrers.length ? (
          <ul className="hp-card hp-list np-ul">
            {referrers.map((r) => {
              const name = personName(r.client, r.client_id);
              const percent = percentText(r, data.summary);
              return (
                <li key={r.client_id} className="hp-pay">
                  <Avatar id={r.client_id} name={name} className="hp-ava" />
                  <Link to={`/admin/clients/${r.client_id}`} className="hp-pay-body rf-pplain">
                    <span className="hp-pay-top">
                      <span className="hp-title hp-ell">{name}</span>
                      <strong className="num">{money(r.earned)}</strong>
                    </span>
                    <span className="hp-note muted">
                      {count(r.invited, 'друг', 'друга', 'друзей')} · на балансе {money(r.balance)} · {percent.text}
                      {percent.own ? ' личный' : ''}
                    </span>
                  </Link>
                </li>
              );
            })}
          </ul>
        ) : null}
        {data.referrersNext !== null ? (
          <button type="button" className="pr-padd press" onClick={() => void data.more('referrers')} disabled={data.loading !== null}>
            {data.loading === 'referrers' ? 'Загружаем…' : 'Показать ещё'}
          </button>
        ) : null}
      </section>

      {data.summary ? (
        <section className="hp-group" aria-labelledby="rf-pt">
          <h2 id="rf-pt" className="hp-h2">
            Итоги и условия
          </h2>
          <div className="hp-card rf-pterms">
            <p>{totalsText(data.summary)}</p>
            <p className="muted">{termsText(data.summary)}</p>
          </div>
        </section>
      ) : null}

      {toast ? (
        <div className="np-toast-wrap" role="status">
          <p className="np-toast">{toast}</p>
        </div>
      ) : null}
    </div>
  );
}
