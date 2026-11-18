import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, dateTime } from '../lib/format';
import { t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { supportLink, supportTickets } from '../lib/support';
import { aiAnswering, aiName, aiTyping, authorName, handoffAfter, MAX_BODY, ticketTitle, visibleMessages, type ChatThread, type TicketBrief } from '../lib/supportChat';
import { AttachButton, DraftList, MessageFiles, useAttachments, type FileLimits } from '../lib/ticketFiles';
import { Card, useCabinet } from './CabinetApp';

const FILES = '/api/client/support/files';

export type { TicketBrief };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function ticketState(ticket: TicketBrief): { text: string; tone: string } {
  if (ticket.status === 'closed') {
    return { text: t('Закрыто'), tone: 'muted' };
  }
  if (ticket.last_author === 'client') {
    return { text: t('Ждёт ответа'), tone: 'muted' };
  }
  return { text: t('Есть ответ'), tone: 'accent' };
}

function LinkCard({ url, alone }: { url: string; alone: boolean }) {
  const telegram = /^https:\/\/(t\.me|telegram\.me)\//i.test(url) || url.startsWith('tg://');
  return (
    <a className="cb-card cb-help hov" href={url} target="_blank" rel="noreferrer">
      <span>
        <strong>{alone ? t('Написать в поддержку') : t('Чат поддержки')}</strong>
        <span className="cb-small cb-muted">{telegram ? t('Откроется чат в Telegram') : t('Откроется страница поддержки')}</span>
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
  return <TicketsPage link={link} tickets={tickets} ai={tickets && site.support_ai?.cabinet === true} limits={site.support_files ?? null} />;
}

function TicketsPage({ link, tickets, ai, limits }: { link: string | null; tickets: boolean; ai: boolean; limits: FileLimits | null }) {
  const navigate = useNavigate();
  const [list, setList] = useState<TicketBrief[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [body, setBody] = useState('');
  const [busy, setBusy] = useState(false);
  const files = useAttachments(FILES, limits);
  const canSend = (body.trim() !== '' || files.ids.length > 0) && files.ready;

  const load = useCallback(async () => {
    try {
      setList(await api<TicketBrief[]>('/api/client/support/tickets'));
      setError(null);
    } catch (err) {
      setError(failText(err, t('Обращения не загрузились')));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const send = async () => {
    const text = body.trim();
    if (!canSend || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await api<{ ticket_id: number }>('/api/client/support/tickets', { method: 'POST', body: { body: text, files: files.ids } });
      setBody('');
      files.reset();
      navigate(`/cabinet/help/${res.ticket_id}`);
    } catch (err) {
      setError(failText(err, t('Сообщение не отправилось')));
    } finally {
      setBusy(false);
    }
  };

  const open = (list ?? []).filter((ticket) => ticket.status === 'open');
  const closed = (list ?? []).filter((ticket) => ticket.status === 'closed');
  const waiting = open.find((ticket) => ticket.last_author === 'client');
  const [waitingBefore, waitingAfter] = waiting ? t('Обращение №{id} ещё ждёт ответа. Дополнить его можно {link}.', { id: waiting.id }).split('{link}') : [];

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <h1 className="cb-title">{t('Помощь')}</h1>
        <p className="cb-lead muted">
          {tickets
            ? ai
              ? t('Опишите вопрос — сначала ответит {ai}. Если он не справится или вы попросите, подключится оператор.', { ai: aiName() })
              : link
                ? t('Напишите здесь — ответ появится на этой странице. Или откройте чат поддержки.')
                : t('Опишите вопрос — ответ появится на этой странице.')
            : t('Поддержка отвечает в чате — кнопка ниже откроет его.')}
        </p>
      </header>
      {link ? <LinkCard url={link} alone={!tickets} /> : null}
      {tickets ? (
        <Card className="cb-newticket" label={t('Новое обращение')}>
          <h2 className="cb-h2">{waiting ? t('Ещё вопрос') : t('Новое обращение')}</h2>
          {waiting ? (
            <p className="cb-small cb-muted">
              {waitingBefore}
              <Link to={`/cabinet/help/${waiting.id}`} className="cb-accent-link">
                {t('в переписке')}
              </Link>
              {waitingAfter}
            </p>
          ) : null}
          <label className="cb-field">
            <span className="cb-sr">{t('Сообщение')}</span>
            <textarea
              rows={4}
              maxLength={MAX_BODY}
              value={body}
              placeholder={t('Что случилось? Например: не подключается iPhone, после оплаты не продлилась подписка')}
              onChange={(e) => setBody(e.target.value)}
              onPaste={(e) => {
                if (files.enabled && e.clipboardData.files.length) {
                  e.preventDefault();
                  files.add(e.clipboardData.files);
                }
              }}
              disabled={busy}
            />
          </label>
          <DraftList files={files} prefix="cb" disabled={busy} />
          <div className="cb-btns">
            <button type="button" className="cb-main" onClick={() => void send()} disabled={busy || !canSend}>
              {busy ? t('Отправляем…') : t('Отправить')}
            </button>
            <AttachButton files={files} className="cb-text cb-clip" disabled={busy} label={t('Прикрепить файл')} />
          </div>
        </Card>
      ) : null}
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
      {list && list.length > 0 ? (
        <Card className="cb-paylist" label={t('Обращения')}>
          <ul className="cb-payrows">
            {[...open, ...closed].map((ticket) => {
              const st = ticketState(ticket);
              return (
                <li key={ticket.id} className="cb-payrow cb-ticketrow">
                  <Link to={`/cabinet/help/${ticket.id}`} className="cb-ticketlink">
                    <span className="cb-payrow-main">
                      <strong>{ticketTitle(ticket)}</strong>
                      <span className="cb-small cb-muted">{dateTime(ticket.status === 'closed' && ticket.closed_at ? ticket.closed_at : ticket.updated_at)}</span>
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
  const files = useAttachments(FILES, site.support_files ?? null);
  const canSend = (body.trim() !== '' || files.ids.length > 0) && files.ready;
  const bottom = useRef<HTMLDivElement>(null);
  const seen = useRef(0);
  const typing = aiTyping(thread);

  const load = useCallback(async () => {
    try {
      setThread(await api<ChatThread>(`/api/client/support/tickets/${id}`));
      setError(null);
    } catch (err) {
      setError(failText(err, t('Обращение не загрузилось')));
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
    if (!canSend || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setThread(await api<ChatThread>(`/api/client/support/tickets/${id}`, { method: 'POST', body: { body: text, files: files.ids } }));
      setBody('');
      files.reset();
    } catch (err) {
      setError(failText(err, t('Сообщение не отправилось')));
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
      setError(failText(err, t('Не получилось позвать оператора')));
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
      setError(failText(err, t('Обращение не закрылось')));
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
      {t('Вопрос передан оператору. Ответ придёт сюда.')}
    </li>
  );

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <Link to="/cabinet/help" className="cb-accent-link cb-back">
          <Icon name="chevron-left" size={18} />
          {t('Все обращения')}
        </Link>
        <h1 className="cb-title">{t('Обращение №{id}', { id })}</h1>
        {ticket && st ? (
          <p className="cb-lead muted">
            {t('{state} · открыто {date}', { state: st.text, date: dateLong(ticket.created_at, true) })}
          </p>
        ) : null}
      </header>
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
      {!thread && !error ? <p className="cb-muted">{t('Загружаем переписку…')}</p> : null}
      {thread ? (
        <Card className="cb-thread" label={t('Переписка')}>
          <ol className="cb-msgs">
            {messages.map((m, i) => {
              const mine = m.author === 'client';
              return [
                handoff === i ? handoffNote : null,
                <li key={`${m.created_at}-${i}`} className={mine ? 'cb-msg mine' : 'cb-msg'}>
                  <span className="cb-msg-who cb-small">{authorName(m.author)}</span>
                  {m.body ? <span className="cb-msg-body">{m.body}</span> : null}
                  <MessageFiles files={m.attachments} base={FILES} prefix="cb" box={[280, 240]} />
                  <span className="cb-msg-at cb-small cb-muted">{dateTime(m.created_at)}</span>
                </li>,
              ];
            })}
            {handoff !== null && handoff >= messages.length ? handoffNote : null}
            {typing ? (
              <li className="cb-chat-note" aria-live="polite">
                {t('{ai} пишет ответ…', { ai: aiName() })}
              </li>
            ) : null}
          </ol>
          <div ref={bottom} />
          {closed ? (
            <div className="cb-thread-end">
              <p className="cb-ink2">{t('Обращение закрыто. Если вопрос остался, начните новое.')}</p>
              <div className="cb-btns">
                <Link to="/cabinet/help" className="cb-main">
                  {t('Новое обращение')}
                </Link>
                {link ? (
                  <a className="cb-second" href={link} target="_blank" rel="noreferrer">
                    {t('Чат поддержки')}
                  </a>
                ) : null}
              </div>
            </div>
          ) : (
            <div className="cb-reply">
              <label className="cb-field">
                <span className="cb-sr">{t('Ответ')}</span>
                <textarea
                  rows={3}
                  maxLength={MAX_BODY}
                  value={body}
                  placeholder={t('Напишите сообщение')}
                  onChange={(e) => setBody(e.target.value)}
                  onPaste={(e) => {
                    if (files.enabled && e.clipboardData.files.length) {
                      e.preventDefault();
                      files.add(e.clipboardData.files);
                    }
                  }}
                  disabled={busy}
                />
              </label>
              <DraftList files={files} prefix="cb" disabled={busy} />
              <div className="cb-btns">
                <button type="button" className="cb-main" onClick={() => void reply()} disabled={busy || !canSend}>
                  {busy ? t('Отправляем…') : t('Отправить')}
                </button>
                <AttachButton files={files} className="cb-text cb-clip" disabled={busy} label={t('Прикрепить файл')} />
                {answering ? (
                  <button type="button" className="cb-text" onClick={() => void callOperator()} disabled={busy}>
                    {t('Позвать оператора')}
                  </button>
                ) : null}
                {!ask ? (
                  <button type="button" className="cb-text" onClick={() => setAsk(true)} disabled={busy}>
                    {t('Вопрос решён, закрыть')}
                  </button>
                ) : null}
              </div>
              {ask ? (
                <div className="cb-ask">
                  <p>
                    <strong>{t('Закрыть обращение?')}</strong> {t('Писать в него больше будет нельзя, но можно начать новое.')}
                  </p>
                  <span className="cb-ask-btns">
                    <button type="button" className="cb-main sm" onClick={() => void close()} disabled={busy}>
                      {t('Закрыть')}
                    </button>
                    <button type="button" className="cb-cancel" onClick={() => setAsk(false)} disabled={busy}>
                      {t('Отменить')}
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
