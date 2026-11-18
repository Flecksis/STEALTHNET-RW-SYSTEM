import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateTime } from '../lib/format';
import { Icon } from '../lib/icons';
import { AI_NAME, aiAnswering, aiTyping, authorName, handoffAfter, MAX_BODY, ticketTitle, visibleMessages, type ChatThread, type TicketBrief } from '../lib/supportChat';
import { BackHead } from './MiniApp';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function stateOf(ticket: TicketBrief): { text: string; tone: string } {
  if (ticket.status === 'closed') {
    return { text: 'Закрыто', tone: 'muted' };
  }
  if (ticket.last_author === 'client') {
    return { text: 'Ждёт ответа', tone: 'muted' };
  }
  return { text: 'Есть ответ', tone: 'ink2' };
}

export function MiniSupport() {
  const navigate = useNavigate();
  const [list, setList] = useState<TicketBrief[] | null>(null);
  const [body, setBody] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api<TicketBrief[]>('/api/client/support/tickets')
      .then(setList)
      .catch((err) => setError(failText(err, 'Обращения не загрузились')));
  }, []);

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
      navigate(`/app/support/${res.ticket_id}`);
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
    <main className="mn-page">
      <BackHead title="Поддержка" />
      <div className="mn-card mn-pad">
        <strong>{waiting ? 'Ещё вопрос' : 'Новый вопрос'}</strong>
        <span className="mn-ink2">Сначала ответит {AI_NAME}. Если он не справится или вы попросите, подключится оператор.</span>
        <label className="mn-field">
          <span className="sr-only">Сообщение</span>
          <textarea rows={4} maxLength={MAX_BODY} value={body} placeholder="Например: не подключается iPhone" onChange={(e) => setBody(e.target.value)} disabled={busy} />
        </label>
        {error ? <span className="mn-error">{error}</span> : null}
        <button type="button" className="mn-main sm press" onClick={() => void send()} disabled={busy || !body.trim()}>
          {busy ? 'Отправляем…' : 'Отправить'}
        </button>
      </div>
      {list && list.length > 0 ? (
        <div className="mn-list" aria-label="Обращения">
          {[...open, ...closed].map((t) => {
            const state = stateOf(t);
            return (
              <Link key={t.id} to={`/app/support/${t.id}`} className="mn-row press mn-ticket">
                <span className="mn-ticket-text">
                  <span className="mn-row-name">{ticketTitle(t)}</span>
                  <span className={`mn-tone ${state.tone}`}>{state.text}</span>
                </span>
                <Icon name="chevron-right" size={16} />
              </Link>
            );
          })}
        </div>
      ) : null}
      {list === null && !error ? <p className="mn-muted">Загружаем…</p> : null}
    </main>
  );
}

export function MiniTicket() {
  const { id = '' } = useParams();
  const ticketId = Number.parseInt(id, 10);
  const [thread, setThread] = useState<ChatThread | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [body, setBody] = useState('');
  const [busy, setBusy] = useState(false);
  const [ask, setAsk] = useState(false);
  const bottom = useRef<HTMLDivElement>(null);
  const seen = useRef(0);
  const typing = aiTyping(thread);

  const load = useCallback(async () => {
    if (!Number.isFinite(ticketId)) {
      setError('Обращение не найдено');
      return;
    }
    try {
      setThread(await api<ChatThread>(`/api/client/support/tickets/${ticketId}`));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Обращение не загрузилось'));
    }
  }, [ticketId]);

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

  const post = async (path: string, payload: unknown, fallback: string) => {
    setBusy(true);
    setError(null);
    try {
      setThread(await api<ChatThread>(path, { method: 'POST', body: payload }));
      return true;
    } catch (err) {
      setError(failText(err, fallback));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const reply = async () => {
    const text = body.trim();
    if (!text || busy) {
      return;
    }
    if (await post(`/api/client/support/tickets/${ticketId}`, { body: text }, 'Сообщение не отправилось')) {
      setBody('');
    }
  };

  const close = async () => {
    setBusy(true);
    try {
      await api(`/api/client/support/tickets/${ticketId}/close`, { method: 'POST', body: {} });
      setAsk(false);
      await load();
    } catch (err) {
      setError(failText(err, 'Обращение не закрылось'));
    } finally {
      setBusy(false);
    }
  };

  const messages = visibleMessages(thread);
  const handoff = handoffAfter(thread);
  const answering = aiAnswering(thread);
  const closed = thread?.ticket.status === 'closed';
  const handoffNote = (
    <li key="handoff" className="mn-chat-note">
      Вопрос передан оператору. Ответ придёт сюда и в Telegram.
    </li>
  );

  return (
    <main className="mn-page">
      <BackHead title={Number.isFinite(ticketId) ? `Обращение №${ticketId}` : 'Обращение'} to="/app/support" />
      {!thread && !error ? <p className="mn-muted">Загружаем переписку…</p> : null}
      {thread ? (
        <ol className="mn-chat" aria-label="Переписка">
          {messages.map((m, i) => {
            const mine = m.author === 'client';
            return [
              handoff === i ? handoffNote : null,
              <li key={`${m.created_at}-${i}`} className={mine ? 'mn-msg mine' : 'mn-msg'}>
                <span className="mn-msg-who">{authorName(m.author)}</span>
                <span className="mn-msg-body">{m.body}</span>
                <span className="mn-msg-at">{dateTime(m.created_at)}</span>
              </li>,
            ];
          })}
          {handoff !== null && handoff >= messages.length ? handoffNote : null}
          {typing ? (
            <li className="mn-chat-note" aria-live="polite">
              {AI_NAME} пишет ответ…
            </li>
          ) : null}
        </ol>
      ) : null}
      <div ref={bottom} />
      {error ? <span className="mn-error">{error}</span> : null}
      {thread && closed ? (
        <div className="mn-card mn-pad">
          <span className="mn-ink2">Обращение закрыто. Если вопрос остался, задайте новый.</span>
          <Link to="/app/support" className="mn-main sm press">
            Новый вопрос
          </Link>
        </div>
      ) : null}
      {thread && !closed ? (
        <div className="mn-card mn-pad">
          <label className="mn-field">
            <span className="sr-only">Ответ</span>
            <textarea rows={3} maxLength={MAX_BODY} value={body} placeholder="Напишите сообщение" onChange={(e) => setBody(e.target.value)} disabled={busy} />
          </label>
          <button type="button" className="mn-main sm press" onClick={() => void reply()} disabled={busy || !body.trim()}>
            {busy ? 'Отправляем…' : 'Отправить'}
          </button>
          {answering ? (
            <button type="button" className="mn-btn2 press" onClick={() => void post(`/api/client/support/tickets/${ticketId}/operator`, {}, 'Не получилось позвать оператора')} disabled={busy}>
              Позвать оператора
            </button>
          ) : null}
          {ask ? (
            <div className="mn-ask">
              <span>
                <strong>Закрыть обращение?</strong> Писать в него будет нельзя, но можно задать новый вопрос.
              </span>
              <div className="mn-ask-grid">
                <button type="button" className="mn-ask-ok press" onClick={() => void close()} disabled={busy}>
                  Закрыть
                </button>
                <button type="button" className="mn-ask-cancel press" onClick={() => setAsk(false)} disabled={busy}>
                  Отменить
                </button>
              </div>
            </div>
          ) : (
            <button type="button" className="mn-link press" onClick={() => setAsk(true)} disabled={busy}>
              Вопрос решён, закрыть
            </button>
          )}
        </div>
      ) : null}
    </main>
  );
}
