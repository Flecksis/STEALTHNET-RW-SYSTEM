import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { dateLong, dateNumeric, days, daysLeft, groupDigits, money, NBSP, timeHM, traffic, trafficPair } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { useSession } from '../session';
import { planTitle } from '../client/SubscriptionBlock';
import { emailOf, liveSubscriptions, nextReset, personName, platformName, seenText, tariffOf, telegramOf } from '../client/model';
import type { ClientCard, Device, SubscriptionDetails } from '../client/types';
import { minutesSince, spanText } from '../home/model';
import './support.css';
import { Avatar } from '../Avatar';
import type { HandoffReason, TicketAi } from '../../lib/supportChat';

type Filter = 'waiting' | 'answered' | 'closed';

type Ticket = {
  id: number;
  client_id: number;
  status: 'open' | 'closed';
  channel: 'bot' | 'cabinet' | 'miniapp' | 'unknown';
  subject: string | null;
  last_author: 'client' | 'support' | 'ai' | 'system';
  created_at: string;
  updated_at: string;
  closed_at: string | null;
};

type ListItem = {
  ticket: Ticket;
  client_name: string | null;
  telegram_username: string | null;
  email: string | null;
  waiting_since: string | null;
  last_message_at: string | null;
  preview: string | null;
  last_author: string | null;
  unread: boolean;
  ai: TicketAi | null;
};

type Counts = { waiting: number; answered: number; closed: number };

type Message = { author: 'client' | 'support' | 'ai' | 'system'; admin_id: number | null; body: string; attachments: unknown[]; created_at: string };

type Thread = { ticket: Ticket; messages: Message[]; admins: Array<{ id: number; email: string }>; ai: TicketAi | null };

const FILTERS: Array<[Filter, string, keyof Counts]> = [
  ['waiting', 'Ждут ответа', 'waiting'],
  ['answered', 'Ответ дан', 'answered'],
  ['closed', 'Закрытые', 'closed'],
];

const TEMPLATES: Array<[string, string]> = [
  ['Обновите подписку', 'Обновите подписку в приложении. Если не поможет, напишите, разберёмся.'],
  ['Подключитесь заново', 'Отключили все устройства. Обновите подписку в приложении и подключитесь заново. Если не поможет, напишите, разберёмся.'],
  ['Дата и время', 'Проверьте дату и время на телефоне. Если не поможет, напишите, разберёмся.'],
];

const HANDOFF_WORD: Record<HandoffReason, string> = {
  human: 'клиент попросил оператора',
  money: 'вопрос про оплату, деньги или возврат',
  unsure: 'ИИ не уверен в ответе',
  groq: 'Groq не ответил',
  client_limit: 'клиент исчерпал ответы ИИ на сегодня',
  budget: 'дневной бюджет ИИ закончился',
  operator: 'оператор ответил сам',
  disabled: 'ИИ-чат выключен',
};

function aiMark(ai: TicketAi | null, status: Ticket['status'], lastAuthor: string | null): string | null {
  if (!ai || status === 'closed') {
    return null;
  }
  if (ai.state === 'active') {
    return lastAuthor === 'client' ? 'ИИ отвечает' : 'Ответил ИИ';
  }
  return ai.reason && ai.reason !== 'operator' && ai.reason !== 'disabled' ? 'От ИИ' : null;
}

function handoffLine(ai: TicketAi | null): string | null {
  if (!ai || ai.state !== 'handed_off' || !ai.handoff_at) {
    return null;
  }
  return ai.reason ? `ИИ передал обращение оператору: ${HANDOFF_WORD[ai.reason]}` : 'ИИ передал обращение оператору';
}

const CHANNEL_WORD: Record<Ticket['channel'], string> = {
  bot: 'через бота',
  cabinet: 'через кабинет',
  miniapp: 'через мини-приложение',
  unknown: '',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function itemName(item: ListItem): string {
  return item.client_name ?? (item.telegram_username ? `@${item.telegram_username}` : (item.email ?? `Клиент ${item.ticket.client_id}`));
}

function ticketState(ticket: Ticket, ai: TicketAi | null): { text: string; color: string } {
  if (ticket.status === 'closed') {
    return { text: 'Закрыто', color: 'var(--muted)' };
  }
  if (ai?.state === 'active') {
    return ticket.last_author === 'client' ? { text: 'ИИ отвечает', color: 'var(--ink-2)' } : { text: 'Ответил ИИ', color: 'var(--ink-2)' };
  }
  if (ticket.last_author === 'client') {
    return { text: 'Ждёт ответа', color: 'var(--warn)' };
  }
  return { text: 'Ответ дан', color: 'var(--ink-2)' };
}

function dayLabel(iso: string): string {
  const date = new Date(iso);
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = Math.round((start(new Date()) - start(date)) / 86_400_000);
  if (diff === 0) {
    return `Сегодня, ${timeHM(iso)}`;
  }
  if (diff === 1) {
    return `Вчера, ${timeHM(iso)}`;
  }
  return `${dateLong(iso)}, ${timeHM(iso)}`;
}

export function Support() {
  const { me, supportWaiting } = useSession();
  const [params, setParams] = useSearchParams();
  const filter = (params.get('filter') as Filter | null) ?? 'waiting';
  const ticketParam = params.get('ticket');
  const [q, setQ] = useState('');
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<ListItem[] | null>(null);
  const [counts, setCounts] = useState<Counts>({ waiting: 0, answered: 0, closed: 0 });
  const [thread, setThread] = useState<Thread | null>(null);
  const [threadError, setThreadError] = useState<string | null>(null);
  const [reply, setReply] = useState('');
  const [closeAfter, setCloseAfter] = useState(false);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const feedRef = useRef<HTMLDivElement | null>(null);
  const selected = ticketParam ? Number(ticketParam) : null;

  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(q.trim()), 300);
    return () => window.clearTimeout(timer);
  }, [q]);

  const loadList = useCallback(async () => {
    try {
      const search = new URLSearchParams({ filter, limit: '50' });
      if (query) {
        search.set('q', query);
      }
      const data = await api<{ items: ListItem[]; counts: Counts }>(`/api/admin/support/tickets?${search.toString()}`);
      setItems(data.items);
      setCounts(data.counts);
      supportWaiting(data.counts.waiting);
      return data.items;
    } catch {
      setItems([]);
      return [];
    }
  }, [filter, query, supportWaiting]);

  useEffect(() => {
    void loadList().then((list) => {
      if (!ticketParam && list[0]) {
        const next = new URLSearchParams(params);
        next.set('ticket', String(list[0].ticket.id));
        setParams(next, { replace: true });
      }
    });
    const timer = window.setInterval(() => void loadList(), 30_000);
    return () => window.clearInterval(timer);
  }, [loadList]);

  const loadThread = useCallback(async (id: number) => {
    try {
      const data = await api<Thread>(`/api/admin/support/tickets/${id}`);
      setThread(data);
      setThreadError(null);
    } catch (err) {
      setThread(null);
      setThreadError(failText(err, 'Обращение не загрузилось'));
    }
  }, []);

  useEffect(() => {
    setReply('');
    setSendError(null);
    setCloseAfter(false);
    if (selected) {
      void loadThread(selected).then(() => void loadList());
    } else {
      setThread(null);
    }
  }, [selected, loadThread]);

  useEffect(() => {
    feedRef.current?.scrollTo({ top: feedRef.current.scrollHeight });
  }, [thread]);

  const pick = (id: number) => {
    const next = new URLSearchParams(params);
    next.set('ticket', String(id));
    setParams(next);
  };

  const pickFilter = (value: Filter) => {
    const next = new URLSearchParams(params);
    next.set('filter', value);
    next.delete('ticket');
    setParams(next);
  };

  const send = async () => {
    if (!thread || !reply.trim()) {
      return;
    }
    setSending(true);
    setSendError(null);
    try {
      const data = await api<Thread>(`/api/admin/support/tickets/${thread.ticket.id}`, {
        method: 'POST',
        body: { body: reply.trim(), close: closeAfter && thread.ticket.status === 'open' },
      });
      setThread(data);
      setReply('');
      setCloseAfter(false);
      void loadList();
    } catch (err) {
      setSendError(failText(err, 'Ответ не отправился'));
    } finally {
      setSending(false);
    }
  };

  const toggleClosed = async () => {
    if (!thread) {
      return;
    }
    const action = thread.ticket.status === 'closed' ? 'reopen' : 'close';
    try {
      await api(`/api/admin/support/tickets/${thread.ticket.id}/${action}`, { method: 'POST', body: {} });
      await loadThread(thread.ticket.id);
      void loadList();
    } catch (err) {
      setSendError(failText(err, 'Статус не изменился'));
    }
  };

  const listed = items?.find((i) => i.ticket.id === selected) ?? null;
  const adminEmail = useMemo(() => new Map((thread?.admins ?? []).map((a) => [a.id, a.email])), [thread]);
  const state = thread ? ticketState(thread.ticket, thread.ai) : null;
  const handoff = thread ? handoffLine(thread.ai) : null;
  const handoffAt = thread?.ai?.handoff_at ? Date.parse(thread.ai.handoff_at) : null;
  const handoffIndex = handoff && handoffAt !== null && thread ? thread.messages.filter((m) => Date.parse(m.created_at) <= handoffAt).length : null;

  return (
    <div className="sp">
      <header className="sp-head">
        <h1 className="page-title tight">Поддержка</h1>
        <div role="group" aria-label="Какие обращения показать" className="chips">
          {FILTERS.map(([key, label, countKey]) => (
            <button key={key} type="button" className="chip" aria-pressed={filter === key} onClick={() => pickFilter(key)}>
              <span>{label}</span>
              <span className="chip-n">{groupDigits(counts[countKey])}</span>
            </button>
          ))}
        </div>
      </header>

      <div className="sp-cols">
        <section aria-label="Обращения" className="sp-sec sp-list">
          <label className="sp-search">
            <Icon name="search" size={17} stroke={1.9} />
            <span className="sr-only">Поиск обращений</span>
            <input type="search" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Имя, тема или текст" maxLength={100} />
          </label>
          <div className="sp-items">
            {items === null ? <p className="sp-empty">Загружаем…</p> : null}
            {items && items.length === 0 ? <p className="sp-empty">Обращений нет</p> : null}
            {items?.map((item) => {
              const name = itemName(item);
              const minutes = minutesSince(item.waiting_since);
              const waitColor =
                filter === 'waiting' && minutes !== null ? (minutes >= 120 ? 'var(--bad)' : minutes >= 30 ? 'var(--warn)' : 'var(--muted)') : 'var(--muted)';
              const time = filter === 'waiting' && minutes !== null ? spanText(minutes) : item.last_message_at ? shortWhen(item.last_message_at) : '';
              const preview = item.preview ? `${item.last_author === 'support' ? 'Вы: ' : item.last_author === 'ai' ? 'ИИ: ' : ''}${item.preview}` : '';
              const mark = aiMark(item.ai, item.ticket.status, item.last_author);
              return (
                <button key={item.ticket.id} type="button" className="sp-item" aria-pressed={item.ticket.id === selected} onClick={() => pick(item.ticket.id)}>
                  <Avatar id={item.ticket.client_id} name={name} className="sp-ava" />
                  <span className="sp-item-main">
                    <span className="sp-item-top">
                      <strong className="sp-ell">{name}</strong>
                      <span className="sp-time num" style={{ color: waitColor }}>
                        {time}
                      </span>
                    </span>
                    <span className="sp-subj-row">
                      <span className="sp-ell sp-subj">{item.ticket.subject ?? 'Без темы'}</span>
                      {mark ? <span className="sp-ai">{mark}</span> : null}
                    </span>
                    <span className="sp-ell sp-prev">{preview}</span>
                  </span>
                  {item.unread ? (
                    <span className="sp-dot" aria-hidden="true">
                      <span className="sr-only">Не прочитано</span>
                    </span>
                  ) : null}
                </button>
              );
            })}
          </div>
        </section>

        <section aria-label="Переписка" className="sp-sec sp-thread">
          {!thread && !threadError ? <p className="sp-empty">{selected ? 'Загружаем переписку…' : 'Выберите обращение слева'}</p> : null}
          {threadError ? (
            <p className="sp-empty err" role="alert">
              {threadError}
            </p>
          ) : null}
          {thread && state ? (
            <>
              <div className="sp-thead">
                <div className="sp-col" style={{ flex: '1 1 240px', minWidth: 0 }}>
                  <h2 className="sp-h2">{thread.ticket.subject ?? 'Без темы'}</h2>
                  <span className="muted">
                    {listed ? itemName(listed) : `Клиент ${thread.ticket.client_id}`}
                    {listed?.telegram_username ? ` · @${listed.telegram_username}` : ''}
                  </span>
                </div>
                <span className="sp-state" style={{ color: state.color }}>
                  <span aria-hidden="true" className="sp-state-dot" style={{ background: state.color }} />
                  {state.text}
                </span>
                <button type="button" className="b38 hov" onClick={() => void toggleClosed()}>
                  {thread.ticket.status === 'closed' ? 'Открыть снова' : 'Закрыть'}
                </button>
              </div>
              <div className="sp-feed" ref={feedRef}>
                {thread.messages.map((m, index) => {
                  const prev = thread.messages[index - 1];
                  const handoffHere = handoff && handoffIndex === index;
                  const newDay = !prev || new Date(prev.created_at).toDateString() !== new Date(m.created_at).toDateString();
                  const mine = m.author === 'support' || m.author === 'ai';
                  const who =
                    m.author === 'support'
                      ? m.admin_id === me.admin.admin_id
                        ? 'вы'
                        : m.admin_id && adminEmail.get(m.admin_id)
                          ? (adminEmail.get(m.admin_id) ?? '').split('@')[0]
                          : 'поддержка'
                      : m.author === 'ai'
                        ? 'ИИ-помощник'
                        : '';
                  const fresh = (minutesSince(m.created_at) ?? 99) < 1;
                  const stamp = `${fresh ? 'сейчас' : timeHM(m.created_at)}${who ? ` · ${who}` : ''}`;
                  return (
                    <div key={`${m.created_at}-${index}`} className="sp-msg-wrap">
                      {handoffHere ? <span className="sp-day">{handoff}</span> : null}
                      {newDay ? (
                        <span className="sp-day">
                          {dayLabel(m.created_at)}
                          {index === 0 && CHANNEL_WORD[thread.ticket.channel] ? ` · ${CHANNEL_WORD[thread.ticket.channel]}` : ''}
                        </span>
                      ) : null}
                      {m.author === 'system' ? (
                        <span className="sp-day">{m.body}</span>
                      ) : (
                        <div className={m.author === 'ai' ? 'sp-msg mine ai' : mine ? 'sp-msg mine' : 'sp-msg'}>
                          <div className="sp-bubble">{m.body}</div>
                          {m.attachments.length ? (
                            <div className="sp-attach">
                              <Icon name="image" size={18} stroke={1.8} />
                              Вложение, {m.attachments.length}
                            </div>
                          ) : null}
                          <span className="sp-stamp">{stamp}</span>
                        </div>
                      )}
                    </div>
                  );
                })}
                {handoff && handoffIndex !== null && handoffIndex >= thread.messages.length ? <span className="sp-day">{handoff}</span> : null}
              </div>
              <div className="sp-reply">
                {thread.ai?.state === 'active' && thread.ticket.status === 'open' ? (
                  <p className="sp-ai-note">Сейчас отвечает ИИ. Если ответите сами, ИИ в этом обращении замолчит.</p>
                ) : null}
                <div role="group" aria-label="Шаблоны ответов" className="sp-templates">
                  {TEMPLATES.map(([label, text]) => (
                    <button key={label} type="button" className="hov" onClick={() => setReply(text)}>
                      {label}
                    </button>
                  ))}
                </div>
                <label className="sr-only" htmlFor="sp-reply">
                  Ответ клиенту
                </label>
                <textarea
                  id="sp-reply"
                  className="sp-field"
                  rows={3}
                  value={reply}
                  maxLength={4000}
                  onChange={(e) => setReply(e.target.value)}
                  placeholder={
                    thread.ticket.channel === 'bot'
                      ? 'Ответ уйдёт клиенту в Telegram'
                      : thread.ticket.channel === 'miniapp'
                        ? 'Ответ появится в мини-приложении и придёт клиенту в Telegram'
                        : me.menu.some((m) => m.module === 'mail')
                        ? 'Ответ уйдёт клиенту в кабинет и на почту'
                        : 'Ответ появится у клиента в кабинете'
                  }
                  disabled={sending}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                      void send();
                    }
                  }}
                />
                {sendError ? (
                  <p className="err" role="alert">
                    {sendError}
                  </p>
                ) : null}
                <div className="sp-send">
                  {thread.ticket.status === 'open' ? (
                    <label className="tf-check">
                      <input type="checkbox" checked={closeAfter} onChange={() => setCloseAfter(!closeAfter)} disabled={sending} />
                      Закрыть после ответа
                    </label>
                  ) : (
                    <span />
                  )}
                  <button type="button" className="btn-main btn" onClick={() => void send()} disabled={sending || !reply.trim()} style={{ height: 42 }}>
                    {sending ? 'Отправляем…' : 'Отправить'}
                  </button>
                </div>
              </div>
            </>
          ) : null}
        </section>

        {thread ? <ClientAside key={thread.ticket.client_id} clientId={thread.ticket.client_id} /> : <aside className="sp-sec sp-aside" aria-label="Клиент" />}
      </div>
    </div>
  );
}

function shortWhen(iso: string): string {
  const minutes = minutesSince(iso) ?? 0;
  if (minutes < 1) {
    return 'сейчас';
  }
  if (minutes < 60 * 24) {
    return spanText(minutes);
  }
  return dateLong(iso);
}

type Ask = '' | 'reset' | 'devices' | 'link';

function ClientAside({ clientId }: { clientId: number }) {
  const { can } = useSession();
  const writable = can('clients_write');
  const [card, setCard] = useState<ClientCard | null>(null);
  const [details, setDetails] = useState<SubscriptionDetails | null>(null);
  const [device, setDevice] = useState<Device | null>(null);
  const [ask, setAsk] = useState<Ask>('');
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await api<ClientCard>(`/api/admin/clients/${clientId}`);
      setCard(data);
      const sub = liveSubscriptions(data)[0];
      if (sub) {
        api<SubscriptionDetails>(`/api/admin/subscriptions/${sub.id}`)
          .then(setDetails)
          .catch(() => setDetails(null));
        if (sub.remna) {
          api<Device[]>(`/api/admin/subscriptions/${sub.id}/actions/devices`, { method: 'POST' })
            .then((list) => setDevice([...list].sort((a, b) => Date.parse(b.added_at) - Date.parse(a.added_at))[0] ?? null))
            .catch(() => setDevice(null));
        }
      }
    } catch {
      setCard(null);
    }
  }, [clientId]);

  useEffect(() => {
    void load();
  }, [load]);

  if (!card) {
    return <aside className="sp-sec sp-aside" aria-label="Клиент" />;
  }
  const sub = liveSubscriptions(card)[0] ?? null;
  const live = details?.subscription ?? sub;
  const tariff = tariffOf(card, sub);
  const tg = telegramOf(card);
  const email = emailOf(card);
  const name = personName(card);
  const blocked = card.client.status === 'blocked';
  const mirror = live?.mirror;
  const left = mirror?.expire_at ? daysLeft(mirror.expire_at) : null;
  const expired = !!live && (mirror?.status === 'expired' || (left !== null && left <= 0));
  const limitBytes = mirror?.traffic_limit_bytes && mirror.traffic_limit_bytes > 0 ? mirror.traffic_limit_bytes : null;
  const exhausted = mirror?.status === 'limited';
  const status = !live
    ? { text: 'Нет подписки', color: 'var(--muted)' }
    : blocked
      ? { text: 'Заблокирован', color: 'var(--bad)' }
      : expired
        ? { text: 'Истекла', color: 'var(--muted)' }
        : exhausted
          ? { text: 'Трафик исчерпан', color: 'var(--warn)' }
          : live.kind === 'trial'
            ? { text: 'Пробный', color: 'var(--ink-2)' }
            : left !== null && left <= 3
              ? { text: 'Истекает', color: 'var(--warn)' }
              : { text: 'Активна', color: 'var(--ink-2)' };
  const reset = live ? nextReset(live, tariff) : '';
  const trafficText = !live
    ? ''
    : limitBytes
      ? `${trafficPair(mirror?.traffic_used_bytes ?? 0, limitBytes)}${reset ? `, сброс ${reset}` : `${NBSP}на весь срок`}`
      : details?.traffic_30d_bytes !== null && details?.traffic_30d_bytes !== undefined
        ? `${traffic(details.traffic_30d_bytes)} за 30${NBSP}дней, без лимита`
        : `${traffic(mirror?.traffic_used_bytes ?? 0)}, без лимита`;
  const seen = mirror?.online_at ? `${seenText(mirror.online_at)}${details?.last_node ? ` через ${details.last_node}` : ''}` : live ? 'ни разу' : '';
  const app = device ? [device.app, [platformName(device.platform), device.os_version].filter(Boolean).join(' ')].filter(Boolean).join(', ') : '';

  const act = async (action: string, body: unknown, done: string) => {
    if (!live) {
      return;
    }
    setBusy(true);
    setNote(null);
    try {
      await api(`/api/admin/subscriptions/${live.id}/actions/${action}`, { method: 'POST', body });
      setAsk('');
      setNote(done);
      await load();
    } catch (err) {
      setNote(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const unblock = async () => {
    setBusy(true);
    try {
      await api(`/api/admin/clients/${clientId}/unblock`, { method: 'POST' });
      setNote('Клиент разблокирован');
      await load();
    } catch (err) {
      setNote(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const confirm = (key: Ask, head: string, tail: string, label: string, run: () => void) =>
    ask === key ? (
      <div role="status" className="sp-confirm">
        <span>
          <strong>{head}</strong> {tail}
        </span>
        <button type="button" className="b34 on-surface" onClick={run} disabled={busy}>
          {label}
        </button>
        <button type="button" className="b34 on-surface" onClick={() => setAsk('')} disabled={busy}>
          Отменить
        </button>
      </div>
    ) : null;

  return (
    <aside className="sp-sec sp-aside" aria-label="Клиент">
      <div className="sp-person">
        <Avatar id={card.client.id} name={name} className="sp-ava big" retry />
        <span className="sp-col" style={{ minWidth: 0 }}>
          <strong style={{ fontSize: 16 }}>{name}</strong>
          <span className="muted" style={{ overflowWrap: 'anywhere' }}>
            {tg?.meta?.username ? `@${tg.meta.username}` : (email?.value ?? (tg ? `Telegram ${tg.value}` : ''))}
          </span>
        </span>
      </div>
      {live ? (
        <div className="sp-col">
          <span className="sp-small muted">{planTitle(live, tariff)}</span>
          <span className="sp-days">
            <strong>{days(expired ? 0 : Math.max(left ?? 0, 0))}</strong>
            <span style={{ color: status.color, fontWeight: 600 }}>{status.text}</span>
          </span>
          <span className="sp-small muted">{mirror?.expire_at ? `${expired ? 'была до' : 'до'} ${dateNumeric(mirror.expire_at)}` : ''}</span>
        </div>
      ) : (
        <p className="muted">{status.text}</p>
      )}
      <div className="sp-col">
        {live ? (
          <>
            <div className="sp-fact">
              <span className="muted">Устройства</span>
              <span className="num">
                {mirror?.devices_count ?? 0}
                {mirror?.hwid_limit ? `${NBSP}из${NBSP}${mirror.hwid_limit}` : ''}
              </span>
            </div>
            {app ? (
              <div className="sp-fact">
                <span className="muted">Приложение</span>
                <span>{app}</span>
              </div>
            ) : null}
            <div className="sp-fact">
              <span className="muted">Трафик</span>
              <span style={{ color: exhausted ? 'var(--warn)' : undefined }}>{trafficText}</span>
            </div>
            <div className="sp-fact">
              <span className="muted">В сети</span>
              <span style={{ color: seen === 'ни разу' ? 'var(--warn)' : undefined }}>{seen}</span>
            </div>
          </>
        ) : null}
        {card.balance && card.balance.minor > 0 ? (
          <div className="sp-fact">
            <span className="muted">Баланс</span>
            <span className="num">{money(card.balance)}</span>
          </div>
        ) : null}
        {card.stats.paid_total ? (
          <div className="sp-fact">
            <span className="muted">Всего оплачено</span>
            <span className="num">{money(card.stats.paid_total)}</span>
          </div>
        ) : null}
      </div>
      {writable ? (
        <div className="sp-col" style={{ gap: 8 }}>
          {blocked ? (
            <button type="button" className="sp-wide hov" onClick={() => void unblock()} disabled={busy}>
              Разблокировать
            </button>
          ) : null}
          {live && exhausted && limitBytes ? (
            <>
              <button type="button" className="sp-wide hov" aria-expanded={ask === 'reset'} onClick={() => setAsk(ask === 'reset' ? '' : 'reset')} disabled={busy}>
                Сбросить трафик
              </button>
              {confirm('reset', 'Обнулить трафик?', `Сейчас ${trafficPair(mirror?.traffic_used_bytes ?? 0, limitBytes)}, станет 0. Отменить будет нельзя.`, 'Сбросить трафик', () => void act('reset-traffic', undefined, 'Трафик обнулён'))}
            </>
          ) : null}
          {live ? (
            <>
              <button type="button" className="sp-wide hov" onClick={() => void act('extend', { days: 3 }, 'Добавлено 3 дня')} disabled={busy}>
                Добавить 3 дня
              </button>
              {live.remna ? (
                <>
                  <button type="button" className="sp-wide hov" aria-expanded={ask === 'devices'} onClick={() => setAsk(ask === 'devices' ? '' : 'devices')} disabled={busy}>
                    Отключить все устройства
                  </button>
                  {confirm(
                    'devices',
                    'Все устройства клиента отключатся.',
                    'Подключатся снова, когда клиент обновит подписку в приложении, ссылка останется прежней.',
                    'Отключить все устройства',
                    () => void act('disconnect-devices', undefined, 'Все устройства отключены'),
                  )}
                  <button type="button" className="sp-wide hov" aria-expanded={ask === 'link'} onClick={() => setAsk(ask === 'link' ? '' : 'link')} disabled={busy}>
                    Выпустить новую ссылку
                  </button>
                  {confirm(
                    'link',
                    'Старая ссылка и все устройства клиента перестанут работать.',
                    tg ? 'Новую ссылку клиент получит в боте и в кабинете.' : 'Новая ссылка будет в кабинете клиента.',
                    'Выпустить новую ссылку',
                    () => void act('reissue-link', undefined, 'Новая ссылка выпущена'),
                  )}
                </>
              ) : null}
            </>
          ) : null}
          {note ? (
            <p role="status" className="sp-small" style={{ color: 'var(--ink-2)' }}>
              {note}
            </p>
          ) : null}
        </div>
      ) : null}
      <Link to={`/admin/clients/${clientId}`} className="sp-open">
        Открыть карточку клиента
        <Icon name="chevron-right" size={16} stroke={2} />
      </Link>
    </aside>
  );
}
