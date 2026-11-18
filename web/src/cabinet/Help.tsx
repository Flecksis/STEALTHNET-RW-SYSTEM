import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, dateTime } from '../lib/format';
import { Icon } from '../lib/icons';
import { supportLink, supportTickets } from '../lib/support';
import { AI_NAME, aiAnswering, aiTyping, authorName, handoffAfter, MAX_BODY, ticketTitle, visibleMessages, type ChatThread, type TicketBrief } from '../lib/supportChat';
import { Card, useCabinet } from './CabinetApp';

export type { TicketBrief };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function ticketState(ticket: TicketBrief): { text: string; tone: string } {
  if (ticket.status === 'closed') {
    return { text: 'Закрыто', tone: 'muted' };
  }
  if (ticket.last_author === 'client') {
    return { text: 'Ждёт ответа', tone: 'muted' };
  }
  return { text: 'Есть ответ', tone: 'accent' };
}

function LinkCard({ url, alone }: { url: string; alone: boolean }) {
  const telegram = /^https:\/\/(t\.me|telegram\.me)\//i.test(url) || url.startsWith('tg://');
  return (
    <a className="cb-card cb-help hov" href={url} target="_blank" rel="noreferrer">
      <span>
        <strong>{alone ? 'Написать в поддержку' : 'Чат поддержки'}</strong>
        <span className="cb-small cb-muted">{telegram ? 'Откроется чат в Telegram' : 'Откроется страница поддержки'}</span>
      </span>
      <Icon name="chevron-right" size={20} />
    </a>
  );
}

export function HelpPage() {
  const { site } = useCabinet();
  const { id } = useParams();
  const ticketId = id ? Number.parseInt(id, 10) : null;
  if (ticketId && Number.isFinite(ticketId)) {
    return <TicketPage id={ticketId} />;
  }
  const link = supportLink(site);
  const tickets = supportTickets(site);
  return <TicketsPage link={link} tickets={tickets} ai={tickets && site.support_ai?.cabinet === true} />;
}

function TicketsPage({ link, tickets, ai }: { link: string | null; tickets: boolean; ai: boolean }) {
  const navigate = useNavigate();
  const [list, setList] = useState<TicketBrief[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [body, setBody] = useState('');
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setList(await api<TicketBrief[]>('/api/client/support/tickets'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Обращения не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const send = async () => {
    const text = body.trim();
    if (!text || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await api<{ ticket_id: number }>('/api/client/support/tickets', { method: 'POST', body: { body: text } });
      setBody('');
      navigate(`/cabinet/help/${res.ticket_id}`);
    } catch (err) {
      setError(failText(err, 'Сообщение не отправилось'));
    } finally {
      setBusy(false);
    }
  };

  const open = (list ?? []).filter((t) => t.status === 'open');
  const closed = (list ?? []).filter((t) => t.status === 'closed');
  const waiting = open.find((t) => t.last_author === 'client');

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <h1 className="cb-title">Помощь</h1>
        <p className="cb-lead muted">
          {tickets
            ? ai
              ? `Опишите вопрос — сначала ответит ${AI_NAME}. Если он не справится или вы попросите, подключится оператор.`
              : link
                ? 'Напишите здесь — ответ появится на этой странице. Или откройте чат поддержки.'
                : 'Опишите вопрос — ответ появится на этой странице.'
            : 'Поддержка отвечает в чате — кнопка ниже откроет его.'}
        </p>
      </header>
      {link ? <LinkCard url={link} alone={!tickets} /> : null}
      {tickets ? (
        <Card className="cb-newticket" label="Новое обращение">
          <h2 className="cb-h2">{waiting ? 'Ещё вопрос' : 'Новое обращение'}</h2>
          {waiting ? (
            <p className="cb-small cb-muted">
              Обращение №{waiting.id} ещё ждёт ответа. Дополнить его можно{' '}
              <Link to={`/cabinet/help/${waiting.id}`} className="cb-accent-link">
                в переписке
              </Link>
              .
            </p>
          ) : null}
          <label className="cb-field">
            <span className="cb-sr">Сообщение</span>
            <textarea rows={4} maxLength={MAX_BODY} value={body} placeholder="Что случилось? Например: не подключается iPhone, после оплаты не продлилась подписка" onChange={(e) => setBody(e.target.value)} disabled={busy} />
          </label>
          <div className="cb-btns">
            <button type="button" className="cb-main" onClick={() => void send()} disabled={busy || !body.trim()}>
              {busy ? 'Отправляем…' : 'Отправить'}
            </button>
          </div>
        </Card>
      ) : null}
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
      {list && list.length > 0 ? (
        <Card className="cb-paylist" label="Обращения">
          <ul className="cb-payrows">
            {[...open, ...closed].map((t) => {
              const st = ticketState(t);
              return (
                <li key={t.id} className="cb-payrow cb-ticketrow">
                  <Link to={`/cabinet/help/${t.id}`} className="cb-ticketlink">
                    <span className="cb-payrow-main">
                      <strong>{ticketTitle(t)}</strong>
                      <span className="cb-small cb-muted">{dateTime(t.status === 'closed' && t.closed_at ? t.closed_at : t.updated_at)}</span>
                    </span>
                    <span className="cb-payrow-side">
                      <span className={`cb-small cb-status ${st.tone}`}>{st.text}</span>
                    </span>
                  </Link>
                </li>
              );
            })}
          </ul>
        </Card>
      ) : null}
    </div>
  );
}

function TicketPage({ id }: { id: number }) {
  const { site } = useCabinet();
  const [thread, setThread] = useState<ChatThread | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [body, setBody] = useState('');
  const [busy, setBusy] = useState(false);
  const [ask, setAsk] = useState(false);
  const bottom = useRef<HTMLDivElement>(null);
  const seen = useRef(0);
  const typing = aiTyping(thread);

  const load = useCallback(async () => {
    try {
      setThread(await api<ChatThread>(`/api/client/support/tickets/${id}`));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Обращение не загрузилось'));
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const timer = window.setInterval(
      () => {
        if (document.visibilityState === 'visible') {
          void load();
        }
      },
      typing ? 2_500 : 15_000,
    );
    return () => window.clearInterval(timer);
  }, [load, typing]);

  useEffect(() => {
    const count = (thread?.messages.length ?? 0) + (typing ? 1 : 0);
    if (count > seen.current) {
      seen.current = count;
      bottom.current?.scrollIntoView({ block: 'end' });
    }
  }, [thread, typing]);

  const reply = async () => {
    const text = body.trim();
    if (!text || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setThread(await api<ChatThread>(`/api/client/support/tickets/${id}`, { method: 'POST', body: { body: text } }));
      setBody('');
    } catch (err) {
      setError(failText(err, 'Сообщение не отправилось'));
    } finally {
      setBusy(false);
    }
  };

  const callOperator = async () => {
    setBusy(true);
    setError(null);
    try {
      setThread(await api<ChatThread>(`/api/client/support/tickets/${id}/operator`, { method: 'POST', body: {} }));
    } catch (err) {
      setError(failText(err, 'Не получилось позвать оператора'));
    } finally {
      setBusy(false);
    }
  };

  const close = async () => {
    setBusy(true);
    try {
      await api(`/api/client/support/tickets/${id}/close`, { method: 'POST', body: {} });
      setAsk(false);
      await load();
    } catch (err) {
      setError(failText(err, 'Обращение не закрылось'));
    } finally {
      setBusy(false);
    }
  };

  const ticket = thread?.ticket ?? null;
  const st = ticket ? ticketState(ticket) : null;
  const closed = ticket?.status === 'closed';
  const messages = visibleMessages(thread);
  const handoff = handoffAfter(thread);
  const answering = aiAnswering(thread);
  const link = supportLink(site);
  const handoffNote = (
    <li key="handoff" className="cb-chat-note">
      Вопрос передан оператору. Ответ придёт сюда.
    </li>
  );

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <Link to="/cabinet/help" className="cb-accent-link cb-back">
          <Icon name="chevron-left" size={18} />
          Все обращения
        </Link>
        <h1 className="cb-title">Обращение №{id}</h1>
        {ticket && st ? (
          <p className="cb-lead muted">
            {st.text} · открыто {dateLong(ticket.created_at, true)}
          </p>
        ) : null}
      </header>
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
      {!thread && !error ? <p className="cb-muted">Загружаем переписку…</p> : null}
      {thread ? (
        <Card className="cb-thread" label="Переписка">
          <ol className="cb-msgs">
            {messages.map((m, i) => {
              const mine = m.author === 'client';
              return [
                handoff === i ? handoffNote : null,
                <li key={`${m.created_at}-${i}`} className={mine ? 'cb-msg mine' : 'cb-msg'}>
                  <span className="cb-msg-who cb-small">{authorName(m.author)}</span>
                  <span className="cb-msg-body">{m.body}</span>
                  <span className="cb-msg-at cb-small cb-muted">{dateTime(m.created_at)}</span>
                </li>,
              ];
            })}
            {handoff !== null && handoff >= messages.length ? handoffNote : null}
            {typing ? (
              <li className="cb-chat-note" aria-live="polite">
                {AI_NAME} пишет ответ…
              </li>
            ) : null}
          </ol>
          <div ref={bottom} />
          {closed ? (
            <div className="cb-thread-end">
              <p className="cb-ink2">Обращение закрыто. Если вопрос остался, начните новое.</p>
              <div className="cb-btns">
                <Link to="/cabinet/help" className="cb-main">
                  Новое обращение
                </Link>
                {link ? (
                  <a className="cb-second" href={link} target="_blank" rel="noreferrer">
                    Чат поддержки
                  </a>
                ) : null}
              </div>
            </div>
          ) : (
            <div className="cb-reply">
              <label className="cb-field">
                <span className="cb-sr">Ответ</span>
                <textarea rows={3} maxLength={MAX_BODY} value={body} placeholder="Напишите сообщение" onChange={(e) => setBody(e.target.value)} disabled={busy} />
              </label>
              <div className="cb-btns">
                <button type="button" className="cb-main" onClick={() => void reply()} disabled={busy || !body.trim()}>
                  {busy ? 'Отправляем…' : 'Отправить'}
                </button>
                {answering ? (
                  <button type="button" className="cb-text" onClick={() => void callOperator()} disabled={busy}>
                    Позвать оператора
                  </button>
                ) : null}
                {!ask ? (
                  <button type="button" className="cb-text" onClick={() => setAsk(true)} disabled={busy}>
                    Вопрос решён, закрыть
                  </button>
                ) : null}
              </div>
              {ask ? (
                <div className="cb-ask">
                  <p>
                    <strong>Закрыть обращение?</strong> Писать в него больше будет нельзя, но можно начать новое.
                  </p>
                  <span className="cb-ask-btns">
                    <button type="button" className="cb-main sm" onClick={() => void close()} disabled={busy}>
                      Закрыть
                    </button>
                    <button type="button" className="cb-cancel" onClick={() => setAsk(false)} disabled={busy}>
                      Отменить
                    </button>
                  </span>
                </div>
              ) : null}
            </div>
          )}
        </Card>
      ) : null}
    </div>
  );
}
