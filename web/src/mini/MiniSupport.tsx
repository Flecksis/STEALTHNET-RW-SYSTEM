import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { Link, Navigate, useNavigate, useParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, dateTime, dayLabel, timeHM } from '../lib/format';
import { msg, t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { supportLink, supportTickets } from '../lib/support';
import { aiAnswering, aiName, aiTyping, authorName, handoffAfter, MAX_BODY, ticketTitle, visibleMessages, type ChatThread, type TicketBrief } from '../lib/supportChat';
import { AttachButton, DraftList, MessageFiles, useAttachments, type Attachment, type Attachments } from '../lib/ticketFiles';
import { BackHead, useMini } from './MiniApp';
import { downloadExternal, openExternal } from './telegram';
import { previewText, ticketGroups, ticketId, ticketMark, ticketWhen, unreadCount, waitingTicket, type TicketMark } from './tickets';

const FILES = '/api/client/support/files';
const TICKETS = '/api/client/support/tickets';
const LIST = '/app/support';
const REFRESH = 20_000;

const MARKS: Record<TicketMark, { text: string; tone: string }> = {
  fresh: { text: msg('Новый ответ'), tone: 'fresh' },
  answered: { text: msg('Есть ответ'), tone: 'ink2' },
  waiting: { text: msg('Ждёт ответа'), tone: 'muted' },
  closed: { text: msg('Закрыто'), tone: 'muted' },
};

async function openFile(file: Attachment, fail: (text: string) => void) {
  try {
    const link = await api<{ url: string; name: string }>(`${FILES}/${file.id}/link`, { method: 'POST', body: {} });
    downloadExternal(`${window.location.origin}${link.url}`, link.name);
  } catch (err) {
    fail(failText(err, t('Файл не открылся')));
  }
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function shortWhen(iso: string): string {
  const day = dayLabel(iso);
  return day === t('Сегодня') ? timeHM(iso) : day;
}

function stateLine(ticket: TicketBrief): string {
  if (ticket.status === 'closed' && ticket.closed_at) {
    return t('Закрыто {date}', { date: dateLong(ticket.closed_at) });
  }
  return t('{state} · открыто {date}', { state: t(MARKS[ticketMark(ticket)].text), date: dateLong(ticket.created_at) });
}

type ComposerProps = {
  label: string;
  placeholder: string;
  rows: number;
  files: Attachments;
  body: string;
  onBody: (body: string) => void;
  busy: boolean;
  onSend: () => void;
  before?: ReactNode;
  children?: ReactNode;
};

function Composer({ label, placeholder, rows, files, body, onBody, busy, onSend, before, children }: ComposerProps) {
  const canSend = (body.trim() !== '' || files.ids.length > 0) && files.ready;
  return (
    <div className="mn-card mn-pad">
      {before}
      <label className="mn-field">
        <span className="sr-only">{label}</span>
        <textarea
          rows={rows}
          maxLength={MAX_BODY}
          value={body}
          placeholder={placeholder}
          onChange={(e) => onBody(e.target.value)}
          onPaste={(e) => {
            if (files.enabled && e.clipboardData.files.length) {
              e.preventDefault();
              files.add(e.clipboardData.files);
            }
          }}
          disabled={busy}
        />
      </label>
      <DraftList files={files} prefix="mn" disabled={busy} />
      <div className="mn-send">
        <AttachButton files={files} className="mn-btn2 mn-clip press" disabled={busy} />
        <button type="button" className="mn-main sm press" onClick={onSend} disabled={busy || !canSend}>
          {busy ? t('Отправляем…') : t('Отправить')}
        </button>
      </div>
      {children}
    </div>
  );
}

function TicketForm({ replace, waiting }: { replace: boolean; waiting: TicketBrief | null }) {
  const navigate = useNavigate();
  const { site } = useMini();
  const [body, setBody] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const files = useAttachments(FILES, site.support_files ?? null);
  const [waitingBefore, waitingAfter] = waiting ? t('Обращение №{id} ещё ждёт ответа. Дополнить его можно {link}.', { id: waiting.id }).split('{link}') : [];

  const send = async () => {
    if (busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await api<{ ticket_id: number }>(TICKETS, { method: 'POST', body: { body: body.trim(), files: files.ids } });
      navigate(`${LIST}/${res.ticket_id}`, { replace });
    } catch (err) {
      setError(failText(err, t('Сообщение не отправилось')));
      setBusy(false);
    }
  };

  return (
    <Composer
      label={t('Сообщение')}
      placeholder={t('Например: не подключается iPhone')}
      rows={4}
      files={files}
      body={body}
      onBody={setBody}
      busy={busy}
      onSend={() => void send()}
      before={
        <>
          <span className="mn-ink2">
            {site.support_ai?.miniapp
              ? t('Сначала ответит {ai}. Если он не справится или вы попросите, подключится оператор.', { ai: aiName() })
              : t('Опишите вопрос. Ответ появится здесь и придёт в Telegram.')}
          </span>
          {waiting ? (
            <span className="mn-ink2">
              {waitingBefore}
              <Link to={`${LIST}/${waiting.id}`} className="mn-tk-link">
                {t('в переписке')}
              </Link>
              {waitingAfter}
            </span>
          ) : null}
        </>
      }
    >
      {error ? (
        <span className="mn-error" role="alert">
          {error}
        </span>
      ) : null}
    </Composer>
  );
}

function TicketRow({ ticket }: { ticket: TicketBrief }) {
  const mark = ticketMark(ticket);
  const { preview } = ticket;
  const text = previewText(preview) || (preview?.file ? t('Файл: {name}', { name: preview.file }) : '');
  return (
    <li>
      <Link to={`${LIST}/${ticket.id}`} className={mark === 'fresh' ? 'mn-payrow mn-tk fresh press' : 'mn-payrow mn-tk press'}>
        <span className="mn-payrow-main">
          <strong>{ticketTitle(ticket)}</strong>
          {preview && text ? (
            <span className="mn-tk-text">
              {authorName(preview.author)}: {text}
            </span>
          ) : null}
        </span>
        <span className="mn-payrow-side">
          <span className="mn-tk-when">{shortWhen(ticketWhen(ticket))}</span>
          <span className={`mn-tone ${MARKS[mark].tone}`}>{t(MARKS[mark].text)}</span>
        </span>
      </Link>
    </li>
  );
}

function LinkRow({ link }: { link: string }) {
  return (
    <div className="mn-list">
      <button type="button" className="mn-row press" onClick={() => openExternal(link)}>
        <Icon name="chat" size={21} />
        <span className="mn-row-name">{t('Чат поддержки')}</span>
        <Icon name="external" size={16} />
      </button>
    </div>
  );
}

function LinkCard({ link, text }: { link: string; text: string }) {
  return (
    <div className="mn-card mn-pad">
      <span className="mn-ink2">{text}</span>
      <button type="button" className="mn-main sm press" onClick={() => openExternal(link)}>
        {t('Написать в поддержку')}
      </button>
    </div>
  );
}

export function MiniSupport() {
  const { site, setUnread } = useMini();
  const [list, setList] = useState<TicketBrief[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const tickets = supportTickets(site);
  const link = supportLink(site);

  const load = useCallback(async () => {
    try {
      const found = await api<TicketBrief[]>(TICKETS);
      setList(found);
      setUnread(unreadCount(found));
      setError(null);
    } catch (err) {
      setError(failText(err, t('Обращения не загрузились')));
    }
  }, [setUnread]);

  useEffect(() => {
    void load();
    const refresh = () => {
      if (document.visibilityState === 'visible') {
        void load();
      }
    };
    const timer = window.setInterval(refresh, REFRESH);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, [load]);

  const { open, closed } = ticketGroups(list ?? []);
  const some = open.length + closed.length > 0;

  return (
    <main className={tickets && some ? 'mn-page support list' : 'mn-page support'}>
      <BackHead title={t('Поддержка')} />
      {!list && !error ? <p className="mn-muted">{t('Загружаем…')}</p> : null}
      {!list && error ? (
        <div className="mn-card mn-pad">
          <span className="mn-error" role="alert">
            {error}
          </span>
          <button type="button" className="mn-btn2 press" onClick={() => void load()}>
            {t('Повторить')}
          </button>
        </div>
      ) : null}
      {!tickets && link ? <LinkCard link={link} text={t('Поддержка отвечает в чате. Новые обращения здесь не принимаются.')} /> : null}
      {list && !some && tickets ? <TicketForm replace={false} waiting={null} /> : null}
      {open.length > 0 ? (
        <ul className="mn-list mn-payrows" aria-label={t('Открытые обращения')}>
          {open.map((ticket) => (
            <TicketRow key={ticket.id} ticket={ticket} />
          ))}
        </ul>
      ) : null}
      {closed.length > 0 ? (
        <>
          <h2 className="mn-tk-h">{t('Закрытые')}</h2>
          <ul className="mn-list mn-payrows" aria-label={t('Закрытые обращения')}>
            {closed.map((ticket) => (
              <TicketRow key={ticket.id} ticket={ticket} />
            ))}
          </ul>
        </>
      ) : null}
      {tickets && link ? <LinkRow link={link} /> : null}
      {tickets && some ? (
        <Link to={`${LIST}/new`} className="mn-main mn-paybtn press">
          <Icon name="plus" size={20} />
          {t('Новое обращение')}
        </Link>
      ) : null}
    </main>
  );
}

export function MiniNewTicket() {
  const { site } = useMini();
  const [waiting, setWaiting] = useState<TicketBrief | null>(null);

  useEffect(() => {
    let alive = true;
    api<TicketBrief[]>(TICKETS)
      .then((list) => alive && setWaiting(waitingTicket(list)))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  if (!supportTickets(site)) {
    return <Navigate to={LIST} replace />;
  }
  return (
    <main className="mn-page support">
      <BackHead title={t('Новое обращение')} to={LIST} />
      <TicketForm replace waiting={waiting} />
    </main>
  );
}

export function MiniTicket() {
  const { id } = useParams();
  const { site, setUnread } = useMini();
  const ticket = ticketId(id);
  const [thread, setThread] = useState<ChatThread | null>(null);
  const [lost, setLost] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [body, setBody] = useState('');
  const [busy, setBusy] = useState(false);
  const [ask, setAsk] = useState(false);
  const files = useAttachments(FILES, site.support_files ?? null);
  const bottom = useRef<HTMLDivElement>(null);
  const seen = useRef(0);
  const typing = aiTyping(thread);
  const tickets = supportTickets(site);
  const link = supportLink(site);

  const show = useCallback(
    (next: ChatThread) => {
      setThread(next);
      setUnread(next.unread ?? 0);
    },
    [setUnread],
  );

  const load = useCallback(async () => {
    if (ticket === null) {
      setLost(t('Обращение не найдено'));
      return;
    }
    try {
      show(await api<ChatThread>(`${TICKETS}/${ticket}`));
      setLost(null);
    } catch (err) {
      setLost(err instanceof ApiError && err.status === 404 ? t('Обращение не найдено') : failText(err, t('Обращение не загрузилось')));
    }
  }, [ticket, show]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const refresh = () => {
      if (document.visibilityState === 'visible') {
        void load();
      }
    };
    const timer = window.setInterval(refresh, typing ? 2_500 : 15_000);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, [load, typing]);

  useEffect(() => {
    const count = (thread?.messages.length ?? 0) + (typing ? 1 : 0);
    if (count > seen.current) {
      seen.current = count;
      bottom.current?.scrollIntoView({ block: 'end' });
    }
  }, [thread, typing]);

  useEffect(() => {
    if (ask) {
      bottom.current?.scrollIntoView({ block: 'end' });
    }
  }, [ask]);

  const post = async (path: string, payload: unknown, fallback: string) => {
    setBusy(true);
    setError(null);
    try {
      show(await api<ChatThread>(path, { method: 'POST', body: payload }));
      return true;
    } catch (err) {
      setError(failText(err, fallback));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const reply = async () => {
    if (!busy && (await post(`${TICKETS}/${ticket}`, { body: body.trim(), files: files.ids }, t('Сообщение не отправилось')))) {
      setBody('');
      files.reset();
    }
  };

  const close = async () => {
    setBusy(true);
    setError(null);
    try {
      await api(`${TICKETS}/${ticket}/close`, { method: 'POST', body: {} });
      setAsk(false);
      await load();
    } catch (err) {
      setError(failText(err, t('Обращение не закрылось')));
    } finally {
      setBusy(false);
    }
  };

  const problem = error ?? (thread ? null : lost);
  const messages = visibleMessages(thread);
  const handoff = handoffAfter(thread);
  const closed = thread?.ticket.status === 'closed';
  const handoffNote = (
    <li key="handoff" className="mn-chat-note">
      {t('Вопрос передан оператору. Ответ придёт сюда и в Telegram.')}
    </li>
  );

  return (
    <main className="mn-page support">
      <BackHead title={ticket === null ? t('Обращение') : t('Обращение №{id}', { id: ticket })} to={LIST} />
      {thread ? <p className="mn-tk-sub">{stateLine(thread.ticket)}</p> : null}
      {!thread && !lost ? <p className="mn-muted">{t('Загружаем переписку…')}</p> : null}
      {thread ? (
        <ol className="mn-chat" aria-label={t('Переписка')}>
          {messages.map((m, i) => {
            const mine = m.author === 'client';
            return [
              handoff === i ? handoffNote : null,
              <li key={`${m.created_at}-${i}`} className={mine ? 'mn-msg mine' : 'mn-msg'}>
                <span className="mn-msg-who">{authorName(m.author)}</span>
                {m.body ? <span className="mn-msg-body">{m.body}</span> : null}
                <MessageFiles files={m.attachments} base={FILES} prefix="mn" box={[240, 220]} viaFetch onFile={(file) => void openFile(file, setError)} />
                <span className="mn-msg-at">{dateTime(m.created_at)}</span>
              </li>,
            ];
          })}
          {handoff !== null && handoff >= messages.length ? handoffNote : null}
          {typing ? (
            <li className="mn-chat-note" aria-live="polite">
              {t('{ai} пишет ответ…', { ai: aiName() })}
            </li>
          ) : null}
        </ol>
      ) : null}
      {problem ? (
        <span className="mn-error" role="alert">
          {problem}
        </span>
      ) : null}
      {thread && closed && tickets ? (
        <div className="mn-card mn-pad">
          <span className="mn-ink2">{t('Обращение закрыто. Если вопрос остался, начните новое.')}</span>
          <Link to={`${LIST}/new`} className="mn-main sm press">
            {t('Новое обращение')}
          </Link>
        </div>
      ) : null}
      {thread && closed && !tickets && link ? <LinkCard link={link} text={t('Обращение закрыто. Если вопрос остался, напишите в чат поддержки.')} /> : null}
      {thread && !closed ? (
        <Composer label={t('Ответ')} placeholder={t('Напишите сообщение')} rows={3} files={files} body={body} onBody={setBody} busy={busy} onSend={() => void reply()}>
          {aiAnswering(thread) ? (
            <button type="button" className="mn-btn2 press" onClick={() => void post(`${TICKETS}/${ticket}/operator`, {}, t('Не получилось позвать оператора'))} disabled={busy}>
              {t('Позвать оператора')}
            </button>
          ) : null}
          {ask ? (
            <div className="mn-ask">
              <span>
                <strong>{t('Закрыть обращение?')}</strong> {tickets ? t('Писать в него больше будет нельзя, но можно начать новое.') : t('Писать в него больше будет нельзя.')}
              </span>
              <div className="mn-ask-grid">
                <button type="button" className="mn-ask-ok press" onClick={() => void close()} disabled={busy}>
                  {t('Закрыть')}
                </button>
                <button type="button" className="mn-ask-cancel press" onClick={() => setAsk(false)} disabled={busy}>
                  {t('Отменить')}
                </button>
              </div>
            </div>
          ) : (
            <button type="button" className="mn-link press" onClick={() => setAsk(true)} disabled={busy}>
              {t('Вопрос решён, закрыть')}
            </button>
          )}
        </Composer>
      ) : null}
      <div ref={bottom} className="mn-tk-end" />
    </main>
  );
}
