import { useCallback, useEffect, useState } from 'react';
import { Link, useLocation, useParams } from 'react-router';
import { api } from '../../lib/api';
import { count, dateTime } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { personName } from '../home/model';
import { PhoneHeader } from '../phone';
import { MoneyTabs } from './MoneyTabs';
import {
  JOURNAL,
  WEBHOOKS,
  deliveriesText,
  eventText,
  failText,
  toneColor,
  verdictText,
  type BodyView,
  type Fact,
  type JournalCard,
  type Tone,
} from './journal';
import '../form.css';
import '../promo/promo.css';
import '../home/phone.css';
import '../protect/protect.css';
import './webhooks.css';

const PAYMENT_STATE: Record<string, string> = {
  pending: 'ждёт оплаты',
  paid: 'оплачен',
  failed: 'не прошёл',
  canceled: 'истёк или отменён',
  refunded: 'возвращён',
  partially_refunded: 'частично возвращён',
};

type Result = { text: string; tone: Tone };

function useCard(id: number) {
  const [card, setCard] = useState<JournalCard | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<Result | null>(null);

  const load = useCallback(async () => {
    try {
      setCard(await api<JournalCard>(`${WEBHOOKS}/${id}`));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Уведомление не загрузилось'));
    }
  }, [id]);

  useEffect(() => {
    setCard(null);
    setResult(null);
    void load();
  }, [load]);

  const retry = async (): Promise<boolean> => {
    setBusy(true);
    try {
      const updated = await api<JournalCard>(`${WEBHOOKS}/${id}/retry`, { method: 'POST', body: {} });
      setCard(updated);
      setResult(
        updated.tone === 'bad'
          ? { text: 'Обработали ещё раз, но снова не получилось: подробности выше.', tone: 'bad' }
          : { text: 'Уведомление обработано ещё раз.', tone: updated.tone },
      );
      return true;
    } catch (err) {
      setResult({ text: failText(err, 'Повторная обработка не удалась'), tone: 'bad' });
      void load();
      return false;
    } finally {
      setBusy(false);
    }
  };

  return { card, error, busy, result, retry };
}

type Data = ReturnType<typeof useCard>;

export function WebhookCard() {
  const { id } = useParams();
  const phone = usePhone();
  const location = useLocation();
  const back = `${JOURNAL}${(location.state as { back?: string } | null)?.back ?? ''}`;
  const data = useCard(Number(id));
  return phone ? <CardPhone data={data} back={back} /> : <CardDesk data={data} back={back} />;
}

function Facts({ facts, className }: { facts: Fact[]; className?: string }) {
  return (
    <dl className={className ? `wh-dl ${className}` : 'wh-dl'}>
      {facts.map((fact, index) => (
        <div key={`${fact.label}-${index}`} className="wh-dl-row">
          <dt>{fact.label}</dt>
          <dd>{fact.value}</dd>
        </div>
      ))}
    </dl>
  );
}

function Body({ body }: { body: BodyView }) {
  switch (body.kind) {
    case 'json':
    case 'text':
      return (
        <pre className="wh-pre" tabIndex={0} aria-label="Тело уведомления">
          {body.text}
        </pre>
      );
    case 'form':
      return body.fields.length ? <Facts facts={body.fields} className="wh-form" /> : <p className="muted">Поля пустые</p>;
    case 'binary':
      return <p className="muted">Не текст: {count(body.size, 'байт', 'байта', 'байт')}</p>;
    default:
      return <p className="muted">{body.reason}</p>;
  }
}

function Steps({ card }: { card: JournalCard }) {
  return (
    <ol className="wh-steps">
      {card.steps.map((step, index) => (
        <li key={`${step.at}-${index}`}>
          <span className="wh-step-time num">{dateTime(step.at)}</span>
          <span style={{ color: step.tone === 'muted' ? 'var(--ink-2)' : toneColor(step.tone), fontWeight: step.tone === 'muted' ? 400 : 600 }}>{step.text}</span>
        </li>
      ))}
    </ol>
  );
}

function Linked({ card }: { card: JournalCard }) {
  const name = card.client_id ? personName(card.client, card.client_id) : null;
  if (!card.payment && !name) {
    const text =
      card.facts.length === 0
        ? 'Неизвестно: уведомление не прошло проверку'
        : card.outcome === 'payment_not_found'
          ? 'Платежа из уведомления нет в панели'
          : 'Уведомление не связано с платежом панели';
    return <p className="muted wh-none">{text}</p>;
  }
  return (
    <div className="wh-linked">
      {card.payment ? (
        <Link to={`/admin/money?payment=${card.payment.id}`} className="wh-linked-row hov">
          <span className="wh-linked-main">Платёж №{card.payment.id}</span>
          <span className="muted">
            {card.payment.amount_text} · {PAYMENT_STATE[card.payment.status] ?? card.payment.status}
          </span>
          <Icon name="chevron-right" size={16} />
        </Link>
      ) : null}
      {name && card.client_id ? (
        <Link to={`/admin/clients/${card.client_id}`} className="wh-linked-row hov">
          <span className="wh-linked-main">{name}</span>
          <span className="muted">клиент</span>
          <Icon name="chevron-right" size={16} />
        </Link>
      ) : null}
    </div>
  );
}

function Retry({ data, phone }: { data: Data; phone?: boolean }) {
  const [asking, setAsking] = useState(false);
  const card = data.card;
  if (!card?.retry.available) {
    return null;
  }
  const run = async () => {
    if (await data.retry()) {
      setAsking(false);
    }
  };
  const question = (
    <>
      <b>Обработать уведомление ещё раз?</b> Если оплата уже засчитана, повтор ничего не изменит: деньги второй раз не зачислятся.
    </>
  );
  if (phone) {
    return asking ? (
      <div className="np-ask">
        <p>{question}</p>
        <div className="np-ask-btns">
          <button type="button" className="np-ok press" disabled={data.busy} onClick={() => void run()}>
            {data.busy ? 'Обрабатываем…' : 'Обработать'}
          </button>
          <button type="button" className="np-cancel press" disabled={data.busy} onClick={() => setAsking(false)}>
            Отменить
          </button>
        </div>
      </div>
    ) : (
      <button type="button" className="btn-main wh-pretry" onClick={() => setAsking(true)}>
        Обработать ещё раз
      </button>
    );
  }
  return asking ? (
    <div className="pr-ask wh-ask">
      <span>{question}</span>
      <span className="pr-ask-btns">
        <button type="button" className="b34 on-surface hov" disabled={data.busy} onClick={() => void run()}>
          {data.busy ? 'Обрабатываем…' : 'Обработать'}
        </button>
        <button type="button" className="b34 on-surface hov" disabled={data.busy} onClick={() => setAsking(false)}>
          Отменить
        </button>
      </span>
    </div>
  ) : (
    <button type="button" className="btn-main wh-retry" onClick={() => setAsking(true)}>
      Обработать ещё раз
    </button>
  );
}

function subline(card: JournalCard): string {
  const repeats = deliveriesText(card.deliveries);
  return [card.event ? eventText(card.event) : 'событие не разобрано', dateTime(card.received_at), repeats].filter(Boolean).join(' · ');
}

function Verdict({ data, phone }: { data: Data; phone?: boolean }) {
  const card = data.card;
  if (!card) {
    return null;
  }
  return (
    <>
      <p className="wh-say" style={{ color: toneColor(card.tone) }}>
        {verdictText(card)}
      </p>
      {card.advice ? <p className="wh-advice">{card.advice}</p> : null}
      {data.result ? (
        <p className="wh-result" role="status" style={{ color: data.result.tone === 'bad' ? 'var(--bad)' : 'var(--ink)' }}>
          {data.result.text}
        </p>
      ) : null}
      <Retry data={data} phone={phone} />
    </>
  );
}

function headerFacts(card: JournalCard): Fact[] {
  return card.remote_ip ? [{ label: 'Адрес отправителя', value: card.remote_ip }, ...card.headers] : card.headers;
}

const MASK_NOTE = 'Подписи, ключи и токены скрыты полностью. У почты, телефона, имени, адреса, IP, карты и кошелька видна только часть.';

function CardDesk({ data, back }: { data: Data; back: string }) {
  const card = data.card;
  return (
    <div className="pr-page">
      <div className="pr-head">
        <h1 className="page-title tight">Деньги</h1>
      </div>
      <MoneyTabs />
      <Link to={back} className="wh-back hov">
        <Icon name="chevron-left" size={16} />
        Все уведомления
      </Link>
      {data.error && !card ? (
        <p className="err" role="alert">
          {data.error}
        </p>
      ) : null}
      {!card && !data.error ? <p className="muted">Загружаем уведомление…</p> : null}
      {card ? (
        <>
          <section className="card wh-head" aria-labelledby="whc-h">
            <h2 id="whc-h" className="section-title">
              Уведомление №{card.id} · {card.provider_title}
            </h2>
            <p className="muted">{subline(card)}</p>
            <Verdict data={data} />
          </section>
          <div className="wh-grid">
            <div className="wh-col">
              <section className="card card-pad wh-sec" aria-labelledby="whc-steps">
                <h3 id="whc-steps" className="wh-h3">
                  Ход обработки
                </h3>
                <Steps card={card} />
              </section>
              <section className="card card-pad wh-sec" aria-labelledby="whc-body">
                <h3 id="whc-body" className="wh-h3">
                  Тело уведомления
                </h3>
                <Body body={card.body} />
                {card.body.kind !== 'missing' ? <p className="cap">{MASK_NOTE}</p> : null}
              </section>
            </div>
            <div className="wh-col">
              <section className="card card-pad wh-sec" aria-labelledby="whc-facts">
                <h3 id="whc-facts" className="wh-h3">
                  Что сообщила платёжка
                </h3>
                {card.facts.length ? <Facts facts={card.facts} /> : <p className="muted wh-none">Уведомление не прошло проверку, поэтому не разбиралось</p>}
                {card.raw_event ? <p className="cap">Событие в терминах платёжки: {card.raw_event}</p> : null}
              </section>
              <section className="card card-pad wh-sec" aria-labelledby="whc-link">
                <h3 id="whc-link" className="wh-h3">
                  Платёж и клиент
                </h3>
                <Linked card={card} />
              </section>
              <section className="card card-pad wh-sec" aria-labelledby="whc-headers">
                <h3 id="whc-headers" className="wh-h3">
                  Заголовки
                </h3>
                {headerFacts(card).length ? <Facts facts={headerFacts(card)} /> : <p className="muted wh-none">Заголовков нет</p>}
                <p className="cap">Хранятся только служебные заголовки: подписи и ключи доступа не сохраняются.</p>
              </section>
            </div>
          </div>
        </>
      ) : null}
    </div>
  );
}

function CardPhone({ data, back }: { data: Data; back: string }) {
  const card = data.card;
  return (
    <div className="hp pt-phone">
      <PhoneHeader title={card ? `Уведомление №${card.id}` : 'Уведомление'} back={{ to: back, label: 'Все уведомления' }} />
      {data.error && !card ? (
        <p className="hp-quiet hp-card" role="alert">
          <span className="err">{data.error}</span>
        </p>
      ) : null}
      {!card && !data.error ? <p className="hp-quiet hp-card">Загружаем уведомление…</p> : null}
      {card ? (
        <>
          <section className="hp-card wh-pcard" aria-label="Итог">
            <span className="hp-title">{card.provider_title}</span>
            <span className="hp-note muted">{subline(card)}</span>
            <Verdict data={data} phone />
          </section>
          <section className="hp-group" aria-labelledby="whp-steps">
            <h2 id="whp-steps" className="hp-h2">
              Ход обработки
            </h2>
            <div className="hp-card wh-pbox">
              <Steps card={card} />
            </div>
          </section>
          <section className="hp-group" aria-labelledby="whp-facts">
            <h2 id="whp-facts" className="hp-h2">
              Что сообщила платёжка
            </h2>
            <div className="hp-card wh-pbox">
              {card.facts.length ? <Facts facts={card.facts} /> : <p className="muted wh-none">Уведомление не прошло проверку, поэтому не разбиралось</p>}
            </div>
          </section>
          <section className="hp-group" aria-labelledby="whp-link">
            <h2 id="whp-link" className="hp-h2">
              Платёж и клиент
            </h2>
            <div className="hp-card wh-pbox">
              <Linked card={card} />
            </div>
          </section>
          <section className="hp-group" aria-labelledby="whp-body">
            <h2 id="whp-body" className="hp-h2">
              Тело уведомления
            </h2>
            <div className="hp-card wh-pbox">
              <Body body={card.body} />
            </div>
            {card.body.kind !== 'missing' ? <p className="cap wh-pcap">{MASK_NOTE}</p> : null}
          </section>
          <section className="hp-group" aria-labelledby="whp-headers">
            <h2 id="whp-headers" className="hp-h2">
              Заголовки
            </h2>
            <div className="hp-card wh-pbox">{headerFacts(card).length ? <Facts facts={headerFacts(card)} /> : <p className="muted wh-none">Заголовков нет</p>}</div>
          </section>
        </>
      ) : null}
    </div>
  );
}
